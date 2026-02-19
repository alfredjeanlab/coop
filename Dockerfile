# SPDX-License-Identifier: BUSL-1.1
# Copyright (c) 2026 Alfred Jean LLC

FROM rust:1.92-bookworm AS builder
ARG TARGETARCH
RUN case "$TARGETARCH" in \
      arm64) HEADERS=linux-headers-arm64 ;; \
      *)     HEADERS=linux-headers-amd64 ;; \
    esac \
    && apt-get update && apt-get install -y protobuf-compiler musl-tools $HEADERS \
    && rustup target add x86_64-unknown-linux-musl \
    && rustup target add aarch64-unknown-linux-musl
ENV RUSTC_WRAPPER=""
WORKDIR /src
COPY . .
RUN case "$TARGETARCH" in \
      arm64) RUST_TARGET=aarch64-unknown-linux-musl ;; \
      *)     RUST_TARGET=x86_64-unknown-linux-musl ;; \
    esac \
    && cargo build --release --target "$RUST_TARGET" -p coop -p coopmux \
    && strip "target/$RUST_TARGET/release/coop" \
    && cp "target/$RUST_TARGET/release/coop" /coop-bin \
    && strip "target/$RUST_TARGET/release/coopmux" \
    && cp "target/$RUST_TARGET/release/coopmux" /coopmux-bin

# ---------------------------------------------------------------------------
# Installer stages: cached independently of base image changes
# ---------------------------------------------------------------------------

FROM debian:bookworm-slim AS claude-install
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
RUN curl -fsSL https://claude.ai/install.sh | bash

FROM debian:bookworm-slim AS gemini-install
RUN apt-get update && apt-get install -y --no-install-recommends nodejs npm ca-certificates \
    && rm -rf /var/lib/apt/lists/*
RUN npm install -g @google/gemini-cli

FROM debian:bookworm-slim AS claudeless-install
ARG TARGETARCH
ARG CLAUDELESS_VERSION=0.4.0
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl \
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

FROM debian:bookworm-slim AS kubectl-install
ARG TARGETARCH
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
RUN curl -fsSL "https://dl.k8s.io/release/$(curl -L -s https://dl.k8s.io/release/stable.txt)/bin/linux/${TARGETARCH}/kubectl" \
    -o /usr/local/bin/kubectl && chmod +x /usr/local/bin/kubectl

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
COPY --from=builder /coop-bin /usr/local/bin/coop
ENTRYPOINT ["coop"]

# ---------------------------------------------------------------------------
# Claudeless: coop + claudeless + scenario fixtures (for testing)
# ---------------------------------------------------------------------------
FROM base AS claudeless
COPY --from=claudeless-install /usr/local/bin/claudeless /usr/local/bin/claudeless
COPY --from=builder /coop-bin /usr/local/bin/coop
COPY crates/cli/tests/scenarios/ /scenarios/
ENTRYPOINT ["coop"]

# ---------------------------------------------------------------------------
# Agents: coop with a pre-installed agent CLI, ready to deploy
# ---------------------------------------------------------------------------

FROM base AS claude
COPY --from=claude-install /root/.local/ /root/.local/
ENV PATH="/root/.local/bin:$PATH"
COPY --from=builder /coop-bin /usr/local/bin/coop
ENTRYPOINT ["coop"]

FROM base AS gemini
RUN apt-get update && apt-get install -y --no-install-recommends nodejs \
    && rm -rf /var/lib/apt/lists/*
COPY --from=gemini-install /usr/local/lib/node_modules/ /usr/local/lib/node_modules/
RUN ln -s ../lib/node_modules/@google/gemini-cli/dist/src/cli.js /usr/local/bin/gemini
COPY --from=builder /coop-bin /usr/local/bin/coop
ENTRYPOINT ["coop"]

# ---------------------------------------------------------------------------
# Job: one-shot agent job (optional clone → run → exit)
# ---------------------------------------------------------------------------
FROM claude AS job
RUN mkdir -p /usr/local/lib/coop
COPY deploy/init.sh /usr/local/lib/coop/init.sh
COPY deploy/job.sh /usr/local/bin/coop-job
RUN chmod +x /usr/local/bin/coop-job
ENTRYPOINT ["/usr/local/bin/coop-job"]

# ---------------------------------------------------------------------------
# Mux: mux server with kubectl for launching session pods in Kubernetes
# ---------------------------------------------------------------------------
FROM base AS coopmux
COPY --from=kubectl-install /usr/local/bin/kubectl /usr/local/bin/kubectl
COPY --from=builder /coopmux-bin /usr/local/bin/coopmux
COPY deploy/k8s-launch.sh /usr/local/bin/coop-launch
RUN chmod +x /usr/local/bin/coop-launch
ENTRYPOINT ["coopmux"]
