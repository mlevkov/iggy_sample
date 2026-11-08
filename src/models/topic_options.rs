//! Per-topic options: what a client may set when it creates a topic, and how
//! a topic's stored options are reported back.
//!
//! Since server 0.9, segment, flush and durability settings belong to each
//! topic and are fixed when it is created ([ADR-0001] PR 2, built on SDK 0.12
//! per [ADR-0002]). The gateway takes them from the `options` object of
//! `POST /streams/{stream}/topics`, and the default topic's from `IGGY_TOPIC_*`
//! environment variables. Both go through [`TopicOptionsInput::validate`].
//!
//! # Values
//!
//! Sizes and durations use Iggy's text forms: `"256 MiB"`, `"1 GB"`,
//! `"7days"`, `"1h 30m"`, and `"unlimited"` where a limit can be lifted.
//! Iggy's spellings for "use the server default" (`"server_default"`, `"0"`)
//! are refused: omitting the option says the same, and the topic's read-back
//! then shows that the server chose the value.
//!
//! # Provenance
//!
//! The server stores each option with an `explicit` flag. The flag is set when
//! the creating client sent the key, and clear when admission filled in a
//! default. Topic reads report it as [`OptionSource`].
//!
//! SDK 0.12 always sends both durability keys. On topics created through the
//! SDK, this gateway included, `durability` and `consumer_offset_durability`
//! therefore always read back as explicit, and an omitted one reads
//! `"replicated"`.
//!
//! # Not offered
//!
//! - `compression_algorithm`: the server stores it but does not apply it.
//! - `partition_resize_policy`: outside ADR-0001's field list.
//!
//! [ADR-0001]: https://github.com/mlevkov/iggy_sample/blob/main/docs/adr/0001-iggy-0.9-upgrade-and-demos.md
//! [ADR-0002]: https://github.com/mlevkov/iggy_sample/blob/main/docs/adr/0002-iggy-0.10-before-topic-options.md

use std::str::FromStr;

use iggy::prelude::{
    Durability, HeaderKey, IggyByteSize, IggyExpiry, MaxTopicSize, ResourceOptions,
    TopicCreateOptions, topic_option_keys as keys,
};
use iggy_common::{
    DEFAULT_SEGMENT_SIZE, MAX_MESSAGES_REQUIRED_TO_SAVE, MAX_PREALLOCATED_TOPIC_BYTES,
    MAX_REPLICATION_FACTOR, MAX_SIZE_OF_MESSAGES_REQUIRED_TO_SAVE, MAX_TOPIC_SEGMENT_SIZE,
    validate_preallocated_topic_bytes, validate_topic_segment_size,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The reason given for a value that means "server default" in Iggy.
const SERVER_DEFAULT_REFUSED: &str = "omit the option to use the server default";

/// Per-topic options as a client or the environment supplies them, before
/// validation. Every option is optional; an omitted one resolves to the
/// server's default when the topic is created.
///
/// Unknown keys are refused, so a misspelt option fails instead of silently
/// leaving the topic on a default.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TopicOptionsInput {
    /// `"replicated"` or `"persisted"`: the storage guarantee a send waits for.
    pub durability: Option<String>,
    /// `"replicated"` or `"persisted"`: the same, for explicit consumer-offset
    /// stores and deletes.
    pub consumer_offset_durability: Option<String>,
    /// How long messages are kept: a duration such as `"7days"`, or `"unlimited"`.
    pub message_expiry: Option<String>,
    /// Retained size past which the oldest sealed segments are deleted: a size
    /// such as `"10 GiB"`, or `"unlimited"`.
    pub max_topic_size: Option<String>,
    /// Size at which a partition starts a new segment: 1 MiB to 1 GiB, in
    /// multiples of 512 B.
    pub segment_size: Option<String>,
    /// Buffered messages that schedule a segment write: 1 to 16 Mi.
    pub messages_required_to_save: Option<u32>,
    /// Buffered bytes that schedule a segment write: 1 B to 1 GiB.
    pub size_of_messages_required_to_save: Option<String>,
    /// Whether each new segment reserves its full size on disk when it opens.
    pub preallocate_segments: Option<bool>,
    /// Nodes that store each partition: 1 to 32, and at most the cluster's
    /// node count (the server refuses a larger one). Omitted, the server's
    /// `default_replication_factor` applies (`"all"` unless an operator
    /// changed it).
    pub replication_factor: Option<u32>,
}

