// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

import { useState } from "react";
import { ActionBtn } from "@/components/ActionBtn";
import { ResultDisplay, showResult } from "@/components/ResultDisplay";
import { Section } from "@/components/Section";
import type { WsRequest } from "@/hooks/useWebSocket";
import { useInit } from "@/hooks/utils";

export function ConfigPanel({ wsRequest }: { wsRequest: WsRequest }) {
  return (
    <div className="flex-1 overflow-y-auto">
      <ProfilesSection wsRequest={wsRequest} />
      <RegisterProfilesSection wsRequest={wsRequest} />
      <SessionSwitchSection wsRequest={wsRequest} />
      <StopConfigSection wsRequest={wsRequest} />
      <StartConfigSection wsRequest={wsRequest} />
      <TranscriptsSection wsRequest={wsRequest} />
      <SignalSection wsRequest={wsRequest} />
      <ShutdownSection wsRequest={wsRequest} />
    </div>
  );
}

interface ProfileInfo {
  name: string;
  status: string;
  cooldown_remaining_secs?: number;
}

function ProfilesSection({ wsRequest }: { wsRequest: WsRequest }) {
  const [profiles, setProfiles] = useState<ProfileInfo[]>([]);
  const [autoRotate, setAutoRotate] = useState(true);
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null);

  async function refresh() {
    const res = await wsRequest({ event: "profiles:list" });
    if (!res.ok) {
      setResult(showResult(res));
      return;
    }
    const data = res.json as { mode?: string; profiles?: ProfileInfo[] };
    setAutoRotate(data.mode === "auto");
    setProfiles(data.profiles ?? []);
  }

  useInit(() => {
    refresh();
  });

  async function toggleAutoRotate(checked: boolean) {
    const mode = checked ? "auto" : "manual";
    const res = await wsRequest({ event: "profiles:mode:set", mode });
    setResult(showResult(res));
    if (res.ok) refresh();
  }

  async function switchProfile(name: string) {
    const res = await wsRequest({ event: "session:switch", profile: name, force: false });
    setResult(showResult(res));
    if (res.ok) setTimeout(refresh, 500);
  }

  return (
    <Section
      label="Profiles"
      headerRight={
        <span className="flex items-center gap-1.5 text-[10px] font-normal normal-case tracking-normal">
          <label className="flex items-center gap-0.5">
            <input
              type="checkbox"
              checked={autoRotate}
              onChange={(e) => toggleAutoRotate(e.target.checked)}
              className="accent-blue-400"
            />
            <span className="text-zinc-500">Auto-rotate</span>
          </label>
          <ActionBtn onClick={refresh} className="!px-1.5 !py-px !text-[10px]">
            Refresh
          </ActionBtn>
        </span>
      }
    >
      {profiles.length === 0 ? (
        <div className="text-[10px] text-zinc-600">No profiles registered</div>
      ) : (
        <div className="mt-1 flex flex-col gap-0.5">
          {profiles.map((p) => (
            <div key={p.name} className="flex items-center gap-1.5 text-[11px]">
              <span
                className={`h-1.5 w-1.5 shrink-0 rounded-full ${
                  p.status === "active"
                    ? "bg-green-500"
                    : p.status === "rate_limited"
                      ? "bg-amber-500"
                      : "bg-zinc-500"
                }`}
              />
              <span className="text-zinc-300">{p.name}</span>
              <span className="text-[10px] text-zinc-500">
                {p.status}
                {p.cooldown_remaining_secs ? ` (${p.cooldown_remaining_secs}s)` : ""}
              </span>
              {p.status !== "active" && (
                <button
                  type="button"
                  className="ml-auto border-none bg-transparent p-0 text-[10px] text-blue-400 hover:text-blue-300 hover:underline"
                  onClick={() => switchProfile(p.name)}
                >
                  switch
                </button>
              )}
            </div>
          ))}
        </div>
      )}
      <ResultDisplay result={result} />
    </Section>
  );
}

