# SPDX-License-Identifier: BUSL-1.1
# Copyright (c) 2026 Alfred Jean LLC

FROM rust:1.92-bookworm AS builder
ARG TARGETARCH=amd64
RUN apt-get update && apt-get install -y protobuf-compiler libssl-dev pkg-config
ENV RUSTC_WRAPPER=""
WORKDIR /src
COPY . .
RUN cargo build --release \
    && strip target/release/coop \
    && cp target/release/coop /coop-bin

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates libssl3 \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd -g 65534 nonroot && useradd -u 65534 -g nonroot -s /bin/false nonroot
COPY --from=builder /coop-bin /coop
USER 65534:65534
ENTRYPOINT ["/coop"]

# ---------------------------------------------------------------------------
# Test stage: debian + claudeless + coop binary + scenario fixtures
# ---------------------------------------------------------------------------
FROM debian:bookworm-slim AS test
ARG TARGETARCH=amd64
ARG CLAUDELESS_VERSION=0.2.5
RUN apt-get update && apt-get install -y --no-install-recommends curl ca-certificates libssl3 \
    && rm -rf /var/lib/apt/lists/*
RUN case "$TARGETARCH" in \
      arm64) ARCH=aarch64 ;; \
      *)     ARCH=x86_64 ;; \
    esac \
    && curl -fsSL "https://github.com/alfredjeanlab/claudeless/releases/download/v${CLAUDELESS_VERSION}/claudeless-linux-${ARCH}.tar.gz" \
       -o /tmp/claudeless.tar.gz \
    && tar -xzf /tmp/claudeless.tar.gz -C /usr/local/bin \
    && rm /tmp/claudeless.tar.gz \
    && chmod +x /usr/local/bin/claudeless
COPY --from=builder /coop-bin /usr/local/bin/coop
COPY crates/cli/tests/scenarios/ /scenarios/
ENTRYPOINT ["coop"]
