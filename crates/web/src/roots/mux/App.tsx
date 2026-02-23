// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

import { useEffect, useRef, useState } from "react";
import { DropOverlay } from "@/components/DropOverlay";
import { OAuthToast } from "@/components/OAuthToast";
import { StatusBar } from "@/components/StatusBar";
import { LaunchCard, Tile } from "@/components/Tile";
import { apiGet } from "@/hooks/useApiClient";
import { useFileUpload } from "@/hooks/useFileUpload";
import { useWebSocket } from "@/hooks/useWebSocket";
import { useInit } from "@/hooks/utils";
import type { MuxMetadata, MuxWsMessage, PromptContext, SessionInfo } from "@/lib/types";
import { type CredentialAlert, CredentialPanel } from "./CredentialPanel";
import { ExpandedSession } from "./ExpandedSession";
import { MuxProvider, useMux } from "./MuxContext";
import { SessionSidebar } from "./SessionSidebar";

function AppInner() {
  const [sessions, setSessions] = useState<Map<string, SessionInfo>>(() => new Map());
  const sessionsRef = useRef(sessions);
  sessionsRef.current = sessions;

  const [focusedSession, setFocusedSession] = useState<string | null>(null);
  const focusedRef = useRef(focusedSession);
  focusedRef.current = focusedSession;

  const [expandedSession, setExpandedSession] = useState<string | null>(null);
  const expandedRef = useRef(expandedSession);
  expandedRef.current = expandedSession;

  const expandedSendInputRef = useRef<((text: string) => void) | null>(null);

  const [launchAvailable, setLaunchAvailable] = useState(false);
  const [oauthUrl, setOauthUrl] = useState<string | null>(null);

  const [credentialAlerts, setCredentialAlerts] = useState<Map<string, CredentialAlert>>(
    () => new Map(),
  );
  const [credPanelOpen, setCredPanelOpen] = useState(false);

  // Stats
  const sessionCount = sessions.size;
  let healthyCount = 0;
  for (const [, info] of sessions) {
    const s = (info.state || "").toLowerCase();
    if (s && s !== "exited" && !s.includes("error")) healthyCount++;
  }
  const alertCount = [...credentialAlerts.values()].filter(
    (a) => a.event !== "credential:refreshed",
  ).length;

  function createSession(
    id: string,
    url: string | null,
    state: string | null,
    metadata: MuxMetadata | null,
  ): SessionInfo {
    return {
      id,
      url,
      state,
      metadata,
      lastMessage: null,
      term: null,
      fit: null,
      sourceCols: 80,
      sourceRows: 24,
      lastScreenLines: null,
      credAlert: false,
    };
  }

  function handleExpandedTransition(
    sessionId: string,
    next: string,
    prompt: PromptContext | null,
    _lastMessage: string | null,
  ) {
    const info = sessionsRef.current.get(sessionId);
    if (info) {
      info.state = next;
      setSessions(new Map(sessionsRef.current));
    }
    if (prompt?.subtype === "oauth_login" && prompt.input) {
      setOauthUrl(prompt.input);
    }
  }

  function toggleExpand(id: string) {
    if (expandedRef.current === id) {
      setExpandedSession(null);
    } else {
      setExpandedSession(id);
      setFocusedSession(id);
    }
  }

  const muxSendRef = useRef<((msg: unknown) => void) | null>(null);

  function onMuxMessage(raw: unknown) {
    const msg = raw as MuxWsMessage;

    if (msg.event === "sessions") {
      const newSessions = new Map<string, SessionInfo>();
      const ids: string[] = [];
      for (const s of msg.sessions) {
        ids.push(s.id);
        if (sessionsRef.current.has(s.id)) {
          // Reuse existing SessionInfo (preserves state + screen data)
          const existing = sessionsRef.current.get(s.id)!;
          // Update URL/state/metadata from backend (in case they changed)
          existing.url = s.url ?? null;
          existing.state = s.state ?? null;
          existing.metadata = s.metadata ?? null;
          newSessions.set(s.id, existing);
        } else {
          newSessions.set(
            s.id,
            createSession(s.id, s.url ?? null, s.state ?? null, s.metadata ?? null),
          );
        }
      }
      // Dispose terminals for sessions that are no longer in the backend list
      for (const [id, info] of sessionsRef.current) {
        if (!newSessions.has(id)) {
          info.term?.dispose();
        }
      }
      sessionsRef.current = newSessions;
      setSessions(newSessions);
      if (ids.length > 0 && muxSendRef.current) {
        muxSendRef.current({ event: "subscribe", sessions: ids });
      }
    } else if (msg.event === "transition") {
      const info = sessionsRef.current.get(msg.session);
      if (info) {
        info.state = msg.next;
        if (msg.last_message != null) info.lastMessage = msg.last_message;
        setSessions(new Map(sessionsRef.current));
      }
      if (msg.prompt?.subtype === "oauth_login" && msg.prompt.input) {
        setOauthUrl(msg.prompt.input);
      }
    } else if (msg.event === "session:online") {
      if (!sessionsRef.current.has(msg.session)) {
        const newSessions = new Map(sessionsRef.current);
        newSessions.set(
          msg.session,
          createSession(msg.session, msg.url ?? null, null, msg.metadata ?? null),
        );
        sessionsRef.current = newSessions;
        setSessions(newSessions);
        muxSendRef.current?.({ event: "subscribe", sessions: [msg.session] });
      }
    } else if (msg.event === "session:offline") {
      const info = sessionsRef.current.get(msg.session);
      if (info) {
        info.term?.dispose();
        const newSessions = new Map(sessionsRef.current);
        newSessions.delete(msg.session);
        sessionsRef.current = newSessions;
        setSessions(newSessions);
        if (focusedRef.current === msg.session) setFocusedSession(null);
        if (expandedRef.current === msg.session) setExpandedSession(null);
      }
    } else if (
      msg.event === "credential:refreshed" ||
      msg.event === "credential:refresh:failed" ||
      msg.event === "credential:reauth:required"
    ) {
      setCredentialAlerts((prev) => {
        const next = new Map(prev);
        if (msg.event === "credential:refreshed") {
          next.delete(msg.account);
        } else {
          const alert: CredentialAlert = { event: msg.event };
          if (msg.event === "credential:reauth:required") {
            const reauth = msg as { auth_url?: string; user_code?: string };
            alert.auth_url = reauth.auth_url;
            alert.user_code = reauth.user_code;
          }
          next.set(msg.account, alert);
        }
        return next;
      });
    } else if (msg.event === "screen_batch") {
      for (const scr of msg.screens) {
        const info = sessionsRef.current.get(scr.session);
        if (!info) continue;

        const lines = scr.lines.slice();
        const ansi = scr.ansi?.slice() ?? lines.slice();
        // Trim trailing blank lines, but leave one for bottom padding.
        while (lines.length > 1 && lines[lines.length - 1].trim() === "") {
          lines.pop();
          ansi.pop();
        }

        info.sourceCols = scr.cols;
        info.sourceRows = scr.rows;
        info.lastScreenLines = ansi;
      }
      setSessions((prev) => new Map(prev));
    }
  }

  const { send: muxSend, status: muxWsStatus } = useWebSocket({
    path: "/ws/mux",
    onMessage: onMuxMessage,
  });

  // Keep muxSendRef in sync
  muxSendRef.current = muxSend;

  useInit(() => {
    apiGet("/api/v1/config/launch").then((res) => {
      if (
        res.ok &&
        res.json &&
        typeof res.json === "object" &&
        "available" in (res.json as Record<string, unknown>)
      ) {
        setLaunchAvailable((res.json as Record<string, unknown>).available === true);
      }
    });
  });

  const { dragActive } = useFileUpload({
    uploadPath: () => (focusedRef.current ? `/api/v1/sessions/${focusedRef.current}/upload` : null),
    onUploaded: (paths) => {
      const text = `${paths.join(" ")} `;
      const focused = focusedRef.current;
      if (!focused) return;
      if (expandedSendInputRef.current) {
        expandedSendInputRef.current(text);
      } else {
        muxSendRef.current?.({ event: "input:send", session: focused, text });
      }
      sessionsRef.current.get(focused)?.term?.focus();
    },
    onError: (msg) => {
      const focused = focusedRef.current;
      if (focused) {
        const info = sessionsRef.current.get(focused);
        info?.term?.write(`\r\n\x1b[31m[${msg}]\x1b[0m\r\n`);
      }
    },
  });

  const { sidebarCollapsed, toggleSidebar } = useMux();
  const sidebarWidth = sidebarCollapsed ? 40 : 220;

  const credPanelOpenRef = useRef(credPanelOpen);
  credPanelOpenRef.current = credPanelOpen;
  const toggleSidebarRef = useRef(toggleSidebar);
  toggleSidebarRef.current = toggleSidebar;
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        if (credPanelOpenRef.current) {
          setCredPanelOpen(false);
        } else if (expandedRef.current) {
          setExpandedSession(null);
        }
      }
      if (e.key === "b" && (e.metaKey || e.ctrlKey)) {
        e.preventDefault();
        toggleSidebarRef.current();
      }
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, []);

  const sessionArray = [...sessions.values()];

  return (
    <div className="flex h-screen flex-col bg-[#0d1117] font-sans text-[#c9d1d9]">
      {/* Header */}
      <header className="flex shrink-0 items-center gap-4 border-b border-[#21262d] px-2.5 py-2.5">
        <div className="flex items-center gap-2">
          <button
            type="button"
            className="border-none bg-transparent p-0.5 text-zinc-500 hover:text-zinc-300"
            onClick={toggleSidebar}
            title={sidebarCollapsed ? "Expand sidebar (Cmd+B)" : "Collapse sidebar (Cmd+B)"}
          >
            <svg
              width="16"
              height="16"
              viewBox="0 0 16 16"
              fill="none"
              stroke="currentColor"
              strokeWidth="1.5"
              strokeLinecap="round"
              strokeLinejoin="round"
            >
              <title>Toggle sidebar</title>
              <rect x="1.5" y="2" width="13" height="12" rx="1.5" />
              <line x1="5.5" y1="2" x2="5.5" y2="14" />
            </svg>
          </button>
          <h1 className="text-base font-semibold">coopmux</h1>
        </div>
        <div className="flex gap-4 text-[13px] text-zinc-500">
          <span>
            {sessionCount} session{sessionCount !== 1 ? "s" : ""}
          </span>
          <span>{healthyCount} healthy</span>
        </div>
        <div className="relative ml-auto">
          <button
            type="button"
            className={`rounded border px-2.5 py-1 text-[12px] transition-colors ${alertCount > 0 ? "border-red-700 bg-red-500/10 text-red-400 hover:border-red-500 hover:text-red-300" : "border-zinc-700 bg-[#1c2128] text-zinc-400 hover:border-zinc-500 hover:text-zinc-300"}`}
            onClick={() => setCredPanelOpen((v) => !v)}
          >
            {alertCount > 0
              ? `${alertCount} Credential Alert${alertCount !== 1 ? "s" : ""}`
              : "Credentials"}
          </button>
          {credPanelOpen && (
            <CredentialPanel onClose={() => setCredPanelOpen(false)} alerts={credentialAlerts} />
          )}
        </div>
      </header>

      <DropOverlay active={dragActive} />
      {oauthUrl && <OAuthToast url={oauthUrl} onDismiss={() => setOauthUrl(null)} />}

      {/* Main area: sidebar + content */}
      <div className="relative flex min-h-0 flex-1 flex-col">
        <div className="flex min-h-0 flex-1">
          <SessionSidebar
            sessions={sessionArray}
            expandedSession={expandedSession}
            focusedSession={focusedSession}
            launchAvailable={launchAvailable}
            onSelectSession={(id) => toggleExpand(id)}
          />

          {/* Grid */}
          {sessionCount > 0 || launchAvailable ? (
            <div className="grid flex-1 auto-rows-min grid-cols-[repeat(auto-fill,minmax(340px,1fr))] content-start gap-3 overflow-auto p-4">
              {sessionArray
                .filter((info) => info.id !== expandedSession)
                .map((info) => (
                  <Tile
                    key={info.id}
                    info={info}
                    focused={focusedSession === info.id}
                    onToggleExpand={() => toggleExpand(info.id)}
                  />
                ))}
              {launchAvailable && <LaunchCard />}
            </div>
          ) : (
            <div className="flex flex-1 items-center justify-center text-sm text-zinc-500">
              <p>Waiting for connections&hellip;</p>
            </div>
          )}
        </div>

        {/* Expanded session overlay */}
        {expandedSession && sessions.get(expandedSession) && (
          <ExpandedSession
            key={expandedSession}
            info={sessions.get(expandedSession)!}
            sidebarWidth={sidebarWidth}
            muxSend={muxSend}
            sendInputRef={expandedSendInputRef}
            onTransition={handleExpandedTransition}
            onClose={() => setExpandedSession(null)}
          />
        )}

        {/* Page-level status bar */}
        <StatusBar label="[coopmux]" wsStatus={muxWsStatus} />
      </div>
    </div>
  );
}

export function App() {
  return (
    <MuxProvider>
      <AppInner />
    </MuxProvider>
  );
}