/// An option that failed validation: the name the caller used for it and why.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{name}: {reason}")]
pub struct TopicOptionError {
    /// The option as the caller named it, e.g. `options.segment_size` or
    /// `IGGY_TOPIC_SEGMENT_SIZE`.
    pub name: String,
    /// Why the value was refused.
    pub reason: String,
}

impl TopicOptionsInput {
    /// Validate every supplied option for a topic of `partitions` partitions.
    ///
    /// `label` maps an option key (`segment_size`) to the name the caller
    /// used for it (`options.segment_size`, `IGGY_TOPIC_SEGMENT_SIZE`), for
    /// the error.
    ///
    /// # Errors
    ///
    /// The first option that fails to parse or is out of range, checked in
    /// field order. The checks that combine options come last: a finite
    /// `max_topic_size` must be at least the segment size, and preallocation
    /// must fit the server's cap across all partitions.
    pub fn validate(
        &self,
        partitions: u32,
        label: impl Fn(&str) -> String,
    ) -> Result<TopicOptions, TopicOptionError> {
        let fail = |key: &str, reason: String| TopicOptionError {
            name: label(key),
            reason,
        };

        let options = TopicOptions {
            durability: parse_option(self.durability.as_deref(), parse_durability)
                .map_err(|reason| fail(keys::DURABILITY, reason))?,
            consumer_offset_durability: parse_option(
                self.consumer_offset_durability.as_deref(),
                parse_durability,
            )
            .map_err(|reason| fail(keys::CONSUMER_OFFSET_DURABILITY, reason))?,
            message_expiry: parse_option(self.message_expiry.as_deref(), parse_message_expiry)
                .map_err(|reason| fail(keys::MESSAGE_EXPIRY, reason))?,
            max_topic_size: parse_option(self.max_topic_size.as_deref(), parse_max_topic_size)
                .map_err(|reason| fail(keys::MAX_TOPIC_SIZE, reason))?,
            segment_size: parse_option(self.segment_size.as_deref(), parse_segment_size)
                .map_err(|reason| fail(keys::SEGMENT_SIZE, reason))?,
            messages_required_to_save: self
                .messages_required_to_save
                .map(check_messages_required_to_save)
                .transpose()
                .map_err(|reason| fail(keys::MESSAGES_REQUIRED_TO_SAVE, reason))?,
            size_of_messages_required_to_save: parse_option(
                self.size_of_messages_required_to_save.as_deref(),
                parse_size_of_messages_required_to_save,
            )
            .map_err(|reason| fail(keys::SIZE_OF_MESSAGES_REQUIRED_TO_SAVE, reason))?,
            preallocate_segments: self.preallocate_segments,
            replication_factor: self
                .replication_factor
                .map(check_replication_factor)
                .transpose()
                .map_err(|reason| fail(keys::REPLICATION_FACTOR, reason))?,
        };

        let segment_bytes = options
            .segment_size
            .map_or(DEFAULT_SEGMENT_SIZE, |size| size.as_bytes_u64());

        // The server refuses this too (`InvalidTopicSize`), but the client
        // gets only the error code, and the SDK prints placeholder values
        // for both sizes (apache/iggy#4785). The whole topic's cap is
        // compared, not the per-partition share it is enforced as.
        if let Some(MaxTopicSize::Custom(cap)) = options.max_topic_size
            && cap.as_bytes_u64() < segment_bytes
        {
            return Err(fail(
                keys::MAX_TOPIC_SIZE,
                format!(
                    "must be at least the segment size ({}); set a smaller segment_size \
                     for a smaller cap",
                    format_size(segment_bytes)
                ),
            ));
        }

        if options.preallocate_segments == Some(true) {
            validate_preallocated_topic_bytes(segment_bytes, partitions).map_err(|_| {
                fail(
                    keys::PREALLOCATE_SEGMENTS,
                    format!(
                        "{partitions} partitions of {} reserve more than the server's {} \
                         preallocation limit",
                        format_size(segment_bytes),
                        format_size(MAX_PREALLOCATED_TOPIC_BYTES),
                    ),
                )
            })?;
        }

        Ok(options)
    }
}

