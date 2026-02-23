// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

import { useState } from "react";
import { AgentBadge } from "@/components/AgentBadge";
import { TerminalPreview } from "@/components/TerminalPreview";
import type { MuxMetadata, SessionInfo } from "@/lib/types";
import { LaunchDialog } from "@/roots/mux/LaunchDialog";

const KNOWN_METADATA_KEYS = new Set(["agent", "k8s"]);

export function flattenLabels(
  metadata: MuxMetadata | null | undefined,
): { key: string; value: string }[] {
  if (!metadata) return [];
  const result: { key: string; value: string }[] = [];

  function walk(obj: Record<string, unknown>, prefix: string) {
    for (const [k, v] of Object.entries(obj)) {
      if (v == null) continue;
      const fullKey = prefix ? `${prefix}.${k}` : k;
      if (typeof v === "object" && !Array.isArray(v)) {
        walk(v as Record<string, unknown>, fullKey);
      } else {
        result.push({ key: fullKey, value: String(v) });
      }
    }
  }

  for (const [k, v] of Object.entries(metadata)) {
    if (KNOWN_METADATA_KEYS.has(k) || v == null) continue;
    if (typeof v === "object" && !Array.isArray(v)) {
      walk(v as Record<string, unknown>, k);
    } else {
      result.push({ key: k, value: String(v) });
    }
  }

  return result;
}

export function sessionTitle(info: SessionInfo): string {
  if (info.metadata?.k8s?.pod) return info.metadata.k8s.pod;
  if (info.url) {
    try {
      return new URL(info.url).host;
    } catch {
      /* fallback */
    }
  }
  return info.id.substring(0, 12);
}

export function sessionSubtitle(info: SessionInfo): string {
  const shortId = info.id.substring(0, 8);
  if (info.metadata?.k8s?.namespace) {
    return `${info.metadata.k8s.namespace} \u00b7 ${shortId}`;
  }
  return shortId;
}

export function formatLabels(metadata: MuxMetadata | null | undefined): string {
  return flattenLabels(metadata)
    .map((l) => `${l.key}: ${l.value}`)
    .join(" \u00b7 ");
}

export function Tile({
  info,
  focused,
  onToggleExpand,
}: {
  info: SessionInfo;
  focused: boolean;
  onToggleExpand: () => void;
}) {
  const title = sessionTitle(info);
  const subtitle = sessionSubtitle(info);

  return (
    // biome-ignore lint/a11y/useSemanticElements: card contains block-level children incompatible with <button>
    <div
      role="button"
      tabIndex={0}
      className={`flex flex-col overflow-hidden rounded-lg border bg-[#1e1e1e] transition-[border-color,background-color] duration-150 h-[280px] ${focused ? "border-blue-500" : "border-[#21262d] hover:border-[#444c56]"} cursor-pointer select-none hover:bg-[#242424]`}
      onClick={onToggleExpand}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") onToggleExpand();
      }}
    >
      {/* Header */}
      <div className="flex shrink-0 items-center justify-between gap-2 border-b border-[#21262d] px-3 py-1.5">
        <div className="flex min-w-0 items-center gap-2">
          <span className="truncate font-mono text-[13px] font-semibold">{title}</span>
          <span className="truncate text-[11px] text-zinc-500">{subtitle}</span>
        </div>
        <div className="flex shrink-0 items-center gap-1.5">
          {info.credAlert && (
            <span className="text-xs text-red-400" title="Credential issue">
              &#9888; auth
            </span>
          )}
          <AgentBadge state={info.state} />
        </div>
      </div>

      <TerminalPreview lastScreenLines={info.lastScreenLines} sourceCols={info.sourceCols} />
    </div>
  );
}

export function LaunchCard() {
  const [showDialog, setShowDialog] = useState(false);

  function handleClick() {
    setShowDialog(true);
  }

  return (
    <>
      <button
        type="button"
        className="flex h-[280px] cursor-pointer items-center justify-center rounded-lg border border-dashed border-[#21262d] text-zinc-500 transition-colors hover:border-[#444c56] hover:text-blue-400"
        onClick={handleClick}
        title="Launch new session"
      >
        <span className="text-3xl">+</span>
      </button>

      {showDialog && <LaunchDialog onClose={() => setShowDialog(false)} />}
    </>
  );
}
