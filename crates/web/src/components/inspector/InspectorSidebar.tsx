// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

import { useEffect, useRef, useState } from "react";
import type { WsRequest } from "@/hooks/useWebSocket";
import { useInterval } from "@/hooks/utils";
import type { EventEntry, PromptContext, WsMessage } from "@/lib/types";
import { ActionsPanel } from "./ActionsPanel";
import { ConfigPanel } from "./ConfigPanel";
import { StatePanel } from "./StatePanel";

type InspectorTab = "state" | "actions" | "config";

export type WsEventListener = (msg: WsMessage) => void;

export interface InspectorSidebarProps {
  /** Subscribe to raw WS events for the event log + live usage. Returns unsubscribe fn. */
  subscribeWsEvents: (listener: WsEventListener) => () => void;
  prompt: PromptContext | null;
  lastMessage: string | null;
  wsSend: (msg: unknown) => void;
  wsRequest: WsRequest;
  onTabClick?: () => void;
}

export function InspectorSidebar({
  subscribeWsEvents,
  prompt,
  lastMessage,
  wsSend,
  wsRequest,
  onTabClick,
}: InspectorSidebarProps) {
  const [activeTab, setActiveTab] = useState<InspectorTab>("state");

  const [health, setHealth] = useState<unknown>(null);
  const [status, setStatus] = useState<unknown>(null);
  const [agent, setAgent] = useState<unknown>(null);
  const [usage, setUsage] = useState<unknown>(null);

  useInterval(async () => {
    const [h, st, ag, u] = await Promise.all([
      wsRequest({ event: "health:get" }).catch(() => null),
      wsRequest({ event: "status:get" }).catch(() => null),
      wsRequest({ event: "agent:get" }).catch(() => null),
      wsRequest({ event: "usage:get" }).catch(() => null),
    ]);
    if (h?.ok) setHealth(h.json);
    if (st?.ok) setStatus(st.json);
    if (ag?.ok) setAgent(ag.json);
    if (u?.ok) setUsage(u.json);
  }, 2000);

  const [events, setEvents] = useState<EventEntry[]>([]);

  const subscribeRef = useRef(subscribeWsEvents);
  subscribeRef.current = subscribeWsEvents;
  useEffect(() => {
    return subscribeRef.current((msg) => {
      // Live usage updates
      if (msg.event === "usage:update" && msg.cumulative) {
        setUsage({ ...msg.cumulative, uptime_secs: "(live)" });
      }
      // Append to event log
      appendEvent(msg, setEvents);
    });
  }, []);

  return (
    <>
      {/* Tab bar */}
      <div className="flex shrink-0 border-b border-[#333] bg-[#151515]">
        {(["state", "actions", "config"] as const).map((tab) => (
          <button
            type="button"
            key={tab}
            className={`flex-1 border-b-2 py-1.5 text-center text-[11px] font-semibold uppercase tracking-wide transition-colors ${
              activeTab === tab
                ? "border-blue-400 text-zinc-300"
                : "border-transparent text-zinc-600 hover:text-zinc-400"
            }`}
            onClick={() => {
              setActiveTab(tab);
              onTabClick?.();
            }}
          >
            {tab}
          </button>
        ))}
      </div>

      {/* Panels */}
      {activeTab === "state" && (
        <StatePanel health={health} status={status} agent={agent} usage={usage} events={events} />
      )}
      {activeTab === "actions" && (
        <ActionsPanel
          prompt={prompt}
          lastMessage={lastMessage}
          wsSend={wsSend}
          wsRequest={wsRequest}
        />
      )}
      {activeTab === "config" && <ConfigPanel wsRequest={wsRequest} />}
    </>
  );
}

function appendEvent(
  msg: WsMessage,
  setEvents: React.Dispatch<React.SetStateAction<EventEntry[]>>,
) {
  setEvents((prev) => {
    const next = [...prev];
    const type = msg.event;
    const ts = new Date().toTimeString().slice(0, 8);

    // Collapse pty/replay
    if (type === "pty" || type === "replay") {
      const len = "data" in msg && msg.data ? atob(msg.data).length : 0;
      const last = next[next.length - 1];
      if (last?.type === "pty") {
        return [
          ...next.slice(0, -1),
          {
            ...last,
            ts,
            detail: `${(last.count ?? 1) + 1}x ${(last.bytes ?? 0) + len}B thru ${("offset" in msg ? msg.offset : 0) + len}`,
            count: (last.count ?? 1) + 1,
            bytes: (last.bytes ?? 0) + len,
          },
        ];
      }
      return [
        ...next,
        {
          ts,
          type: "pty",
          detail: `1x ${len}B thru ${("offset" in msg ? msg.offset : 0) + len}`,
          count: 1,
          bytes: len,
        },
      ].slice(-200);
    }

    // Collapse pong
    if (type === "pong") {
      const last = next[next.length - 1];
      if (last?.type === "pong") {
        return [
          ...next.slice(0, -1),
          { ...last, ts, detail: `${(last.count ?? 1) + 1}x`, count: (last.count ?? 1) + 1 },
        ];
      }
      return [...next, { ts, type: "pong", detail: "1x", count: 1 }].slice(-200);
    }

    // Other events
    let detail = "";
    if (msg.event === "transition") {
      detail = `${msg.prev} -> ${msg.next}`;
      if (msg.cause) detail += ` [${msg.cause}]`;
      if (msg.error_detail) detail += ` (${msg.error_category || "error"})`;
    } else if (msg.event === "exit") {
      detail = msg.signal != null ? `signal ${msg.signal}` : `code ${msg.code ?? "?"}`;
    } else if (msg.event === "error") {
      detail = `${msg.code}: ${msg.message}`;
    } else if (msg.event === "resize") {
      detail = `${msg.cols}x${msg.rows}`;
    } else if (msg.event === "stop:outcome") {
      detail = msg.type || "";
    } else if (msg.event === "start:outcome") {
      detail = msg.source || "";
      if (msg.session_id) detail += ` session=${msg.session_id}`;
      if (msg.injected) detail += " (injected)";
    } else if (msg.event === "prompt:outcome") {
      detail = `${msg.source}: ${msg.type || "?"}`;
      if (msg.subtype) detail += `(${msg.subtype})`;
      if (msg.option != null) detail += ` opt=${msg.option}`;
    } else if (msg.event === "session:switched") {
      detail = msg.scheduled ? "scheduled" : "immediate";
    } else if (msg.event === "usage:update") {
      detail = msg.cumulative
        ? `in=${msg.cumulative.input_tokens} out=${msg.cumulative.output_tokens} $${msg.cumulative.total_cost_usd?.toFixed(4) ?? "?"} seq=${msg.seq}`
        : `seq=${msg.seq}`;
    } else if (msg.event === "hook:raw") {
      const d = msg.data || {};
      const parts = [d.event || "?"];
      if (d.tool_name) parts.push(d.tool_name);
      if (d.notification_type) parts.push(d.notification_type);
      detail = parts.join(" ");
    }

    return [...next, { ts, type: msg.event, detail }].slice(-200);
  });
}