/// Validated per-topic options. Each `Some` is sent explicitly when the topic
/// is created; each `None` is left to the server's default.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TopicOptions {
    /// Storage guarantee a send waits for.
    pub durability: Option<Durability>,
    /// Storage guarantee an explicit consumer-offset store or delete waits for.
    pub consumer_offset_durability: Option<Durability>,
    /// How long messages are kept.
    pub message_expiry: Option<IggyExpiry>,
    /// Retained size past which the oldest sealed segments are deleted.
    pub max_topic_size: Option<MaxTopicSize>,
    /// Size at which a partition starts a new segment.
    pub segment_size: Option<IggyByteSize>,
    /// Buffered messages that schedule a segment write.
    pub messages_required_to_save: Option<u32>,
    /// Buffered bytes that schedule a segment write.
    pub size_of_messages_required_to_save: Option<IggyByteSize>,
    /// Whether each new segment reserves its full size on disk when it opens.
    pub preallocate_segments: Option<bool>,
    /// Nodes that store each partition.
    pub replication_factor: Option<u32>,
}

impl TopicOptions {
    /// The SDK's create options for a topic of `partitions` partitions.
    ///
    /// The SDK has no "unset" for the two durability fields and sends both,
    /// so an omitted durability goes out as its default, `replicated`.
    #[must_use]
    pub fn create_options(&self, partitions: u32) -> TopicCreateOptions {
        TopicCreateOptions {
            partitions_count: Some(partitions),
            message_expiry: self.message_expiry,
            max_topic_size: self.max_topic_size,
            segment_size: self.segment_size,
            durability: self.durability.unwrap_or_default(),
            consumer_offset_durability: self.consumer_offset_durability.unwrap_or_default(),
            messages_required_to_save: self.messages_required_to_save,
            size_of_messages_required_to_save: self.size_of_messages_required_to_save,
            preallocate_segments: self.preallocate_segments,
            replication_factor: self.replication_factor,
            ..TopicCreateOptions::default()
        }
    }

    /// The options set here that `stored` contradicts, as
    /// `(key, set here, stored)`. An option set here that the topic does
    /// not store counts as a difference, with `"unset"` as the stored value.
    #[must_use]
    pub fn differences(&self, stored: &TopicOptionsView) -> Vec<OptionDifference> {
        let mut differences = Vec::new();
        let mut compare = |key: &'static str, wanted: Option<String>, stored: Option<String>| {
            if let Some(wanted) = wanted
                && stored.as_ref() != Some(&wanted)
            {
                differences.push(OptionDifference {
                    key,
                    wanted,
                    stored: stored.unwrap_or_else(|| "unset".to_string()),
                });
            }
        };
        compare(
            keys::DURABILITY,
            self.durability.map(format_durability),
            stored.durability.value.clone(),
        );
        compare(
            keys::CONSUMER_OFFSET_DURABILITY,
            self.consumer_offset_durability.map(format_durability),
            stored.consumer_offset_durability.value.clone(),
        );
        compare(
            keys::MESSAGE_EXPIRY,
            self.message_expiry.map(format_message_expiry),
            stored.message_expiry.value.clone(),
        );
        compare(
            keys::MAX_TOPIC_SIZE,
            self.max_topic_size.map(format_max_topic_size),
            stored.max_topic_size.value.clone(),
        );
        compare(
            keys::SEGMENT_SIZE,
            self.segment_size.map(format_byte_size),
            stored.segment_size.value.clone(),
        );
        compare(
            keys::MESSAGES_REQUIRED_TO_SAVE,
            self.messages_required_to_save.map(|n| n.to_string()),
            stored
                .messages_required_to_save
                .value
                .map(|n| n.to_string()),
        );
        compare(
            keys::SIZE_OF_MESSAGES_REQUIRED_TO_SAVE,
            self.size_of_messages_required_to_save.map(format_byte_size),
            stored.size_of_messages_required_to_save.value.clone(),
        );
        compare(
            keys::PREALLOCATE_SEGMENTS,
            self.preallocate_segments.map(|b| b.to_string()),
            stored.preallocate_segments.value.map(|b| b.to_string()),
        );
        compare(
            keys::REPLICATION_FACTOR,
            self.replication_factor.map(|n| n.to_string()),
            stored.replication_factor.value.map(|n| n.to_string()),
        );
        differences
    }
}