function RegisterProfilesSection({ wsRequest }: { wsRequest: WsRequest }) {
  const [json, setJson] = useState("");
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null);

  async function handleRegister() {
    let profiles: unknown;
    try {
      profiles = JSON.parse(json);
    } catch {
      setResult({ ok: false, text: "Invalid JSON" });
      return;
    }
    const list = Array.isArray(profiles) ? profiles : [profiles];
    const res = await wsRequest({ event: "profiles:register", profiles: list });
    setResult(showResult(res));
    if (res.ok) setJson("");
  }

  return (
    <Section label="Register Profiles">
      <textarea
        value={json}
        onChange={(e) => setJson(e.target.value)}
        rows={3}
        placeholder='[{"name":"main","credentials":{"ANTHROPIC_API_KEY":"sk-ant-..."}}]'
        className="w-full resize-y rounded border border-zinc-600 bg-[#222] px-1.5 py-0.5 font-mono text-[11px] leading-snug text-zinc-300 outline-none focus:border-blue-400"
      />
      <div className="mt-1">
        <ActionBtn variant="success" onClick={handleRegister}>
          Register
        </ActionBtn>
      </div>
      <ResultDisplay result={result} />
    </Section>
  );
}

function SessionSwitchSection({ wsRequest }: { wsRequest: WsRequest }) {
  const [creds, setCreds] = useState("");
  const [force, setForce] = useState(false);
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null);

  async function handleSwitch() {
    let credentials = null;
    if (creds.trim()) {
      try {
        credentials = JSON.parse(creds);
      } catch {
        setResult({ ok: false, text: "Invalid JSON" });
        return;
      }
    }
    const body: Record<string, unknown> = { event: "session:switch", force };
    if (credentials) body.credentials = credentials;
    const res = await wsRequest(body);
    setResult(showResult(res));
  }

  return (
    <Section label="Session Switch">
      <textarea
        value={creds}
        onChange={(e) => setCreds(e.target.value)}
        rows={2}
        placeholder='{"ANTHROPIC_API_KEY": "sk-ant-..."}'
        className="w-full resize-y rounded border border-zinc-600 bg-[#222] px-1.5 py-0.5 font-mono text-[11px] leading-snug text-zinc-300 outline-none focus:border-blue-400"
      />
      <div className="mt-1 flex items-center gap-2">
        <label className="flex items-center gap-0.5">
          <input
            type="checkbox"
            checked={force}
            onChange={(e) => setForce(e.target.checked)}
            className="accent-blue-400"
          />
          <span className="text-[10px] text-zinc-500">Force (skip idle wait)</span>
        </label>
        <ActionBtn variant="warn" onClick={handleSwitch}>
          Switch
        </ActionBtn>
      </div>
      <ResultDisplay result={result} />
    </Section>
  );
}

function StopConfigSection({ wsRequest }: { wsRequest: WsRequest }) {
  const [mode, setMode] = useState("allow");
  const [prompt, setPrompt] = useState("");
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null);

  async function handleGet() {
    const res = await wsRequest({ event: "stop:config:get" });
    setResult(showResult(res));
    if (res.ok && res.json) {
      const data = res.json as { config?: { mode?: string; prompt?: string } };
      const cfg = data.config ?? (res.json as { mode?: string; prompt?: string });
      setMode(cfg.mode || "allow");
      setPrompt(cfg.prompt || "");
    }
  }

  async function handlePut() {
    const res = await wsRequest({
      event: "stop:config:put",
      config: { mode, prompt: prompt || null },
    });
    setResult(showResult(res));
  }

  return (
    <Section label="Stop Config">
      <div className="flex items-center gap-1">
        <select
          value={mode}
          onChange={(e) => setMode(e.target.value)}
          className="rounded border border-zinc-600 bg-[#222] px-1.5 py-0.5 font-mono text-[11px] text-zinc-300 outline-none focus:border-blue-400"
        >
          <option value="allow">allow</option>
          <option value="signal">signal</option>
        </select>
        <ActionBtn onClick={handleGet}>Get</ActionBtn>
        <ActionBtn onClick={handlePut}>Put</ActionBtn>
      </div>
      <div className="mt-1">
        <input
          type="text"
          value={prompt}
          onChange={(e) => setPrompt(e.target.value)}
          placeholder="Block reason (optional)"
          className="w-full rounded border border-zinc-600 bg-[#222] px-1.5 py-0.5 font-mono text-[11px] text-zinc-300 outline-none focus:border-blue-400"
        />
      </div>
      <ResultDisplay result={result} />
    </Section>
  );
}

