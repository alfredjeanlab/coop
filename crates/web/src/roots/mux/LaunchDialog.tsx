// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

import { useEffect, useRef, useState } from "react";
import { ActionBtn } from "@/components/ActionBtn";
import { ResultDisplay, showResult } from "@/components/ResultDisplay";
import { Section } from "@/components/Section";
import { apiGet, apiPost } from "@/hooks/useApiClient";
import { useInit } from "@/hooks/utils";

interface LaunchDialogProps {
  onClose: () => void;
}

interface EnvPreset {
  label: string;
  env: Record<string, string>;
}

const PRESETS: EnvPreset[] = [
  { label: "Local Directory", env: { WORKING_DIR: "" } },
  { label: "Git Clone", env: { GIT_REPO: "", GIT_BRANCH: "main", WORKING_DIR: "/workspace/repo" } },
  { label: "Empty", env: {} },
];

export function LaunchDialog({ onClose }: LaunchDialogProps) {
  const [selectedPreset, setSelectedPreset] = useState(0);
  const [env, setEnv] = useState<Record<string, string>>({ WORKING_DIR: "" });
  const [cwd, setCwd] = useState("");
  const [launching, setLaunching] = useState(false);
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null);
  const dialogRef = useRef<HTMLDivElement>(null);

  useInit(() => {
    apiGet("/api/v1/config/launch").then((res) => {
      if (res.ok && res.json && typeof res.json === "object") {
        const json = res.json as Record<string, unknown>;
        if (typeof json.cwd === "string") setCwd(json.cwd);
      }
    });
  });

  // Click outside closes dialog
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;
  useEffect(() => {
    const onClick = (e: MouseEvent) => {
      if (dialogRef.current && !dialogRef.current.contains(e.target as Node)) {
        onCloseRef.current();
      }
    };
    document.addEventListener("mousedown", onClick);
    return () => document.removeEventListener("mousedown", onClick);
  }, []);

  async function handleLaunch() {
    setLaunching(true);
    setResult(null);

    // Filter out empty values
    const filteredEnv = Object.fromEntries(Object.entries(env).filter(([_, v]) => v.trim() !== ""));

    const res = await apiPost("/api/v1/sessions/launch", { env: filteredEnv });
    setResult(showResult(res));
    setLaunching(false);

    if (res.ok) {
      setTimeout(onClose, 1500);
    }
  }

  const updateEnvKey = (oldKey: string, newKey: string) => {
    const newEnv = { ...env };
    if (oldKey !== newKey) {
      const value = newEnv[oldKey];
      delete newEnv[oldKey];
      newEnv[newKey] = value;
    }
    setEnv(newEnv);
  };

  const updateEnvValue = (key: string, value: string) => {
    setEnv({ ...env, [key]: value });
  };

  const addEnvVar = () => {
    setEnv({ ...env, NEW_VAR: "" });
  };

  const removeEnvVar = (key: string) => {
    const newEnv = { ...env };
    delete newEnv[key];
    setEnv(newEnv);
  };

  return (
    <div className="fixed inset-0 z-[300] flex items-center justify-center bg-black/50">
      <div
        ref={dialogRef}
        className="w-full max-w-lg rounded border border-[#21262d] bg-[#161b22] p-4 shadow-xl"
      >
        <Section
          label="Launch Session"
          headerRight={
            <button
              type="button"
              className="text-[10px] text-zinc-500 hover:text-zinc-300"
              onClick={onClose}
            >
              Close
            </button>
          }
        >
          {/* Preset selector */}
          <div className="mb-3">
            <label htmlFor="preset-select" className="mb-1 block text-[10px] text-zinc-500">
              Preset
            </label>
            <select
              id="preset-select"
              className="w-full rounded border border-[#2a2a2a] bg-[#0d1117] px-2 py-1 text-[11px] font-mono text-zinc-300 outline-none"
              value={selectedPreset}
              onChange={(e) => {
                const idx = Number(e.target.value);
                setSelectedPreset(idx);
                setEnv({ ...PRESETS[idx].env });
              }}
            >
              {PRESETS.map((preset, idx) => (
                <option key={idx} value={idx}>
                  {preset.label}
                </option>
              ))}
            </select>
          </div>

          {/* Environment variables */}
          <div className="mb-3">
            <div className="mb-1 flex items-center justify-between">
              <div className="text-[10px] text-zinc-500">Environment Variables</div>
              <button
                type="button"
                className="text-[10px] text-zinc-500 hover:text-zinc-300"
                onClick={addEnvVar}
              >
                + Add
              </button>
            </div>
            <div className="space-y-1.5">
              {Object.entries(env).map(([key, value]) => (
                <div key={key} className="flex gap-1.5">
                  <input
                    className="w-1/3 rounded border border-[#2a2a2a] bg-[#0d1117] px-2 py-1 text-[11px] font-mono text-zinc-300 placeholder-zinc-600 outline-none focus:border-zinc-500"
                    placeholder="KEY"
                    value={key}
                    onChange={(e) => updateEnvKey(key, e.target.value)}
                  />
                  <input
                    className="flex-1 rounded border border-[#2a2a2a] bg-[#0d1117] px-2 py-1 text-[11px] font-mono text-zinc-300 placeholder-zinc-600 outline-none focus:border-zinc-500"
                    placeholder={key === "WORKING_DIR" && cwd ? cwd : "value"}
                    value={value}
                    onChange={(e) => updateEnvValue(key, e.target.value)}
                  />
                  <button
                    type="button"
                    className="px-1 text-[14px] text-zinc-500 hover:text-red-400"
                    onClick={() => removeEnvVar(key)}
                  >
                    ×
                  </button>
                </div>
              ))}
              {Object.keys(env).length === 0 && (
                <div className="py-2 text-center text-[11px] text-zinc-500">
                  No environment variables
                </div>
              )}
            </div>
          </div>

          {/* Launch button */}
          <div className="flex gap-2">
            <ActionBtn
              variant="success"
              onClick={handleLaunch}
              className={launching ? "flex-1 opacity-50" : "flex-1"}
            >
              {launching ? "Launching..." : "Launch Session"}
            </ActionBtn>
          </div>

          <ResultDisplay result={result} />
        </Section>
      </div>
    </div>
  );
}