/// One option on which wanted and stored values disagree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptionDifference {
    /// The option key, e.g. `durability`.
    pub key: &'static str,
    /// The value asked for, in reported form.
    pub wanted: String,
    /// The topic's stored value, in reported form, or `"unset"`.
    pub stored: String,
}

/// Who chose a stored option's value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OptionSource {
    /// The client that created the topic sent it.
    Explicit,
    /// The server filled in its default when it admitted the topic.
    Default,
    /// The topic stores no value, so its partitions use the server's built-in
    /// setting, which a client cannot read.
    Unset,
}

/// A stored option's value and who chose it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReportedOption<T> {
    /// The value, in the form `POST` accepts. `null` when the topic stores
    /// none, or stores one this build cannot read.
    pub value: Option<T>,
    /// Who chose it.
    pub source: OptionSource,
}

/// A topic's stored options, as topic reads report them. Every value is in
/// the form the create request accepts, so it can be sent back unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TopicOptionsView {
    pub durability: ReportedOption<String>,
    pub consumer_offset_durability: ReportedOption<String>,
    pub message_expiry: ReportedOption<String>,
    pub max_topic_size: ReportedOption<String>,
    pub segment_size: ReportedOption<String>,
    pub messages_required_to_save: ReportedOption<u32>,
    pub size_of_messages_required_to_save: ReportedOption<String>,
    pub preallocate_segments: ReportedOption<bool>,
    /// The factor admission resolved and stored: on a single node, or with
    /// `default_replication_factor = "all"`, the node count at creation.
    pub replication_factor: ReportedOption<u32>,
}

impl TopicOptionsView {
    /// Report a topic's stored options, as the SDK returns them on `Topic`
    /// and `TopicDetails`.
    #[must_use]
    pub fn from_stored(stored: &ResourceOptions) -> Self {
        let typed = TopicCreateOptions::from_resource_options(stored);
        Self {
            durability: report(
                stored,
                keys::DURABILITY,
                Some(format_durability(typed.durability)),
            ),
            consumer_offset_durability: report(
                stored,
                keys::CONSUMER_OFFSET_DURABILITY,
                Some(format_durability(typed.consumer_offset_durability)),
            ),
            message_expiry: report(
                stored,
                keys::MESSAGE_EXPIRY,
                typed.message_expiry.map(format_message_expiry),
            ),
            max_topic_size: report(
                stored,
                keys::MAX_TOPIC_SIZE,
                typed.max_topic_size.map(format_max_topic_size),
            ),
            segment_size: report(
                stored,
                keys::SEGMENT_SIZE,
                typed.segment_size.map(format_byte_size),
            ),
            messages_required_to_save: report(
                stored,
                keys::MESSAGES_REQUIRED_TO_SAVE,
                typed.messages_required_to_save,
            ),
            size_of_messages_required_to_save: report(
                stored,
                keys::SIZE_OF_MESSAGES_REQUIRED_TO_SAVE,
                typed
                    .size_of_messages_required_to_save
                    .map(format_byte_size),
            ),
            preallocate_segments: report(
                stored,
                keys::PREALLOCATE_SEGMENTS,
                typed.preallocate_segments,
            ),
            replication_factor: report(stored, keys::REPLICATION_FACTOR, typed.replication_factor),
        }
    }
}

/// Report `key`: its source from the stored entry, and `value` only when the
/// topic stores the key at all.
fn report<T>(stored: &ResourceOptions, key: &str, value: Option<T>) -> ReportedOption<T> {
    match HeaderKey::from_str(key)
        .ok()
        .and_then(|key| stored.get(&key))
    {
        Some(entry) => ReportedOption {
            value,
            source: if entry.explicit {
                OptionSource::Explicit
            } else {
                OptionSource::Default
            },
        },
        None => ReportedOption {
            value: None,
            source: OptionSource::Unset,
        },
    }
}

// ---------------------------------------------------------------------------
// Parsing. Each parser trims its input and returns the reason it refused it.
// ---------------------------------------------------------------------------

/// Parse an option that may be absent.
fn parse_option<T>(
    raw: Option<&str>,
    parse: fn(&str) -> Result<T, String>,
) -> Result<Option<T>, String> {
    raw.map(parse).transpose()
}

fn parse_durability(raw: &str) -> Result<Durability, String> {
    Durability::from_str(raw.trim()).map_err(|_| "must be \"replicated\" or \"persisted\"".into())
}

