#!/bin/sh
# SPDX-License-Identifier: BUSL-1.1
# Copyright (c) 2026 Alfred Jean LLC
#
# Entrypoint for a one-shot agent job. Runs a task and exits.
#
# Agent:
#   COOP_JOB_PROMPT         - task for the agent (required)
#   COOP_JOB_PRIME          - shell script for context injection (optional)
#
# Lifecycle hooks (shell commands):
#   COOP_JOB_ON_DONE        - run on success (optional)
#   COOP_JOB_ON_FAIL        - run on failure (optional)
#
# Workspace / credentials: see init.sh

set -eu

. /usr/local/lib/coop/init.sh

COOP_PORT="${COOP_PORT:-8080}"

die() {
  echo "ERROR: $1" >&2
  [ -n "${COOP_JOB_ON_FAIL:-}" ] && eval "$COOP_JOB_ON_FAIL" || true
  exit 1
}

# --- Agent config ---

AGENT_CONFIG=$(mktemp /tmp/agent.XXXXXX.json)
if [ -n "${COOP_JOB_PRIME:-}" ]; then
  printf '{"start":{"shell":"%s"}}' "$(printf '%s' "$COOP_JOB_PRIME" | sed 's/\\/\\\\/g; s/"/\\"/g')" > "$AGENT_CONFIG"
else
  printf '{}' > "$AGENT_CONFIG"
fi

# --- Start coop ---

coop --port "$COOP_PORT" --agent-config "$AGENT_CONFIG" -- claude --dangerously-skip-permissions &
COOP_PID=$!
trap 'kill $COOP_PID 2>/dev/null || true' EXIT

agent_state() {
  curl -sf "localhost:$COOP_PORT/api/v1/agent" | jq -r '.state' 2>/dev/null || echo "unknown"
}

# Wait for initial idle (setup complete)
echo "Waiting for agent..."
until [ "$(agent_state)" = "idle" ]; do sleep 1; done

# --- Deliver prompt ---

curl -sf -X POST "localhost:$COOP_PORT/api/v1/agent/nudge" \
  -H "Content-Type: application/json" \
  -d "{\"message\": $(printf '%s' "$COOP_JOB_PROMPT" | jq -Rs .)}"

# Wait for working, then idle (task complete)
until [ "$(agent_state)" = "working" ]; do sleep 1; done
until [ "$(agent_state)" = "idle" ]; do
  state=$(agent_state)
  [ "$state" = "error" ]  && die "agent errored"
  [ "$state" = "exited" ] && die "agent exited unexpectedly"
  sleep 1
done

kill $COOP_PID 2>/dev/null || true
trap - EXIT

# --- Done ---

[ -n "${COOP_JOB_ON_DONE:-}" ] && eval "$COOP_JOB_ON_DONE" || true
