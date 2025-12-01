//! Fuzz testing for validation functions.
//!
//! This fuzz target tests the robustness of the validation module against
//! arbitrary input strings. It ensures that validation functions:
//!
//! - Never panic on any input
//! - Always return a valid Result (Ok or Err)
//! - Handle edge cases like empty strings, long strings, and special characters
//!
//! # Running the Fuzz Tests
//!
//! ```bash
//! # Install cargo-fuzz (requires nightly)
//! cargo +nightly install cargo-fuzz
//!
//! # Run the validation fuzz target
//! cargo +nightly fuzz run fuzz_validation
//!
//! # Run with a time limit (e.g., 60 seconds)
//! cargo +nightly fuzz run fuzz_validation -- -max_total_time=60
//!
//! # View coverage
//! cargo +nightly fuzz coverage fuzz_validation
//! ```
//!
//! # What This Tests
//!
//! - `validate_resource_name`: Stream and topic name validation
//! - `validate_event_type`: Event type string validation
//! - `validate_partition_count`: Numeric partition count validation
//! - `validate_partition_id`: Partition ID validation
//! - `validate_consumer_id`: Consumer ID validation

#![no_main]

use libfuzzer_sys::fuzz_target;
use iggy_sample::validation::{
    validate_consumer_id,
    validate_event_type,
    validate_partition_count,
    validate_partition_id,
    validate_resource_name,
};

fuzz_target!(|data: &[u8]| {
    // Try to interpret the bytes as a UTF-8 string for string validation
    if let Ok(s) = std::str::from_utf8(data) {
        // Test resource name validation (shouldn't panic)
        let _ = validate_resource_name(s, "Stream");
        let _ = validate_resource_name(s, "Topic");

        // Test event type validation (shouldn't panic)
        let _ = validate_event_type(s);
    }

    // Test numeric validation with bytes interpreted as u32
    // This tests boundary conditions and all possible u32 values
    if data.len() >= 4 {
        let value = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);

        // Test partition count validation (shouldn't panic)
        let _ = validate_partition_count(value, "Topic");

        // Test partition ID validation (shouldn't panic)
        let _ = validate_partition_id(value);

        // Test consumer ID validation (shouldn't panic)
        let _ = validate_consumer_id(value);
    }
});