fn parse_message_expiry(raw: &str) -> Result<IggyExpiry, String> {
    let raw = raw.trim();
    if raw.eq_ignore_ascii_case("unlimited") {
        return Ok(IggyExpiry::NeverExpire);
    }
    // Not lowercased: humantime reads "M" as months and "m" as minutes.
    match IggyExpiry::from_str(raw) {
        Ok(IggyExpiry::ExpireDuration(duration)) if duration.is_zero() => {
            Err(SERVER_DEFAULT_REFUSED.into())
        }
        Ok(IggyExpiry::ServerDefault) => Err(SERVER_DEFAULT_REFUSED.into()),
        Ok(expiry) => Ok(expiry),
        Err(error) => Err(format!(
            "must be a duration such as \"7days\" or \"1h 30m\", or \"unlimited\" ({error})"
        )),
    }
}

fn parse_max_topic_size(raw: &str) -> Result<MaxTopicSize, String> {
    let raw = raw.trim();
    if raw.eq_ignore_ascii_case("unlimited") {
        return Ok(MaxTopicSize::Unlimited);
    }
    let size = parse_byte_size(raw).map_err(|reason| format!("{reason}, or \"unlimited\""))?;
    if size.as_bytes_u64() == 0 {
        // Iggy reads 0 (and "none") as the server default, not as no limit.
        return Err(format!(
            "{SERVER_DEFAULT_REFUSED}, or use \"unlimited\" for no limit"
        ));
    }
    Ok(MaxTopicSize::Custom(size))
}

fn parse_segment_size(raw: &str) -> Result<IggyByteSize, String> {
    let size = parse_byte_size(raw)?;
    validate_topic_segment_size(size.as_bytes_u64(), MAX_TOPIC_SEGMENT_SIZE)
        .map_err(|_| "must be 1 MiB to 1 GiB, in multiples of 512 B".to_string())?;
    Ok(size)
}

fn check_messages_required_to_save(count: u32) -> Result<u32, String> {
    if (1..=MAX_MESSAGES_REQUIRED_TO_SAVE).contains(&count) {
        Ok(count)
    } else {
        Err(format!("must be 1 to {MAX_MESSAGES_REQUIRED_TO_SAVE}"))
    }
}

fn check_replication_factor(factor: u32) -> Result<u32, String> {
    if (1..=MAX_REPLICATION_FACTOR).contains(&factor) {
        Ok(factor)
    } else {
        Err(format!(
            "must be 1 to {MAX_REPLICATION_FACTOR}; omit it for the server's default"
        ))
    }
}

fn parse_size_of_messages_required_to_save(raw: &str) -> Result<IggyByteSize, String> {
    let size = parse_byte_size(raw)?;
    if size.as_bytes_u64() == 0 {
        return Err(SERVER_DEFAULT_REFUSED.into());
    }
    if size.as_bytes_u64() > MAX_SIZE_OF_MESSAGES_REQUIRED_TO_SAVE {
        return Err(format!(
            "must be at most {}",
            format_size(MAX_SIZE_OF_MESSAGES_REQUIRED_TO_SAVE)
        ));
    }
    Ok(size)
}

fn parse_byte_size(raw: &str) -> Result<IggyByteSize, String> {
    IggyByteSize::from_str(raw.trim()).map_err(|_| "must be a size such as \"256 MiB\"".into())
}

// ---------------------------------------------------------------------------
// Formatting. Every form here parses back to the same value.
// ---------------------------------------------------------------------------

fn format_durability(durability: Durability) -> String {
    durability.to_string()
}

fn format_message_expiry(expiry: IggyExpiry) -> String {
    match expiry {
        IggyExpiry::NeverExpire => "unlimited".to_string(),
        IggyExpiry::ExpireDuration(duration) => duration.as_human_time_string(),
        IggyExpiry::ServerDefault => "server_default".to_string(),
    }
}

fn format_max_topic_size(size: MaxTopicSize) -> String {
    match size {
        MaxTopicSize::Unlimited => "unlimited".to_string(),
        MaxTopicSize::Custom(size) => format_byte_size(size),
        MaxTopicSize::ServerDefault => "server_default".to_string(),
    }
}

fn format_byte_size(size: IggyByteSize) -> String {
    format_size(size.as_bytes_u64())
}

