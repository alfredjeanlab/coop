// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

import { useEffect, useRef, useState } from "react";
import { ActionBtn } from "@/components/ActionBtn";
import { ResultDisplay, showResult } from "@/components/ResultDisplay";
import { Section } from "@/components/Section";
import { apiGet, apiPost } from "@/hooks/useApiClient";
import { useInterval } from "@/hooks/utils";

export interface CredentialAlert {
  event: string;
  auth_url?: string;
  user_code?: string;
}

interface AccountStatus {
  name: string;
  provider: string;
  status: "healthy" | "refreshing" | "expired";
  expires_in_secs?: number;
  has_refresh_token: boolean;
  reauth: boolean;
}

const statusStyles: Record<string, string> = {
  healthy: "bg-green-500/20 text-green-400",
  refreshing: "bg-yellow-500/20 text-yellow-400",
  expired: "bg-red-500/20 text-red-400",
};

function StatusBadge({ status }: { status: string }) {
  return (
    <span
      className={`inline-block rounded-full px-2 py-0.5 text-[10px] font-medium uppercase ${statusStyles[status] || "bg-zinc-700 text-zinc-400"}`}
    >
      {status}
    </span>
  );
}

/** Env key options per provider. First entry is the default. */
const providerEnvKeys: Record<string, string[]> = {
  claude: ["CLAUDE_CODE_OAUTH_TOKEN", "ANTHROPIC_API_KEY"],
  openai: ["OPENAI_API_KEY"],
  gemini: ["GEMINI_API_KEY"],
  other: ["API_KEY"],
};

function envKeysForProvider(provider: string): string[] {
  return providerEnvKeys[provider] ?? ["API_KEY"];
}

function formatExpiry(secs: number): string {
  if (secs < 60) return `${secs}s`;
  if (secs < 3600) return `${Math.floor(secs / 60)}m`;
  return `${Math.floor(secs / 3600)}h ${Math.floor((secs % 3600) / 60)}m`;
}

interface CredentialPanelProps {
  onClose: () => void;
  alerts?: Map<string, CredentialAlert>;
}

