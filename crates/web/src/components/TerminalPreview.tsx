// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

import { useEffect, useRef } from "react";
import { renderAnsiPre } from "@/lib/ansi-render";
import { MONO_FONT, PREVIEW_FONT_SIZE, THEME } from "@/lib/constants";

interface TerminalPreviewProps {
  /** Cached screen lines (ANSI-escaped) to render. */
  lastScreenLines: string[] | null;
  sourceCols: number;
}

/** Read-only, non-interactive terminal preview anchored to the bottom. */
export function TerminalPreview({ lastScreenLines, sourceCols }: TerminalPreviewProps) {
  const containerRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    while (el.firstChild) el.removeChild(el.firstChild);
    if (!lastScreenLines) return;
    const pre = renderAnsiPre(lastScreenLines, {
      fontSize: PREVIEW_FONT_SIZE,
      background: THEME.background,
    });
    Object.assign(pre.style, {
      position: "absolute",
      bottom: "0",
      left: "0",
      padding: "2px 4px",
      width: `${sourceCols}ch`,
      overflow: "hidden",
    });
    el.appendChild(pre);
  }, [lastScreenLines, sourceCols]);

  return (
    <div
      ref={containerRef}
      className="pointer-events-none relative flex-1 overflow-hidden"
      style={{ fontFamily: MONO_FONT }}
    />
  );
}