function StartConfigSection({ wsRequest }: { wsRequest: WsRequest }) {
  const [json, setJson] = useState("");
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null);

  async function handleGet() {
    const res = await wsRequest({ event: "config:start:get" });
    setResult(showResult(res));
    if (res.ok && res.json) {
      const data = res.json as { config?: unknown };
      setJson(JSON.stringify(data.config ?? res.json, null, 2));
    }
  }

  async function handlePut() {
    let body: unknown;
    try {
      body = JSON.parse(json);
    } catch {
      setResult({ ok: false, text: "Invalid JSON" });
      return;
    }
    const res = await wsRequest({ event: "config:put:get", config: body });
    setResult(showResult(res));
  }

  return (
    <Section label="Start Config">
      <div className="flex items-center gap-1">
        <ActionBtn onClick={handleGet}>Get</ActionBtn>
        <ActionBtn onClick={handlePut}>Put</ActionBtn>
      </div>
      <textarea
        value={json}
        onChange={(e) => setJson(e.target.value)}
        rows={2}
        placeholder='{"mode":"allow"}'
        className="mt-1 w-full resize-y rounded border border-zinc-600 bg-[#222] px-1.5 py-0.5 font-mono text-[11px] leading-snug text-zinc-300 outline-none focus:border-blue-400"
      />
      <ResultDisplay result={result} />
    </Section>
  );
}

interface TranscriptInfo {
  number: number;
  timestamp: string;
  line_count: number;
  byte_size: number;
}

function formatBytes(b: number): string {
  if (b < 1024) return `${b}B`;
  if (b < 1024 * 1024) return `${(b / 1024).toFixed(1)}K`;
  return `${(b / (1024 * 1024)).toFixed(1)}M`;
}

