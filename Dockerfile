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
    && cp "target/$RUST_TARGET/release/coop" /coop-bin \
    && strip "target/$RUST_TARGET/release/coopmux" \
    && cp "target/$RUST_TARGET/release/coopmux" /coopmux-bin

# ---------------------------------------------------------------------------
# Base: common developer tools shared by all runtime stages
# ---------------------------------------------------------------------------
FROM debian:bookworm-slim AS base
RUN apt-get update && apt-get install -y --no-install-recommends \
    git python3 build-essential openssh-client \
    jq ripgrep fd-find tree \
    ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*

# ---------------------------------------------------------------------------
# Empty: coop binary with common developer tools
# ---------------------------------------------------------------------------
FROM base AS empty
COPY --from=builder /coop-bin /coop
ENTRYPOINT ["/coop"]

# ---------------------------------------------------------------------------
# Claudeless: coop + claudeless + scenario fixtures (for testing)
# ---------------------------------------------------------------------------
FROM base AS claudeless
ARG TARGETARCH
ARG CLAUDELESS_VERSION=0.3.0
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

FROM base AS claude
RUN curl -fsSL https://claude.ai/install.sh | bash
ENV PATH="/root/.local/bin:$PATH"
COPY --from=builder /coop-bin /usr/local/bin/coop
ENTRYPOINT ["coop"]

FROM base AS gemini
RUN apt-get update && apt-get install -y --no-install-recommends nodejs npm \
    && rm -rf /var/lib/apt/lists/*
RUN npm install -g @google/gemini-cli
COPY --from=builder /coop-bin /usr/local/bin/coop
ENTRYPOINT ["coop"]

# ---------------------------------------------------------------------------
# Coopmux: mux server with kubectl for launching session pods in Kubernetes
# ---------------------------------------------------------------------------
FROM base AS coopmux
ARG TARGETARCH
RUN curl -fsSL "https://dl.k8s.io/release/$(curl -L -s https://dl.k8s.io/release/stable.txt)/bin/linux/${TARGETARCH}/kubectl" \
    -o /usr/local/bin/kubectl && chmod +x /usr/local/bin/kubectl
COPY --from=builder /coopmux-bin /usr/local/bin/coopmux
COPY deploy/k8s-launch.sh /usr/local/bin/coop-launch
RUN chmod +x /usr/local/bin/coop-launch
ENTRYPOINT ["coopmux"]