/// `bytes` in the largest unit that divides it exactly, so it parses back
/// unchanged. The SDK's own display rounds to two decimals ("1.07 GB" for
/// 1 GiB), which does not.
fn format_size(bytes: u64) -> String {
    const UNITS: [(u64, &str); 8] = [
        (1 << 40, "TiB"),
        (1_000_000_000_000, "TB"),
        (1 << 30, "GiB"),
        (1_000_000_000, "GB"),
        (1 << 20, "MiB"),
        (1_000_000, "MB"),
        (1 << 10, "KiB"),
        (1_000, "KB"),
    ];
    UNITS
        .iter()
        .find(|(unit, _)| bytes >= *unit && bytes.is_multiple_of(*unit))
        .map_or_else(
            || format!("{bytes} B"),
            |(unit, name)| format!("{} {name}", bytes / unit),
        )
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use iggy::prelude::OptionValue;

    use super::*;

    fn label(key: &str) -> String {
        format!("options.{key}")
    }

    fn validate(input: TopicOptionsInput) -> Result<TopicOptions, TopicOptionError> {
        input.validate(1, label)
    }

    fn refused(input: TopicOptionsInput) -> TopicOptionError {
        validate(input).expect_err("input should be refused")
    }

    #[test]
    fn test_empty_input_leaves_every_option_to_the_server() {
        let options = validate(TopicOptionsInput::default()).unwrap();
        assert_eq!(options, TopicOptions::default());

        let create = options.create_options(3);
        assert_eq!(create.partitions_count, Some(3));
        assert_eq!(create.message_expiry, None);
        assert_eq!(create.max_topic_size, None);
        assert_eq!(create.segment_size, None);
        assert_eq!(create.durability, Durability::Replicated);
    }

    #[test]
    fn test_full_input_parses_into_create_options() {
        let input = TopicOptionsInput {
            durability: Some("persisted".into()),
            consumer_offset_durability: Some(" Persisted ".into()),
            message_expiry: Some("7days".into()),
            max_topic_size: Some("10 GiB".into()),
            segment_size: Some("256 MiB".into()),
            messages_required_to_save: Some(1),
            size_of_messages_required_to_save: Some("4 KiB".into()),
            preallocate_segments: Some(false),
            replication_factor: Some(3),
        };
        let create = validate(input).unwrap().create_options(2);

        assert_eq!(create.durability, Durability::Persisted);
        assert_eq!(create.consumer_offset_durability, Durability::Persisted);
        assert_eq!(
            create.message_expiry.map(format_message_expiry).as_deref(),
            Some("7days")
        );
        assert_eq!(
            create.max_topic_size,
            Some(MaxTopicSize::Custom(IggyByteSize::from(10_u64 << 30)))
        );
        assert_eq!(create.segment_size, Some(IggyByteSize::from(256_u64 << 20)));
        assert_eq!(create.messages_required_to_save, Some(1));
        assert_eq!(
            create.size_of_messages_required_to_save,
            Some(IggyByteSize::from(4096_u64))
        );
        assert_eq!(create.preallocate_segments, Some(false));
        assert_eq!(create.replication_factor, Some(3));
    }

    #[test]
    fn test_unlimited_lifts_expiry_and_size_limits() {
        let input = TopicOptionsInput {
            message_expiry: Some("Unlimited".into()),
            max_topic_size: Some("unlimited".into()),
            ..Default::default()
        };
        let options = validate(input).unwrap();
        assert_eq!(options.message_expiry, Some(IggyExpiry::NeverExpire));
        assert_eq!(options.max_topic_size, Some(MaxTopicSize::Unlimited));
    }

    #[test]
    fn test_unknown_durability_is_refused_with_the_callers_name() {
        let error = refused(TopicOptionsInput {
            consumer_offset_durability: Some("fsync".into()),
            ..Default::default()
        });
        assert_eq!(error.name, "options.consumer_offset_durability");
        assert!(error.reason.contains("\"persisted\""));
    }

    #[test]
    fn test_server_default_spellings_are_refused() {
        for raw in ["server_default", "default", "0s"] {
            let error = refused(TopicOptionsInput {
                message_expiry: Some(raw.into()),
                ..Default::default()
            });
            assert_eq!(
                error.reason, SERVER_DEFAULT_REFUSED,
                "message_expiry {raw:?}"
            );
        }
        // Iggy reads "none" as the server default for a size, not as no limit.
        for raw in ["0", "none", "server_default"] {
            let error = refused(TopicOptionsInput {
                max_topic_size: Some(raw.into()),
                ..Default::default()
            });
            assert_eq!(
                error.name, "options.max_topic_size",
                "max_topic_size {raw:?}"
            );
        }
        let error = refused(TopicOptionsInput {
            size_of_messages_required_to_save: Some("0".into()),
            ..Default::default()
        });
        assert_eq!(error.reason, SERVER_DEFAULT_REFUSED);
    }

    #[test]
    fn test_message_expiry_needs_a_unit() {
        let error = refused(TopicOptionsInput {
            message_expiry: Some("3600".into()),
            ..Default::default()
        });
        assert_eq!(error.name, "options.message_expiry");
    }

    #[test]
    fn test_segment_size_bounds_follow_the_server() {
        for raw in ["1023 KiB", "2 GiB", "1048577", "0"] {
            let error = refused(TopicOptionsInput {
                segment_size: Some(raw.into()),
                ..Default::default()
            });
            assert_eq!(error.name, "options.segment_size", "segment_size {raw:?}");
        }
        for raw in ["1 MiB", "1 GiB", "1049088 B"] {
            let input = TopicOptionsInput {
                segment_size: Some(raw.into()),
                ..Default::default()
            };
            assert!(validate(input).is_ok(), "segment_size {raw:?}");
        }
    }

    #[test]
    fn test_replication_factor_is_bounded() {
        for factor in [0, MAX_REPLICATION_FACTOR + 1] {
            let error = refused(TopicOptionsInput {
                replication_factor: Some(factor),
                ..Default::default()
            });
            assert_eq!(error.name, "options.replication_factor");
        }
        let options = validate(TopicOptionsInput {
            replication_factor: Some(2),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(options.create_options(3).replication_factor, Some(2));
    }

    #[test]
    fn test_flush_thresholds_are_bounded() {
        for count in [0, MAX_MESSAGES_REQUIRED_TO_SAVE + 1] {
            let error = refused(TopicOptionsInput {
                messages_required_to_save: Some(count),
                ..Default::default()
            });
            assert_eq!(error.name, "options.messages_required_to_save");
        }
        let error = refused(TopicOptionsInput {
            size_of_messages_required_to_save: Some("2 GiB".into()),
            ..Default::default()
        });
        assert_eq!(error.reason, "must be at most 1 GiB");
    }

    #[test]
    fn test_finite_topic_cap_must_cover_one_segment() {
        // Unset segment_size means the server's 1 GiB.
        let error = refused(TopicOptionsInput {
            max_topic_size: Some("512 MiB".into()),
            ..Default::default()
        });
        assert_eq!(error.name, "options.max_topic_size");
        assert!(error.reason.contains("(1 GiB)"), "{}", error.reason);

        let error = refused(TopicOptionsInput {
            max_topic_size: Some("1 MiB".into()),
            segment_size: Some("16 MiB".into()),
            ..Default::default()
        });
        assert!(error.reason.contains("(16 MiB)"), "{}", error.reason);

        // Equal is enough, and the cap is the whole topic's, whatever the
        // partition count (as the server checks it).
        let input = TopicOptionsInput {
            max_topic_size: Some("16 MiB".into()),
            segment_size: Some("16 MiB".into()),
            ..Default::default()
        };
        assert!(input.validate(2, label).is_ok());
        let unlimited = TopicOptionsInput {
            max_topic_size: Some("unlimited".into()),
            segment_size: Some("16 MiB".into()),
            ..Default::default()
        };
        assert!(validate(unlimited).is_ok());
    }

    #[test]
    fn test_preallocation_is_capped_across_partitions() {
        let input = TopicOptionsInput {
            preallocate_segments: Some(true),
            ..Default::default()
        };
        // 64 partitions of the default 1 GiB segment reach the 64 GiB cap.
        assert!(input.validate(64, label).is_ok());
        let error = input.validate(65, label).expect_err("past the cap");
        assert_eq!(error.name, "options.preallocate_segments");
        assert!(error.reason.contains("65 partitions of 1 GiB"));

        let small = TopicOptionsInput {
            segment_size: Some("16 MiB".into()),
            ..input
        };
        assert!(small.validate(1000, label).is_ok());
    }

    #[test]
    fn test_unknown_option_keys_are_refused_on_deserialize() {
        let error = serde_json::from_str::<TopicOptionsInput>(r#"{"durabilty":"persisted"}"#)
            .expect_err("misspelt key");
        assert!(error.to_string().contains("unknown field `durabilty`"));
    }

    #[test]
    fn test_format_size_uses_the_largest_exact_unit() {
        assert_eq!(format_size(1 << 30), "1 GiB");
        assert_eq!(format_size(1_000_000_000), "1 GB");
        assert_eq!(format_size(256 << 20), "256 MiB");
        assert_eq!(format_size(1_049_088), "1049088 B");
        assert_eq!(format_size(1500), "1500 B");
        assert_eq!(format_size(0), "0 B");
    }

    #[test]
    fn test_formatted_values_parse_back_unchanged() {
        for bytes in [
            512,
            1000,
            4096,
            1_049_088,
            256 << 20,
            1_000_000_000,
            1 << 30,
        ] {
            let parsed = parse_byte_size(&format_size(bytes)).unwrap();
            assert_eq!(parsed.as_bytes_u64(), bytes, "{}", format_size(bytes));
        }
        for raw in ["7days", "1h 30m", "500ms", "unlimited"] {
            let expiry = parse_message_expiry(raw).unwrap();
            assert_eq!(
                parse_message_expiry(&format_message_expiry(expiry)).unwrap(),
                expiry,
                "{raw}"
            );
        }
    }

    fn stored_entry(stored: &mut ResourceOptions, key: &str, options: &TopicCreateOptions) {
        let wire = options.to_option_map().unwrap();
        let header = HeaderKey::from_str(key).unwrap();
        let mut entry: OptionValue = wire.get(&header).cloned().unwrap();
        entry.explicit = true;
        stored.insert(header, entry);
    }

    #[test]
    fn test_view_reports_values_and_sources() {
        let create = TopicCreateOptions {
            segment_size: Some(IggyByteSize::from(16_u64 << 20)),
            durability: Durability::Persisted,
            messages_required_to_save: Some(1024),
            ..Default::default()
        };
        let mut stored = ResourceOptions::new();
        stored_entry(&mut stored, keys::DURABILITY, &create);
        stored_entry(&mut stored, keys::SEGMENT_SIZE, &create);
        stored_entry(&mut stored, keys::MESSAGES_REQUIRED_TO_SAVE, &create);
        let header = HeaderKey::from_str(keys::MESSAGES_REQUIRED_TO_SAVE).unwrap();
        stored.get_mut(&header).unwrap().explicit = false;

        let view = TopicOptionsView::from_stored(&stored);

        assert_eq!(view.durability.value.as_deref(), Some("persisted"));
        assert_eq!(view.durability.source, OptionSource::Explicit);
        assert_eq!(view.segment_size.value.as_deref(), Some("16 MiB"));
        assert_eq!(view.messages_required_to_save.value, Some(1024));
        assert_eq!(view.messages_required_to_save.source, OptionSource::Default);
        assert_eq!(view.message_expiry.value, None);
        assert_eq!(view.message_expiry.source, OptionSource::Unset);
    }

    #[test]
    fn test_differences_name_each_contradicted_option() {
        let create = TopicCreateOptions {
            segment_size: Some(IggyByteSize::from(16_u64 << 20)),
            ..Default::default()
        };
        let mut stored = ResourceOptions::new();
        stored_entry(&mut stored, keys::DURABILITY, &create);
        stored_entry(&mut stored, keys::SEGMENT_SIZE, &create);
        let view = TopicOptionsView::from_stored(&stored);

        let wanted = TopicOptions {
            durability: Some(Durability::Persisted),
            segment_size: Some(IggyByteSize::from(16_u64 << 20)),
            preallocate_segments: Some(true),
            ..Default::default()
        };
        assert_eq!(
            wanted.differences(&view),
            vec![
                OptionDifference {
                    key: keys::DURABILITY,
                    wanted: "persisted".into(),
                    stored: "replicated".into(),
                },
                OptionDifference {
                    key: keys::PREALLOCATE_SEGMENTS,
                    wanted: "true".into(),
                    stored: "unset".into(),
                },
            ]
        );
        assert!(TopicOptions::default().differences(&view).is_empty());
    }
}
