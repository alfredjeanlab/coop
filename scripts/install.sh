#!/bin/bash
# SPDX-License-Identifier: BUSL-1.1
# Copyright (c) 2026 Alfred Jean LLC
#
# Curl-pipe installer for coop:
#   curl -fsSL https://github.com/alfredjeanlab/coop/releases/latest/download/install.sh | bash
#
# Environment variables:
#   COOP_VERSION  - version to install (default: latest)
#   COOP_INSTALL  - install directory (default: ~/.local/bin)

set -euo pipefail

REPO="alfredjeanlab/coop"
INSTALL_DIR="${COOP_INSTALL:-$HOME/.local/bin}"

# --- Detect platform ---

OS="$(uname -s)"
case "$OS" in
    Linux)  OS_TAG="linux" ;;
    Darwin) OS_TAG="macos" ;;
    *)
        echo "Error: unsupported OS: $OS" >&2
        exit 1
        ;;
esac

ARCH="$(uname -m)"
case "$ARCH" in
    x86_64|amd64)  ARCH_TAG="x86_64" ;;
    aarch64|arm64) ARCH_TAG="aarch64" ;;
    *)
        echo "Error: unsupported architecture: $ARCH" >&2
        exit 1
        ;;
esac

# --- Resolve version ---

if [ -n "${COOP_VERSION:-}" ]; then
    VERSION="$COOP_VERSION"
else
    VERSION="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" | grep '"tag_name"' | sed -E 's/.*"tag_name": *"([^"]+)".*/\1/')"
    if [ -z "$VERSION" ]; then
        echo "Error: failed to resolve latest version" >&2
        exit 1
    fi
fi

TARBALL="coop-${OS_TAG}-${ARCH_TAG}.tar.gz"
BASE_URL="https://github.com/${REPO}/releases/download/${VERSION}"

echo "Installing coop ${VERSION} (${OS_TAG}/${ARCH_TAG})..."

# --- Download and verify ---

TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

curl -fsSL "${BASE_URL}/${TARBALL}" -o "${TMPDIR}/${TARBALL}"
curl -fsSL "${BASE_URL}/SHA256SUMS" -o "${TMPDIR}/SHA256SUMS"

cd "$TMPDIR"
if command -v sha256sum >/dev/null 2>&1; then
    grep "$TARBALL" SHA256SUMS | sha256sum -c --quiet
elif command -v shasum >/dev/null 2>&1; then
    grep "$TARBALL" SHA256SUMS | shasum -a 256 -c --quiet
else
    echo "Warning: no sha256sum or shasum found, skipping checksum verification" >&2
fi

# --- Install ---

mkdir -p "$INSTALL_DIR"
tar -xzf "$TARBALL" -C "$INSTALL_DIR"

# Re-sign binaries on macOS (required after copying adhoc-signed binaries)
if [ "$OS_TAG" = "macos" ] && command -v codesign >/dev/null 2>&1; then
    codesign --force --sign - "${INSTALL_DIR}/coop"
    codesign --force --sign - "${INSTALL_DIR}/coopmux"
fi

echo "Installed coop and coopmux to ${INSTALL_DIR}"

# --- PATH check ---

case ":${PATH}:" in
    *":${INSTALL_DIR}:"*) ;;
    *)
        echo ""
        echo "Warning: ${INSTALL_DIR} is not in your PATH."
        echo "Add it with:  export PATH=\"${INSTALL_DIR}:\$PATH\""
        ;;
esac
