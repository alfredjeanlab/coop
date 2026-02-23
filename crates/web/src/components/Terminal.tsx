// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import type { ITheme } from "@xterm/xterm";
import { Terminal as XTerm } from "@xterm/xterm";
import { forwardRef, useEffect, useImperativeHandle, useRef } from "react";
import "@xterm/xterm/css/xterm.css";
import { MONO_FONT, THEME } from "@/lib/constants";

export interface TerminalHandle {
  terminal: XTerm | null;
  fit: () => void;
}

interface TerminalProps {
  /** Externally-created terminal — skip managed creation when provided. */
  instance?: XTerm;
  /** FitAddon for ResizeObserver fitting in instance mode. */
  fitAddon?: FitAddon;
  /** Fires after instance.open() — use for replaying cached screen data. */
  onReady?: () => void;
  fontSize?: number;
  fontFamily?: string;
  theme?: ITheme;
  scrollback?: number;
  cursorBlink?: boolean;
  disableStdin?: boolean;
  className?: string;
  onData?: (data: string) => void;
  onBinary?: (data: string) => void;
  onResize?: (size: { cols: number; rows: number }) => void;
}

export const Terminal = forwardRef<TerminalHandle, TerminalProps>(function Terminal(
  {
    instance,
    fitAddon: externalFit,
    onReady,
    fontSize,
    fontFamily = MONO_FONT,
    theme,
    scrollback = 10000,
    cursorBlink = false,
    disableStdin = false,
    className,
    onData,
    onBinary,
    onResize,
  },
  ref,
) {
  const containerRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<XTerm | null>(null);
  const fitRef = useRef<FitAddon | null>(null);

  // Store callbacks in refs to avoid re-creating terminal
  const onDataRef = useRef(onData);
  onDataRef.current = onData;
  const onBinaryRef = useRef(onBinary);
  onBinaryRef.current = onBinary;
  const onResizeRef = useRef(onResize);
  onResizeRef.current = onResize;
  const onReadyRef = useRef(onReady);
  onReadyRef.current = onReady;

  useImperativeHandle(ref, () => ({
    get terminal() {
      return instance ?? termRef.current;
    },
    fit() {
      (externalFit ?? fitRef.current)?.fit();
    },
  }));

  const mountedRef = useRef(false);
  const fitAddonRef = useRef(externalFit);
  fitAddonRef.current = externalFit;

  useEffect(() => {
    if (instance == null) return;

    const el = containerRef.current;
    if (!el || mountedRef.current) return;

    // xterm.js v5's open() returns early when the terminal was already
    // opened (this.element exists).  When the same instance is reused in
    // a new React container (e.g. tile → expanded overlay), we must
    // manually re-attach the existing DOM element instead of calling
    // open() again.
    if (instance.element) {
      el.appendChild(instance.element);
      instance.refresh(0, instance.rows - 1);
    } else {
      instance.open(el);
    }
    mountedRef.current = true;
    onReadyRef.current?.();

    // Always observe resizes; the ref-based callback is a no-op when
    // no FitAddon is provided (preview mode).
    const observer = new ResizeObserver(() => {
      requestAnimationFrame(() => fitAddonRef.current?.fit());
    });
    observer.observe(el);

    return () => {
      observer.disconnect();
      mountedRef.current = false;
      // Detach xterm's DOM from this container so it can be re-attached
      // elsewhere.  Do NOT dispose — the instance is owned externally.
      while (el.firstChild) el.removeChild(el.firstChild);
    };
  }, [instance]);

  useEffect(() => {
    if (instance != null) return;

    const el = containerRef.current;
    if (!el) return;

    const term = new XTerm({
      fontSize: fontSize ?? 14,
      fontFamily,
      theme: theme ?? THEME,
      scrollback,
      cursorBlink,
      cursorInactiveStyle: "none",
      disableStdin,
      convertEol: false,
    });

    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(el);

    try {
      const webgl = new WebglAddon();
      webgl.onContextLoss(() => webgl.dispose());
      term.loadAddon(webgl);
    } catch {
      // canvas fallback
    }

    term.onData((data) => onDataRef.current?.(data));
    term.onBinary((data) => onBinaryRef.current?.(data));
    term.onResize((size) => onResizeRef.current?.(size));

    // ResizeObserver fires after layout (before paint), catching both
    // the initial container sizing and subsequent resizes.  This replaces
    // the synchronous fit() that could run before flex layout settled.
    const observer = new ResizeObserver(() => {
      requestAnimationFrame(() => fit.fit());
    });
    observer.observe(el);

    termRef.current = term;
    fitRef.current = fit;

    return () => {
      observer.disconnect();
      term.dispose();
      termRef.current = null;
      fitRef.current = null;
    };
  }, [instance, fontSize, fontFamily, theme, scrollback, cursorBlink, disableStdin]);

  return (
    <div className={className} style={{ background: (theme ?? THEME).background }}>
      <div ref={containerRef} className="h-full w-full overflow-hidden" />
    </div>
  );
});
