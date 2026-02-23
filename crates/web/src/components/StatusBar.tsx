// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

export interface StatusBarProps {
  /** Label shown at far left (default: "[coop]") */
  label?: string;
  /** WebSocket connection status */
  wsStatus: "connecting" | "connected" | "disconnected";
  /** Host name shown next to the connection indicator */
  host?: string;
  /** PTY byte offset — shown when > 0 */
  ptyOffset?: number;
  /** Callback to toggle the inspector sidebar */
  onToggleInspector?: () => void;
  /** Whether the inspector sidebar is currently visible */
  inspectorVisible?: boolean;
}

export function StatusBar({
  label = "[coop]",
  wsStatus,
  host,
  ptyOffset,
  onToggleInspector,
  inspectorVisible,
}: StatusBarProps) {
  return (
    <div className="flex h-8 shrink-0 items-center justify-between border-t border-[#333] bg-[#1a1a1a] px-3 font-mono text-[13px] text-zinc-300">
      <span className="flex items-center gap-2.5">
        <span className="font-bold tracking-wide text-teal-400">{label}</span>
      </span>
      <span className="flex items-center gap-2.5">
        {ptyOffset != null && ptyOffset > 0 && (
          <span className="text-xs text-zinc-600">offset: {ptyOffset}</span>
        )}
        <span className="flex items-center">
          <span
            className={`mr-1.5 inline-block h-2 w-2 rounded-full ${
              wsStatus === "connected"
                ? "bg-green-500"
                : wsStatus === "connecting"
                  ? "bg-amber-500"
                  : "bg-red-500"
            }`}
          />
          <span>
            {wsStatus === "connected"
              ? `Connected \u2014 ${host || location.host}`
              : wsStatus === "connecting"
                ? "Connecting\u2026"
                : "Disconnected"}
          </span>
        </span>
        {onToggleInspector && (
          <button
            type="button"
            className="rounded border border-zinc-600 px-2.5 py-0.5 font-mono text-[11px] text-zinc-200 hover:border-zinc-400 hover:text-white"
            onClick={onToggleInspector}
          >
            {inspectorVisible ? "Hide" : "Inspector"}
          </button>
        )}
      </span>
    </div>
  );
}
