import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import { Terminal as XTerm } from "@xterm/xterm";
import { type RefObject, useEffect, useRef, useState } from "react";
import { InspectorSidebar, type WsEventListener } from "@/components/inspector/InspectorSidebar";
import { StreamAlert } from "@/components/StreamAlert";
import { Terminal } from "@/components/Terminal";
import { TerminalLayout } from "@/components/TerminalLayout";
import { formatLabels, sessionSubtitle, sessionTitle } from "@/components/Tile";
import { type ConnectionStatus, WsRpc } from "@/hooks/useWebSocket";
import { useInit } from "@/hooks/utils";
import { renderAnsiPre } from "@/lib/ansi-render";
import { b64decode, textToB64 } from "@/lib/base64";
import { EXPANDED_FONT_SIZE, MONO_FONT, THEME } from "@/lib/constants";
import { ReplayGate } from "@/lib/replay-gate";
import type { PromptContext, SessionInfo, WsMessage } from "@/lib/types";
import { WsMessageHarness } from "@/lib/ws-harness";

interface ExpandedSessionProps {
  info: SessionInfo;
  sidebarWidth: number;
  muxSend: (msg: unknown) => void;
  sendInputRef: RefObject<((text: string) => void) | null>;
  onTransition: (
    sessionId: string,
    next: string,
    prompt: PromptContext | null,
    lastMessage: string | null,
  ) => void;
  onClose: () => void;
}

