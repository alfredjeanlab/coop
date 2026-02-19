#!/bin/sh
# SPDX-License-Identifier: BUSL-1.1
# Copyright (c) 2026 Alfred Jean LLC
#
# Shared environment setup. Source this from crew.sh and job.sh.
#
# Git credentials (set up before workspace clone):
#   COOP_INIT_SSH_KEY           - base64-encoded SSH private key (optional)
#   COOP_INIT_SSH_KNOWN_HOSTS   - known_hosts entries to append (optional)
#   COOP_INIT_GIT_TOKEN         - HTTP bearer token for git auth (optional)
#   COOP_INIT_GIT_USER          - git config user.name (optional)
#   COOP_INIT_GIT_EMAIL         - git config user.email (optional)
#
# Workspace:
#   COOP_INIT_REPO          - git repo to clone (optional, skip if pre-mounted)
#   COOP_INIT_BRANCH        - branch to checkout (default: main)
#   COOP_INIT_BASE          - base ref (default: origin/main)
#
# Agent credentials:
#   ANTHROPIC_API_KEY / CLAUDE_CODE_OAUTH_TOKEN

COOP_INIT_BRANCH="${COOP_INIT_BRANCH:-main}"
COOP_INIT_BASE="${COOP_INIT_BASE:-origin/main}"

# --- Git credentials ---

if [ -n "${COOP_INIT_SSH_KEY:-}" ]; then
  mkdir -p "$HOME/.ssh"
  chmod 700 "$HOME/.ssh"
  printf '%s' "$COOP_INIT_SSH_KEY" | base64 -d > "$HOME/.ssh/id_rsa"
  chmod 600 "$HOME/.ssh/id_rsa"
fi

if [ -n "${COOP_INIT_SSH_KNOWN_HOSTS:-}" ]; then
  mkdir -p "$HOME/.ssh"
  printf '%s\n' "$COOP_INIT_SSH_KNOWN_HOSTS" >> "$HOME/.ssh/known_hosts"
fi

if [ -n "${COOP_INIT_GIT_TOKEN:-}" ]; then
  git config --global credential.helper store
  HOST=$(printf '%s' "${COOP_INIT_REPO:-https://github.com}" | grep -oE '[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}' | head -1)
  printf 'https://oauth2:%s@%s\n' "$COOP_INIT_GIT_TOKEN" "${HOST:-github.com}" >> "$HOME/.git-credentials"
fi

if [ -n "${COOP_INIT_GIT_USER:-}" ]; then
  git config --global user.name "$COOP_INIT_GIT_USER"
fi

if [ -n "${COOP_INIT_GIT_EMAIL:-}" ]; then
  git config --global user.email "$COOP_INIT_GIT_EMAIL"
fi

# --- Workspace ---

if [ -n "${COOP_INIT_REPO:-}" ]; then
  git clone "$COOP_INIT_REPO" /workspace
  cd /workspace
  git checkout -b "$COOP_INIT_BRANCH" "$COOP_INIT_BASE"
else
  cd /workspace
fi

# --- Skip onboarding ---

if [ -n "${CLAUDE_CODE_OAUTH_TOKEN:-}" ] || [ -n "${ANTHROPIC_API_KEY:-}" ]; then
  VER=$(claude --version 2>/dev/null | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1)
  VER=${VER:-0.0.0}
  CWD=$(pwd)
  printf '{"hasCompletedOnboarding":true,"lastOnboardingVersion":"%s","projects":{"%s":{"hasTrustDialogAccepted":true,"allowedTools":[]}}}\n' \
    "$VER" "$CWD" > "$HOME/.claude.json"
fi
