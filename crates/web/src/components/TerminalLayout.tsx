// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

import { type ReactNode, useState } from "react";
import { AgentBadge } from "./AgentBadge";
import { StatusBar } from "./StatusBar";

export interface TerminalLayoutProps {
  /** Title displayed in the header bar */
  title: string;
  /** Subtitle displayed next to the title */
  subtitle?: string;
  /** Content rendered at the right end of the header */
  headerRight?: ReactNode;
  /** Show a credential alert badge in the header */
  credAlert?: boolean;

  /** WebSocket connection status */
  wsStatus: "connecting" | "connected" | "disconnected";
  /** Agent state for the header badge */
  agentState?: string | null;
  /** PTY byte offset for the status bar */
  ptyOffset?: number;
  /** Host shown in the status bar connection indicator */
  host?: string;
  /** Label shown at far left of the status bar */
  statusLabel?: string;

  /** Inspector sidebar content. Rendered in a resizable panel when visible. */
  inspector?: ReactNode;
  /** Called after sidebar interactions (toggle, resize) for e.g. terminal refocus */
  onInteraction?: () => void;

  /** Terminal content (rendered as the main area) */
  children: ReactNode;
  /** Extra classes on the root container */
  className?: string;
  /** Inline styles on the root container */
  style?: React.CSSProperties;
}

export function TerminalLayout({
  title,
  subtitle,
  headerRight,
  credAlert,
  wsStatus,
  agentState,
  ptyOffset,
  host,
  statusLabel,
  inspector,
  onInteraction,
  children,
  className,
  style,
}: TerminalLayoutProps) {
  const [sidebarVisible, setSidebarVisible] = useState(false);
  const [sidebarWidth, setSidebarWidth] = useState(450);

  function handleToggle() {
    setSidebarVisible((v) => !v);
    onInteraction?.();
  }

  function handleResize(e: React.MouseEvent) {
    e.preventDefault();
    const onMove = (ev: MouseEvent) => {
      const right = window.innerWidth - ev.clientX;
      setSidebarWidth(Math.min(600, Math.max(300, right)));
    };
    const onUp = () => {
      document.body.style.cursor = "";
      document.body.style.userSelect = "";
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
      onInteraction?.();
    };
    document.body.style.cursor = "col-resize";
    document.body.style.userSelect = "none";
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  }

  return (
    <div
      className={`flex flex-col overflow-hidden bg-[#1e1e1e] font-sans text-[#c9d1d9] ${className || ""}`}
      style={style}
    >
      {/* Header */}
      <div className="flex shrink-0 items-center justify-between gap-2 border-b border-[#333] px-3 py-1.5">
        <div className="flex min-w-0 items-center gap-2">
          <span className="truncate font-mono text-[13px] font-semibold text-zinc-200">
            {title}
          </span>
          {subtitle && (
            <span className="truncate font-mono text-[11px] text-zinc-500">{subtitle}</span>
          )}
        </div>
        <div className="flex shrink-0 items-center gap-1.5">
          {credAlert && (
            <span className="text-xs text-red-400" title="Credential issue">
              &#9888; auth
            </span>
          )}
          {agentState && <AgentBadge state={agentState} />}
          {headerRight}
        </div>
      </div>

      {/* Main area: terminal + optional sidebar */}
      <div className="flex min-h-0 flex-1">
        {children}

        {/* Resize handle */}
        {sidebarVisible && inspector && (
          <div
            aria-hidden="true"
            className="w-[5px] shrink-0 cursor-col-resize transition-colors hover:bg-blue-400"
            onMouseDown={handleResize}
          />
        )}

        {/* Inspector sidebar */}
        {sidebarVisible && inspector && (
          <div
            className="flex shrink-0 flex-col overflow-hidden border-l border-[#333] bg-[#181818] font-mono text-xs text-zinc-400"
            style={{ width: sidebarWidth }}
          >
            {inspector}
          </div>
        )}
      </div>

      {/* Status bar */}
      <StatusBar
        label={statusLabel}
        wsStatus={wsStatus}
        ptyOffset={ptyOffset}
        host={host}
        onToggleInspector={inspector ? handleToggle : undefined}
        inspectorVisible={sidebarVisible}
      />
    </div>
  );
}
