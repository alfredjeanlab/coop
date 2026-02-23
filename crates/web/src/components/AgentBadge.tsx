// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

const stateStyles: Record<string, string> = {
  idle: "bg-blue-500/20 text-blue-400",
  waiting_for_input: "bg-blue-500/20 text-blue-400",
  working: "bg-green-500/20 text-green-400",
  permission_prompt: "bg-yellow-500/20 text-yellow-500",
  plan_prompt: "bg-yellow-500/20 text-yellow-500",
  setup_prompt: "bg-purple-500/20 text-purple-400",
  question_prompt: "bg-yellow-500/20 text-yellow-500",
  error: "bg-red-500/20 text-red-400",
  parked: "bg-red-500/20 text-red-400",
  restarting: "bg-blue-500/20 text-blue-400",
  exited: "bg-zinc-700 text-zinc-400",
  starting: "bg-blue-500/20 text-blue-400",
  unknown: "bg-zinc-700 text-zinc-400",
};

export function badgeClassForState(state: string | null | undefined): string {
  if (!state) return stateStyles.unknown;
  const s = state.toLowerCase();
  if (s === "idle" || s === "waiting_for_input") return stateStyles.idle;
  if (s === "working") return stateStyles.working;
  if (s.includes("prompt")) return stateStyles.permission_prompt;
  if (s.includes("error") || s === "parked") return stateStyles.error;
  if (s === "exited") return stateStyles.exited;
  if (s === "starting" || s === "restarting") return stateStyles.starting;
  return stateStyles.unknown;
}

interface AgentBadgeProps {
  state: string | null | undefined;
  className?: string;
}

export function AgentBadge({ state, className }: AgentBadgeProps) {
  return (
    <span
      className={`inline-block rounded-full px-2 py-0.5 text-[11px] font-medium uppercase whitespace-nowrap ${badgeClassForState(state)} ${className || ""}`}
    >
      {state || "unknown"}
    </span>
  );
}
