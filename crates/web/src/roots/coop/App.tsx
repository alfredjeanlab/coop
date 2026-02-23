// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

import { useEffect, useRef, useState } from "react";
import "@xterm/xterm/css/xterm.css";
import { DropOverlay } from "@/components/DropOverlay";
import { InspectorSidebar, type WsEventListener } from "@/components/inspector/InspectorSidebar";
import { OAuthToast } from "@/components/OAuthToast";
import { StreamAlert } from "@/components/StreamAlert";
import { Terminal, type TerminalHandle } from "@/components/Terminal";
import { TerminalLayout } from "@/components/TerminalLayout";
import { useFileUpload } from "@/hooks/useFileUpload";
import { useWebSocket } from "@/hooks/useWebSocket";
import { useInit, useInterval } from "@/hooks/utils";
import { b64decode, b64encode } from "@/lib/base64";
import { TERMINAL_FONT_SIZE, THEME } from "@/lib/constants";
import { ReplayGate } from "@/lib/replay-gate";
import type { PromptContext, WsMessage } from "@/lib/types";
import { WsMessageHarness } from "@/lib/ws-harness";

export function App() {
  const termRef = useRef<TerminalHandle>(null);
  const [wsStatus, setWsStatus] = useState<"connecting" | "connected" | "disconnected">(
    "connecting",
  );
  const [agentState, setAgentState] = useState<string | null>(null);
  const [prompt, setPrompt] = useState<PromptContext | null>(null);
  const [lastMessage, setLastMessage] = useState<string | null>(null);
  const [ptyOffset, setPtyOffset] = useState(0);
  const [showStreamAlert, setShowStreamAlert] = useState<string | null>(null);
  const gateRef = useRef(new ReplayGate());
  const harnessRef = useRef(new WsMessageHarness());
  const lastDiagnoseCountRef = useRef(0);
  const diagnoseTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  useInit(() => {
    (window as any).__wsHarness = harnessRef.current;
  });

  const wsListenersRef = useRef(new Set<WsEventListener>());

  function subscribeWsEvents(listener: WsEventListener) {
    wsListenersRef.current.add(listener);
    return () => {
      wsListenersRef.current.delete(listener);
    };
  }

  function onMessage(raw: unknown) {
    const msg = raw as WsMessage;

    // Notify subscribers (inspector events + usage)
    for (const fn of wsListenersRef.current) fn(msg);

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

    if (msg.event === "replay") {
      const bytes = b64decode(msg.data);
      harnessRef.current.replay(bytes, msg.offset, msg.next_offset);
      const action = gateRef.current.onReplay(bytes.length, msg.offset, msg.next_offset);
      if (!action) return;
      const term = termRef.current?.terminal;
      if (!term) return;
      if (action.isFirst) term.reset();
      term.write(action.skip > 0 ? bytes.subarray(action.skip) : bytes);
      setPtyOffset(gateRef.current.offset());
      scheduleDiagnose();
    } else if (msg.event === "pty") {
      const bytes = b64decode(msg.data);
      harnessRef.current.pty(bytes, msg.offset);
      const skip = gateRef.current.onPty(bytes.length, msg.offset);
      if (skip === null) return;
      const term = termRef.current?.terminal;
      if (!term) return;
      term.write(skip > 0 ? bytes.subarray(skip) : bytes);
      setPtyOffset(gateRef.current.offset());
      scheduleDiagnose();
    } else if (msg.event === "transition") {
      setAgentState(msg.next);
      setPrompt(msg.prompt ?? null);
      setLastMessage(msg.last_message ?? null);
    } else if (msg.event === "exit") {
      setWsStatus("disconnected");
      setAgentState("exited");
    }
  }

  const {
    send,
    request,
    status: connectionStatus,
  } = useWebSocket({ path: "/ws?subscribe=pty,state,usage,hooks", onMessage });

  const sendRef = useRef(send);
  sendRef.current = send;
  const requestRef = useRef(request);
  requestRef.current = request;

  useEffect(() => {
    setWsStatus(connectionStatus);
    if (connectionStatus === "connected") {
      gateRef.current.reset();
      harnessRef.current.reconnect();
      lastDiagnoseCountRef.current = 0;
      setShowStreamAlert(null);
      // Resize before replay so the PTY dimensions match XTerm when the
      // ring buffer snapshot is captured. WS messages are ordered, so the
      // server processes resize before replay:get — no need to await.
      const term = termRef.current?.terminal;
      if (term) {
        sendRef.current({ event: "resize", cols: term.cols, rows: term.rows });
      }
      sendRef.current({ event: "replay:get", offset: 0 });
      // Initial agent state poll
      requestRef
        .current({ event: "agent:get" })
        .then((res) => {
          if (res.ok && res.json) {
            const a = res.json as { state?: string; prompt?: PromptContext; last_message?: string };
            if (a.state) setAgentState(a.state);
            setPrompt(a.prompt ?? null);
            setLastMessage(a.last_message ?? null);
          }
        })
        .catch(() => {});
    }
  }, [connectionStatus]);

  // OAuth auto-prompt (derived at render time)
  const oauthUrl = prompt?.subtype === "oauth_login" && prompt.input ? prompt.input : null;

  // Keep-alive ping
  useInterval(() => send({ event: "ping" }), connectionStatus === "connected" ? 15_000 : null);

  function onTermData(data: string) {
    const encoder = new TextEncoder();
    send({ event: "input:send:raw", data: b64encode(encoder.encode(data)) });
  }

  function onTermBinary(data: string) {
    const bytes = new Uint8Array(data.length);
    for (let i = 0; i < data.length; i++) bytes[i] = data.charCodeAt(i);
    send({ event: "input:send:raw", data: b64encode(bytes) });
  }

  function onTermResize(size: { cols: number; rows: number }) {
    send({ event: "resize", ...size });
  }

  const { dragActive } = useFileUpload({
    uploadPath: "/api/v1/upload",
    onUploaded: (paths) => {
      const text = `${paths.join(" ")} `;
      const encoder = new TextEncoder();
      send({ event: "input:send:raw", data: b64encode(encoder.encode(text)) });
      termRef.current?.terminal?.focus();
    },
    onError: (msg) => {
      termRef.current?.terminal?.write(`\r\n\x1b[31m[${msg}]\x1b[0m\r\n`);
    },
  });

  function focusTerminal() {
    termRef.current?.terminal?.focus();
  }

  return (
    <TerminalLayout
      className="h-screen"
      title={location.host}
      wsStatus={wsStatus}
      agentState={agentState}
      ptyOffset={ptyOffset}
      onInteraction={focusTerminal}
      inspector={
        <InspectorSidebar
          subscribeWsEvents={subscribeWsEvents}
          prompt={prompt}
          lastMessage={lastMessage}
          wsSend={send}
          wsRequest={request}
          onTabClick={focusTerminal}
        />
      }
    >
      <DropOverlay active={dragActive} />
      {oauthUrl && <OAuthToast url={oauthUrl} onDismiss={() => setPrompt(null)} />}
      {showStreamAlert && (
        <StreamAlert message={showStreamAlert} onDismiss={() => setShowStreamAlert(null)} />
      )}
      <Terminal
        ref={termRef}
        fontSize={TERMINAL_FONT_SIZE}
        theme={THEME}
        className="min-w-0 flex-1 py-4 pl-4"
        onData={onTermData}
        onBinary={onTermBinary}
        onResize={onTermResize}
      />
    </TerminalLayout>
  );
}
