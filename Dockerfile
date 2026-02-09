# SPDX-License-Identifier: BUSL-1.1
# Copyright (c) 2026 Alfred Jean LLC

FROM rust:1.92-bookworm AS builder
ARG TARGETARCH
RUN apt-get update && apt-get install -y protobuf-compiler musl-tools \
    && rustup target add x86_64-unknown-linux-musl \
    && rustup target add aarch64-unknown-linux-musl
ENV RUSTC_WRAPPER=""
WORKDIR /src
COPY . .
RUN case "$TARGETARCH" in \
      arm64) RUST_TARGET=aarch64-unknown-linux-musl ;; \
      *)     RUST_TARGET=x86_64-unknown-linux-musl ;; \
    esac \
    && cargo build --release --target "$RUST_TARGET" \
    && strip "target/$RUST_TARGET/release/coop" \
    && cp "target/$RUST_TARGET/release/coop" /coop-bin

FROM gcr.io/distroless/static-debian12:nonroot
COPY --from=builder /coop-bin /coop
ENTRYPOINT ["/coop"]

# ---------------------------------------------------------------------------
# Test stage: debian + claudeless + coop binary + scenario fixtures
# ---------------------------------------------------------------------------
FROM debian:bookworm-slim AS test
ARG TARGETARCH
ARG CLAUDELESS_VERSION=0.3.0
RUN apt-get update && apt-get install -y --no-install-recommends curl ca-certificates \
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

# ---------------------------------------------------------------------------
# Agent images: coop with a pre-installed agent CLI, ready to deploy
# ---------------------------------------------------------------------------

FROM debian:bookworm-slim AS claude
RUN apt-get update && apt-get install -y --no-install-recommends curl ca-certificates \
    && rm -rf /var/lib/apt/lists/*
RUN curl -fsSL https://claude.ai/install.sh | bash
ENV PATH="/root/.claude/local/bin:$PATH"
COPY --from=builder /coop-bin /usr/local/bin/coop
ENTRYPOINT ["coop"]

FROM debian:bookworm-slim AS gemini
RUN apt-get update && apt-get install -y --no-install-recommends curl ca-certificates nodejs npm \
    && rm -rf /var/lib/apt/lists/*
RUN npm install -g @google/gemini-cli
COPY --from=builder /coop-bin /usr/local/bin/coop
ENTRYPOINT ["coop"]
