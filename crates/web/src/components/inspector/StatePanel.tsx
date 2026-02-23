// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

import { useEffect, useRef, useState } from "react";
import type { EventEntry } from "@/lib/types";

function renderRows(obj: unknown, prefix: string): { key: string; value: string }[] {
  if (obj == null) return [{ key: prefix || "--", value: "--" }];
  if (typeof obj !== "object") return [{ key: prefix, value: String(obj) }];
  const entries = Array.isArray(obj)
    ? obj.map((v, i) => [String(i), v] as const)
    : Object.entries(obj as Record<string, unknown>);
  const rows: { key: string; value: string }[] = [];
  for (const [k, v] of entries) {
    const key = prefix ? `${prefix}.${k}` : k;
    if (v != null && typeof v === "object") {
      rows.push(...renderRows(v, key));
    } else {
      rows.push({ key, value: String(v ?? "--") });
    }
  }
  return rows;
}

function ApiTable({
  label,
  data,
  defaultOpen = true,
}: {
  label: string;
  data: unknown;
  defaultOpen?: boolean;
}) {
  const rows = data ? renderRows(data, "") : [];
  return (
    <div className="px-2.5 py-1">
      <details open={defaultOpen}>
        <summary className="cursor-pointer py-0.5 text-[11px] font-semibold text-zinc-400 select-none">
          {label}
        </summary>
        <table className="w-full border-collapse">
          <tbody>
            {rows.length > 0 ? (
              rows.map((r, i) => (
                <tr key={i}>
                  <td className="max-w-[90px] truncate px-1 py-px text-[11px] text-zinc-500">
                    {r.key}
                  </td>
                  <td className="max-w-[180px] truncate px-1 py-px text-[11px] text-zinc-300">
                    {r.value}
                  </td>
                </tr>
              ))
            ) : (
              <tr>
                <td colSpan={2} className="px-1 py-px text-[11px] text-zinc-600">
                  --
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </details>
    </div>
  );
}

const typeColors: Record<string, string> = {
  pty: "text-blue-400",
  output: "text-blue-400",
  transition: "text-yellow-400",
  exit: "text-red-400",
  error: "text-red-400",
  resize: "text-teal-400",
  pong: "text-zinc-600",
  stop: "text-amber-500",
  start: "text-green-400",
  prompt: "text-green-400",
  session: "text-purple-400",
  usage: "text-teal-400",
  hook: "text-purple-400",
};

function eventTypeColor(type: string): string {
  const base = type.split(":")[0];
  return typeColors[base] || "text-zinc-400";
}

interface StatePanelProps {
  health: unknown;
  status: unknown;
  agent: unknown;
  usage: unknown;
  events: EventEntry[];
}

export function StatePanel({ health, status, agent, usage, events }: StatePanelProps) {
  const logRef = useRef<HTMLDivElement>(null);
  const [apiHeight, setApiHeight] = useState<number | undefined>(undefined);

  // Auto-scroll event log when new events arrive
  const eventCount = events.length;
  useEffect(() => {
    if (!eventCount) return;
    const el = logRef.current;
    if (!el) return;
    if (el.scrollHeight - el.scrollTop - el.clientHeight < 60) {
      el.scrollTop = el.scrollHeight;
    }
  }, [eventCount]);

  // Horizontal resize handle
  function handleHResize(e: React.MouseEvent) {
    e.preventDefault();
    const panel = e.currentTarget.closest("[data-state-panel]") as HTMLElement | null;
    if (!panel) return;

    document.body.style.cursor = "row-resize";
    document.body.style.userSelect = "none";

    const onMove = (ev: MouseEvent) => {
      const rect = panel.getBoundingClientRect();
      const y = ev.clientY - rect.top;
      const max = rect.height - 60 - 5;
      setApiHeight(Math.min(max, Math.max(80, y)));
    };

    const onUp = () => {
      document.body.style.cursor = "";
      document.body.style.userSelect = "";
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    };

    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col" data-state-panel>
      {/* API section */}
      <div className="shrink-0 overflow-y-auto" style={{ height: apiHeight ?? "50%" }}>
        <ApiTable label="Health" data={health} />
        <ApiTable label="Status" data={status} />
        <ApiTable label="Agent" data={agent} />
        <ApiTable label="Usage" data={usage} />
      </div>

      {/* Resize handle */}
      <div
        aria-hidden="true"
        className="h-[5px] shrink-0 cursor-row-resize border-t border-[#2a2a2a] transition-colors hover:bg-blue-400"
        onMouseDown={handleHResize}
      />

      {/* Events header */}
      <h2 className="shrink-0 border-b border-[#2a2a2a] px-2.5 py-2 text-[11px] font-semibold uppercase tracking-wide text-zinc-500">
        Events
      </h2>

      {/* Event log */}
      <div
        ref={logRef}
        className="min-h-[60px] flex-1 overflow-y-auto px-2.5 py-1 text-[11px] leading-snug"
      >
        {events.map((ev, i) => (
          <div key={i} className="border-b border-[#222] py-0.5">
            <span className="text-zinc-600">{ev.ts}</span>{" "}
            <span className={`font-semibold ${eventTypeColor(ev.type)}`}>{ev.type}</span>
            {ev.detail && <span className="ml-1 text-zinc-500">{ev.detail}</span>}
          </div>
        ))}
      </div>
    </div>
  );
}