export function ExpandedSession({
  info,
  sidebarWidth,
  muxSend,
  sendInputRef,
  onTransition,
  onClose,
}: ExpandedSessionProps) {
  const wsRef = useRef<WebSocket | null>(null);
  const rpcRef = useRef<WsRpc | null>(null);
  const gateRef = useRef(new ReplayGate());
  const harnessRef = useRef(new WsMessageHarness());
  const lastDiagnoseCountRef = useRef(0);
  const diagnoseTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const [wsStatus, setWsStatus] = useState<ConnectionStatus>("disconnected");
  const [ready, setReady] = useState(false);
  const [prompt, setPrompt] = useState<PromptContext | null>(null);
  const [lastMessage, setLastMessage] = useState<string | null>(null);
  const [showStreamAlert, setShowStreamAlert] = useState<string | null>(null);
  const wsListenersRef = useRef(new Set<WsEventListener>());
  const onTransitionRef = useRef(onTransition);
  onTransitionRef.current = onTransition;

  // Stable XTerm + FitAddon, created once on mount
  const [{ term, fit }] = useState(() => {
    const t = new XTerm({
      scrollback: 10000,
      fontSize: EXPANDED_FONT_SIZE,
      fontFamily: MONO_FONT,
      theme: THEME,
      cursorBlink: false,
      cursorInactiveStyle: "none",
      disableStdin: true,
      convertEol: false,
    });
    const f = new FitAddon();
    t.loadAddon(f);
    return { term: t, fit: f };
  });

  const cleanupRef = useRef<(() => void) | null>(null);

  // Initialize handlers (runs once during first render)
  useInit(() => {
    (window as unknown as Record<string, unknown>).__wsHarness = harnessRef.current;
    info.term = term;
    info.fit = fit;

    const dataDisp = term.onData((data) => {
      if (wsRef.current?.readyState === WebSocket.OPEN) {
        wsRef.current.send(JSON.stringify({ event: "input:send:raw", data: textToB64(data) }));
      } else {
        muxSend({ event: "input:send", session: info.id, text: data });
      }
    });

    const resizeDisp = term.onResize(({ cols, rows }) => {
      if (wsRef.current?.readyState === WebSocket.OPEN) {
        wsRef.current.send(JSON.stringify({ event: "resize", cols, rows }));
      }
    });

    sendInputRef.current = (text: string) => {
      if (wsRef.current?.readyState === WebSocket.OPEN) {
        wsRef.current.send(JSON.stringify({ event: "input:send:raw", data: textToB64(text) }));
      } else {
        muxSend({ event: "input:send", session: info.id, text });
      }
    };

    cleanupRef.current = () => {
      (window as unknown as Record<string, unknown>).__wsHarness = undefined;
      dataDisp.dispose();
      resizeDisp.dispose();
      rpcRef.current?.dispose();
      rpcRef.current = null;
      wsRef.current?.close();
      wsRef.current = null;
      term.dispose();
      info.term = null;
      info.fit = null;
      sendInputRef.current = null;
    };
  });

  // Cleanup on unmount
  useEffect(() => () => cleanupRef.current?.(), []);

  function scheduleDiagnose() {
    if (diagnoseTimerRef.current !== null) return;
    diagnoseTimerRef.current = setTimeout(() => {
      diagnoseTimerRef.current = null;
      const issues = harnessRef.current.diagnose();
      if (issues.length > lastDiagnoseCountRef.current) {
        lastDiagnoseCountRef.current = issues.length;
        setShowStreamAlert(issues[0].message);
        setTimeout(() => setShowStreamAlert(null), 10_000);
      }
    }, 2_000);
  }

  function connectWs() {
    gateRef.current.reset();
    harnessRef.current.reconnect();
    lastDiagnoseCountRef.current = 0;
    setShowStreamAlert(null);
    const proto = location.protocol === "https:" ? "wss:" : "ws:";
    const params = new URLSearchParams(location.search);
    let url = `${proto}//${location.host}/ws/${info.id}?subscribe=pty,state,usage,hooks`;
    const token = params.get("token");
    if (token) url += `&token=${encodeURIComponent(token)}`;

    setWsStatus("connecting");
    const ws = new WebSocket(url);
    wsRef.current = ws;
    const rpc = new WsRpc(ws);
    rpcRef.current = rpc;

    ws.onopen = () => {
      setWsStatus("connected");
      // Resize before replay so the PTY dimensions match XTerm when the
      // ring buffer snapshot is captured. WS messages are ordered, so the
      // server processes resize before replay:get — no need to await.
      ws.send(JSON.stringify({ event: "resize", cols: term.cols, rows: term.rows }));
      ws.send(JSON.stringify({ event: "replay:get", offset: 0 }));

      rpc.request({ event: "agent:get" }).then((res) => {
        if (res.ok && res.json) {
          const a = res.json as { state?: string; prompt?: PromptContext; last_message?: string };
          setPrompt(a.prompt ?? null);
          setLastMessage(a.last_message ?? null);
          if (a.state) {
            onTransitionRef.current(info.id, a.state, a.prompt ?? null, a.last_message ?? null);
          }
        }
      });
    };

    ws.onmessage = (evt) => {
      try {
        const msg = JSON.parse(evt.data);
        if (rpc.handleMessage(msg)) return;

        for (const fn of wsListenersRef.current) fn(msg as WsMessage);

        if (msg.event === "replay") {
          const bytes = b64decode(msg.data);
          harnessRef.current.replay(bytes, msg.offset, msg.next_offset);
          const action = gateRef.current.onReplay(bytes.length, msg.offset, msg.next_offset);
          if (!action) return;
          if (action.isFirst) {
            term.reset();
            handleReplayReady(term, setReady);
          }
          term.write(action.skip > 0 ? bytes.subarray(action.skip) : bytes);
          scheduleDiagnose();
        } else if (msg.event === "pty") {
          const bytes = b64decode(msg.data);
          harnessRef.current.pty(bytes, msg.offset);
          const skip = gateRef.current.onPty(bytes.length, msg.offset);
          if (skip === null) return;
          term.write(skip > 0 ? bytes.subarray(skip) : bytes);
          scheduleDiagnose();
        } else if (msg.event === "transition") {
          setPrompt(msg.prompt ?? null);
          setLastMessage(msg.last_message ?? null);
          onTransitionRef.current(info.id, msg.next, msg.prompt ?? null, msg.last_message ?? null);
        }
      } catch {
        // ignore parse errors
      }
    };

    ws.onclose = () => {
      wsRef.current = null;
      rpc.dispose();
      rpcRef.current = null;
      setWsStatus("disconnected");
    };
  }

  function handleReady() {
    try {
      const webgl = new WebglAddon();
      webgl.onContextLoss(() => webgl.dispose());
      term.loadAddon(webgl);
    } catch {
      // canvas fallback
    }
    fit.fit();
    connectWs();
  }

  function subscribeWsEvents(listener: WsEventListener) {
    wsListenersRef.current.add(listener);
    return () => {
      wsListenersRef.current.delete(listener);
    };
  }

  function wsSend(msg: unknown) {
    if (wsRef.current?.readyState === WebSocket.OPEN) {
      wsRef.current.send(JSON.stringify(msg));
    }
  }

  function wsRequest(msg: Record<string, unknown>) {
    const rpc = rpcRef.current;
    if (!rpc) {
      return Promise.resolve({ ok: false, status: 0, json: null, text: "Not connected" } as const);
    }
    return rpc.request(msg);
  }

  function handleTerminalFocus() {
    term.focus();
  }

  return (
    <TerminalLayout
      className="absolute inset-y-0 right-0 z-[100] transition-[left] duration-200"
      style={{ left: sidebarWidth }}
      title={sessionTitle(info)}
      subtitle={[sessionSubtitle(info), formatLabels(info.metadata)]
        .filter(Boolean)
        .join(" \u00b7 ")}
      credAlert={info.credAlert}
      headerRight={
        <button
          type="button"
          className="border-none bg-transparent p-1 text-zinc-500 hover:text-zinc-300"
          title="Close (Esc)"
          onClick={onClose}
        >
          <svg
            width="18"
            height="18"
            viewBox="0 0 18 18"
            fill="none"
            stroke="currentColor"
            strokeWidth="2"
            strokeLinecap="round"
          >
            <title>Close</title>
            <line x1="4" y1="4" x2="14" y2="14" />
            <line x1="14" y1="4" x2="4" y2="14" />
          </svg>
        </button>
      }
      wsStatus={wsStatus}
      agentState={info.state}
      statusLabel="[coopmux]"
      onInteraction={handleTerminalFocus}
      inspector={
        <InspectorSidebar
          subscribeWsEvents={subscribeWsEvents}
          prompt={prompt}
          lastMessage={lastMessage}
          wsSend={wsSend}
          wsRequest={wsRequest}
          onTabClick={handleTerminalFocus}
        />
      }
    >
      {showStreamAlert && (
        <StreamAlert message={showStreamAlert} onDismiss={() => setShowStreamAlert(null)} />
      )}
      <div className="relative min-w-0 flex-1">
        <Terminal
          instance={term}
          fitAddon={fit}
          onReady={handleReady}
          theme={THEME}
          className={`h-full py-4 pl-4 ${ready ? "" : "invisible"}`}
        />
        {!ready && (
          <div
            className="absolute inset-0 overflow-hidden"
            style={{ background: THEME.background }}
          >
            {/* Loading bar — absolutely positioned so it doesn't shift the preview */}
            <div className="absolute top-0 left-0 right-0 h-0.5 overflow-hidden bg-zinc-800">
              <div
                className="absolute inset-y-0 w-1/3 animate-[shimmer_1.5s_ease-in-out_infinite] bg-blue-500/60"
                style={{ animation: "shimmer 1.5s ease-in-out infinite" }}
              />
              <style>{`@keyframes shimmer { 0% { left: -33% } 100% { left: 100% } }`}</style>
            </div>
            {/* Cached screen preview */}
            <div className="mr-[14px] overflow-hidden py-4 pl-4">
              {info.lastScreenLines ? (
                <LoadingPreview lines={info.lastScreenLines} />
              ) : (
                <div
                  className="flex items-center gap-2 text-sm text-zinc-500"
                  style={{ fontFamily: MONO_FONT }}
                >
                  Loading session&hellip;
                </div>
              )}
            </div>
          </div>
        )}
      </div>
    </TerminalLayout>
  );
}

/** Render cached screen lines using the shared ANSI renderer. */
function LoadingPreview({ lines }: { lines: string[] }) {
  return (
    <div
      ref={(el) => {
        if (el && !el.firstChild) {
          el.appendChild(renderAnsiPre(lines, { fontSize: EXPANDED_FONT_SIZE }));
        }
      }}
    />
  );
}

/** Enable input and focus the terminal after replay completes.
 *  Focus is deferred to the next animation frame so React can first
 *  render the terminal visible (removing the `invisible` CSS class). */
export function handleReplayReady(
  term: { options: { disableStdin?: boolean }; focus: () => void },
  setReady: (v: boolean) => void,
) {
  term.options.disableStdin = false;
  setReady(true);
  requestAnimationFrame(() => term.focus());
}