export function CredentialPanel({ onClose, alerts }: CredentialPanelProps) {
  const [accounts, setAccounts] = useState<AccountStatus[]>([]);
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null);
  const [showForm, setShowForm] = useState(false);
  const [pendingExchange, setPendingExchange] = useState<{ account: string; state: string } | null>(
    null,
  );
  const [exchangeCode, setExchangeCode] = useState("");
  const panelRef = useRef<HTMLDivElement>(null);

  // Fetch account status on mount and periodically.
  async function fetchStatus() {
    const res = await apiGet("/api/v1/credentials/status");
    if (res.ok && Array.isArray(res.json)) {
      setAccounts(res.json as AccountStatus[]);
    }
  }

  useInterval(fetchStatus, 10_000);

  // Click outside closes panel.
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;
  useEffect(() => {
    const onClick = (e: MouseEvent) => {
      if (panelRef.current && !panelRef.current.contains(e.target as Node)) {
        onCloseRef.current();
      }
    };
    document.addEventListener("mousedown", onClick);
    return () => document.removeEventListener("mousedown", onClick);
  }, []);

  async function handleReauth(account: string) {
    setResult(null);
    setPendingExchange(null);
    setExchangeCode("");
    const res = await apiPost("/api/v1/credentials/reauth", { account });
    if (res.ok && res.json && typeof res.json === "object") {
      const data = res.json as Record<string, unknown>;
      const authUrl = data.auth_url as string | undefined;
      const userCode = data.user_code as string | undefined;
      const state = data.state as string | undefined;

      if (userCode) {
        // Device code flow — show code for user to enter.
        setResult({ ok: true, text: `Enter code: ${userCode}` });
        fetchStatus();
        return;
      }
      if (authUrl) {
        // PKCE flow — open authorization URL in browser.
        window.open(authUrl, "_blank");
        if (state) {
          // Platform redirect — user will paste the code back.
          setPendingExchange({ account, state });
        }
      }
    }
    if (!pendingExchange) {
      setResult(showResult(res));
    }
    fetchStatus();
  }

  async function handleExchange() {
    if (!pendingExchange || !exchangeCode.trim()) return;
    setResult(null);
    const res = await apiPost("/api/v1/credentials/exchange", {
      state: pendingExchange.state,
      code: exchangeCode.trim(),
    });
    if (res.ok) {
      setPendingExchange(null);
      setExchangeCode("");
      setResult({ ok: true, text: "Authorization complete" });
    } else {
      setResult(showResult(res));
    }
    fetchStatus();
  }

  async function handleDistribute(account: string) {
    setResult(null);
    const res = await apiPost("/api/v1/credentials/distribute", { account, switch: true });
    setResult(showResult(res));
  }

  const [formName, setFormName] = useState("");
  const [formProvider, setFormProvider] = useState("claude");
  const [formEnvKey, setFormEnvKey] = useState(envKeysForProvider("claude")[0]);
  const [formToken, setFormToken] = useState("");
  const [formSubmitting, setFormSubmitting] = useState(false);

  async function handleAddAccount() {
    if (!formName.trim()) return;
    setFormSubmitting(true);
    setResult(null);

    const body: Record<string, unknown> = {
      name: formName.trim(),
      provider: formProvider,
      env_key: formEnvKey,
      reauth: !formToken.trim(),
    };
    if (formToken.trim()) {
      body.token = formToken.trim();
    }

    const res = await apiPost("/api/v1/credentials/new", body);
    setResult(showResult(res));
    setFormSubmitting(false);

    if (res.ok) {
      setFormName("");
      setFormToken("");
      setFormEnvKey(envKeysForProvider(formProvider)[0]);
      setShowForm(false);
      fetchStatus();
    }
  }

  return (
    <div
      ref={panelRef}
      className="absolute right-0 top-full z-[200] mt-1 w-80 rounded border border-[#21262d] bg-[#161b22] shadow-xl"
    >
      {/* Device code alerts */}
      {alerts && [...alerts.entries()].some(([, a]) => a.user_code) && (
        <Section label="Authorization Required">
          {[...alerts.entries()]
            .filter(([, a]) => a.user_code)
            .map(([account, alert]) => (
              <div
                key={account}
                className="mb-1.5 rounded border border-yellow-700/50 bg-yellow-500/10 px-2 py-1.5"
              >
                <div className="text-[11px] text-yellow-300">{account}</div>
                <div className="mt-1 flex items-center gap-2">
                  <code className="rounded bg-[#0d1117] px-2 py-0.5 text-sm font-bold text-yellow-200">
                    {alert.user_code}
                  </code>
                  <button
                    type="button"
                    className="text-[10px] text-zinc-400 hover:text-zinc-200"
                    onClick={() => navigator.clipboard.writeText(alert.user_code ?? "")}
                  >
                    Copy
                  </button>
                </div>
                {alert.auth_url && (
                  <a
                    href={alert.auth_url}
                    target="_blank"
                    rel="noopener noreferrer"
                    className="mt-1 block truncate text-[10px] text-blue-400 hover:text-blue-300"
                  >
                    {alert.auth_url}
                  </a>
                )}
              </div>
            ))}
        </Section>
      )}

      {/* Paste authorization code (PKCE with platform redirect) */}
      {pendingExchange && (
        <Section label="Paste Authorization Code">
          <div className="rounded border border-blue-700/50 bg-blue-500/10 px-2 py-1.5">
            <div className="text-[11px] text-blue-300 mb-1">{pendingExchange.account}</div>
            <div className="flex gap-1.5">
              <input
                className="flex-1 rounded border border-[#2a2a2a] bg-[#0d1117] px-2 py-1 text-[11px] font-mono text-zinc-300 placeholder-zinc-600 outline-none focus:border-zinc-500"
                placeholder="Paste code here"
                value={exchangeCode}
                onChange={(e) => setExchangeCode(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") handleExchange();
                }}
              />
              <ActionBtn variant="success" onClick={handleExchange}>
                Submit
              </ActionBtn>
            </div>
          </div>
        </Section>
      )}

      {/* Account list */}
      <Section
        label="Accounts"
        headerRight={
          <button
            type="button"
            className="text-[10px] text-zinc-500 hover:text-zinc-300"
            onClick={() => setShowForm((v) => !v)}
          >
            {showForm ? "Cancel" : "+ Add"}
          </button>
        }
      >
        {accounts.length === 0 && !showForm && (
          <div className="py-2 text-center text-[11px] text-zinc-500">No accounts configured</div>
        )}
        {accounts.map((acct) => (
          <div
            key={acct.name}
            className="mb-1.5 flex items-center gap-2 rounded bg-[#1c2128] px-2 py-1.5"
          >
            <div className="min-w-0 flex-1">
              <div className="flex items-center gap-1.5">
                <span className="truncate text-[11px] font-mono text-zinc-300">{acct.name}</span>
                <span className="shrink-0 rounded bg-[#2a2a2a] px-1 py-px text-[9px] uppercase text-zinc-500">
                  {acct.provider}
                </span>
              </div>
              <div className="flex items-center gap-1.5 mt-0.5">
                <StatusBadge status={acct.status} />
                {acct.expires_in_secs != null && (
                  <span className="text-[10px] text-zinc-500">
                    {formatExpiry(acct.expires_in_secs)}
                  </span>
                )}
              </div>
            </div>
            <div className="flex shrink-0 gap-1">
              {acct.status === "expired" && acct.reauth && (
                <ActionBtn variant="warn" onClick={() => handleReauth(acct.name)}>
                  Reauth
                </ActionBtn>
              )}
              {acct.status === "healthy" && (
                <ActionBtn onClick={() => handleDistribute(acct.name)}>Push</ActionBtn>
              )}
            </div>
          </div>
        ))}

        {/* Add account form */}
        {showForm && (
          <div className="mt-2 rounded border border-[#2a2a2a] bg-[#1c2128] p-2">
            <div className="mb-1.5 flex gap-2">
              <input
                className="flex-1 rounded border border-[#2a2a2a] bg-[#0d1117] px-2 py-1 text-[11px] font-mono text-zinc-300 placeholder-zinc-600 outline-none focus:border-zinc-500"
                placeholder="Account name"
                value={formName}
                onChange={(e) => setFormName(e.target.value)}
              />
              <select
                className="rounded border border-[#2a2a2a] bg-[#0d1117] px-1.5 py-1 text-[11px] font-mono text-zinc-300 outline-none"
                value={formProvider}
                onChange={(e) => {
                  setFormProvider(e.target.value);
                  setFormEnvKey(envKeysForProvider(e.target.value)[0]);
                }}
              >
                <option value="claude">claude</option>
                <option value="openai">openai</option>
                <option value="gemini">gemini</option>
                <option value="other">other</option>
              </select>
            </div>
            <select
              className="mb-1.5 w-full rounded border border-[#2a2a2a] bg-[#0d1117] px-2 py-1 text-[11px] font-mono text-zinc-300 outline-none"
              value={formEnvKey}
              onChange={(e) => setFormEnvKey(e.target.value)}
            >
              {envKeysForProvider(formProvider).map((key) => (
                <option key={key} value={key}>
                  {key}
                </option>
              ))}
            </select>
            <input
              className="mb-1.5 w-full rounded border border-[#2a2a2a] bg-[#0d1117] px-2 py-1 text-[11px] font-mono text-zinc-300 placeholder-zinc-600 outline-none focus:border-zinc-500"
              placeholder="Token (optional)"
              type="password"
              value={formToken}
              onChange={(e) => setFormToken(e.target.value)}
            />
            <ActionBtn
              variant="success"
              onClick={handleAddAccount}
              className={formSubmitting ? "opacity-50" : ""}
            >
              {formSubmitting ? "Adding..." : "Add Account"}
            </ActionBtn>
          </div>
        )}
      </Section>

      <ResultDisplay result={result} />
    </div>
  );
}
