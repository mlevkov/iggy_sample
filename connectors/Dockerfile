# syntax=docker/dockerfile:1
#
# Iggy connector runtime with the HTTP source and HTTP sink plugins
# (ADR-0001 PR 4, pinned per ADR-0002). Build context: this directory.
#
# apache/iggy-connect ships the runtime binary only: upstream publishes no
# plugin builds. The plugins are therefore compiled here from apache/iggy at
# the release commit the image comes from. Runtime and plugins exchange
# configuration and messages across a C ABI defined by the connector SDK, so
# both must come from the same commit.
#
# The image's org.opencontainers.image.revision label names 1fb58a67 (the
# branch head when the release workflow ran), not 91e02063. Its binary is
# release code: it carries strings from 91e02063 and none from the sink
# delivery changes that came after it (verified 2026-11-08).
#
# Bumping connectors: change IGGY_COMMIT and the runtime tag together, and
# keep the build stage on Debian trixie, the runtime image's glibc.
#
# Both base images are pinned by tag and by digest (the multi-arch index
# digest, amd64 and arm64), so a re-pushed tag cannot change what is built.
# The runtime's digest is the image checked above; Dependabot does not
# cover this directory, so bump both digests by hand, with the tag:
#   docker buildx imagetools inspect <image>:<tag>   (its "Digest:" line)

ARG IGGY_COMMIT=91e0206367aad2d30259f7d45203832b08ae84e7
ARG RUNTIME_IMAGE=apache/iggy-connect:0.6.0@sha256:0b5a4c5356723f2c51d16002e9b75a9081a47c289b9e9d3c4b47e84710b5f9cc

FROM rust:1.99.1-slim-trixie@sha256:f6c4a35d21dbb3c89d7a8d056c193e94fc09037f2b58dd858b09c39a47dd02d7 AS plugins
ARG IGGY_COMMIT
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates git \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
# Fetch exactly one commit by its full hash, then confirm it: the hash is
# the integrity check for the source.
RUN git init -q iggy \
    && git -C iggy fetch -q --depth 1 https://github.com/apache/iggy.git "$IGGY_COMMIT" \
    && git -C iggy checkout -q FETCH_HEAD \
    && test "$(git -C iggy rev-parse HEAD)" = "$IGGY_COMMIT"
WORKDIR /src/iggy
# The checkout's rust-toolchain.toml names 1.99.1 plus components this
# build does not need; use the image's toolchain as is.
ENV RUSTUP_TOOLCHAIN=1.99.1
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/src/iggy/target,sharing=locked \
    cargo build --locked --release \
        -p iggy_connector_http_source \
        -p iggy_connector_http_sink \
    && mkdir -p /out \
    && cp target/release/libiggy_connector_http_source.so \
          target/release/libiggy_connector_http_sink.so /out/

FROM ${RUNTIME_IMAGE}
# The runtime image has no CA certificates (apache/iggy#4787). The HTTP
# sink's client verifies TLS against the system roots and refuses to build
# without any, even for a plain http:// URL, so the plugin's open() fails,
# and the runtime logs only "Plugin initialization failed"
# (apache/iggy#4786). curl serves the healthcheck.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
# The runtime image runs as root; this one does not.
RUN useradd --system --uid 10001 --user-group --home-dir /var/lib/iggy-connectors \
        --create-home --shell /usr/sbin/nologin connectors \
    && mkdir -p /var/lib/iggy-connectors/state \
    && chown -R connectors:connectors /var/lib/iggy-connectors \
    && chmod 0700 /var/lib/iggy-connectors /var/lib/iggy-connectors/state
COPY --from=plugins /out/ /usr/local/lib/iggy-connectors/
COPY templates/ /etc/iggy-connectors/templates/
COPY --chmod=0755 entrypoint.sh /usr/local/bin/connectors-entrypoint
COPY --chmod=0755 healthcheck.sh /usr/local/bin/connectors-healthcheck
USER connectors
WORKDIR /var/lib/iggy-connectors
# Healthy once both connectors run: a plugin that fails to open leaves the
# runtime up, so the process alone says nothing.
HEALTHCHECK --interval=5s --timeout=5s --start-period=10s --retries=12 \
    CMD ["/usr/local/bin/connectors-healthcheck"]
ENTRYPOINT ["/usr/local/bin/connectors-entrypoint"]