function TranscriptsSection({ wsRequest }: { wsRequest: WsRequest }) {
  const [transcripts, setTranscripts] = useState<TranscriptInfo[]>([]);
  const [activeLine, setActiveLine] = useState<number | null>(null);
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null);

  async function refresh() {
    const [listRes, catchupRes] = await Promise.all([
      wsRequest({ event: "transcript:list" }),
      wsRequest({ event: "transcript:catchup", since_transcript: 0, since_line: 0 }),
    ]);
    if (!listRes.ok) {
      setResult(showResult(listRes));
      return;
    }
    setTranscripts((listRes.json as { transcripts?: TranscriptInfo[] })?.transcripts ?? []);
    if (catchupRes.ok && catchupRes.json) {
      setActiveLine((catchupRes.json as { current_line?: number }).current_line ?? null);
    }
  }

  useInit(() => {
    refresh();
  });

  async function downloadTranscriptCatchup() {
    try {
      const res = await wsRequest({
        event: "transcript:catchup",
        since_transcript: 0,
        since_line: 0,
      });
      if (!res.ok || !res.json) {
        throw new Error(res.text || "Request failed");
      }
      const data = res.json as { transcripts?: Array<{ lines: string[] }>; live_lines?: string[] };
      const allLines: string[] = [];
      if (data.transcripts) {
        for (const transcript of data.transcripts) {
          allLines.push(...transcript.lines);
        }
      }
      if (data.live_lines) {
        allLines.push(...data.live_lines);
      }
      const content = allLines.join("\n");
      const blob = new Blob([content], { type: "text/plain;charset=utf-8" });
      const link = document.createElement("a");
      link.href = URL.createObjectURL(blob);
      link.download = "transcript.txt";
      document.body.appendChild(link);
      link.click();
      document.body.removeChild(link);
      URL.revokeObjectURL(link.href);
    } catch (err) {
      setResult({ ok: false, text: `Download failed: ${err}` });
    }
  }

  async function downloadTranscriptSnapshot(number: number) {
    try {
      const res = await wsRequest({ event: "transcript:get", number });
      if (!res.ok || !res.json) {
        throw new Error(res.text || "Request failed");
      }
      const data = res.json as { content?: string };
      if (!data.content) {
        throw new Error("No content in response");
      }
      const blob = new Blob([data.content], { type: "text/plain;charset=utf-8" });
      const link = document.createElement("a");
      link.href = URL.createObjectURL(blob);
      link.download = `transcript-${number}.txt`;
      document.body.appendChild(link);
      link.click();
      document.body.removeChild(link);
      URL.revokeObjectURL(link.href);
    } catch (err) {
      setResult({ ok: false, text: `Download failed: ${err}` });
    }
  }

  return (
    <Section
      label="Transcripts"
      headerRight={
        <ActionBtn onClick={refresh} className="!px-1.5 !py-px !text-[10px]">
          Refresh
        </ActionBtn>
      }
    >
      {activeLine != null && (
        <div className="mt-1 text-[10px] text-zinc-500">
          <span className="text-green-500">active</span> {activeLine} lines{" "}
          <ActionBtn onClick={downloadTranscriptCatchup} className="!px-1.5 !py-px !text-[10px]">
            Download
          </ActionBtn>
        </div>
      )}
      {transcripts.length === 0 ? (
        <div className="mt-1 text-[10px] text-zinc-600">No snapshots yet</div>
      ) : (
        <div className="mt-1 flex flex-col gap-0.5">
          {transcripts.map((t) => {
            const time = new Date(Number(t.timestamp) * 1000).toLocaleTimeString([], {
              hour: "2-digit",
              minute: "2-digit",
            });
            return (
              <div key={t.number} className="flex items-center gap-1.5 text-[11px]">
                <span className="min-w-5 text-zinc-500">#{t.number}</span>
                <span className="flex-1 text-zinc-400">
                  {time} · {t.line_count} lines · {formatBytes(t.byte_size)}
                </span>
                <ActionBtn
                  onClick={() => downloadTranscriptSnapshot(t.number)}
                  className="!px-1.5 !py-px !text-[10px]"
                >
                  Download
                </ActionBtn>
              </div>
            );
          })}
        </div>
      )}
      <ResultDisplay result={result} />
    </Section>
  );
}

function SignalSection({ wsRequest }: { wsRequest: WsRequest }) {
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null);

  async function sendSignal(signal: string) {
    const res = await wsRequest({ event: "signal:send", signal });
    setResult(showResult(res));
  }

  return (
    <Section label="Signal">
      <div className="flex items-center gap-1">
        <ActionBtn onClick={() => sendSignal("SIGINT")}>SIGINT</ActionBtn>
        <ActionBtn variant="warn" onClick={() => sendSignal("SIGTERM")}>
          SIGTERM
        </ActionBtn>
        <ActionBtn variant="danger" onClick={() => sendSignal("SIGKILL")}>
          SIGKILL
        </ActionBtn>
      </div>
      <div className="mt-1 flex items-center gap-1">
        <ActionBtn onClick={() => sendSignal("SIGTSTP")}>SIGTSTP</ActionBtn>
        <ActionBtn onClick={() => sendSignal("SIGCONT")}>SIGCONT</ActionBtn>
        <ActionBtn onClick={() => sendSignal("SIGHUP")}>SIGHUP</ActionBtn>
      </div>
      <ResultDisplay result={result} />
    </Section>
  );
}

function ShutdownSection({ wsRequest }: { wsRequest: WsRequest }) {
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null);

  const handleRestart = async () => {
    const res = await wsRequest({ event: "session:restart" });
    setResult(showResult(res));
  };

  const handleShutdown = async () => {
    const res = await wsRequest({ event: "shutdown" });
    setResult(showResult(res));
  };

  return (
    <Section label="Lifecycle">
      <div className="flex items-center gap-1">
        <ActionBtn variant="warn" onClick={handleRestart}>
          Restart
        </ActionBtn>
        <ActionBtn variant="danger" onClick={handleShutdown}>
          Shutdown
        </ActionBtn>
      </div>
      <ResultDisplay result={result} />
    </Section>
  );
}
