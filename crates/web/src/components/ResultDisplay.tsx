// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

import type { ApiResult } from "@/lib/types";

export function showResult(res: ApiResult): { ok: boolean; text: string } {
  const display = res.json ? JSON.stringify(res.json) : res.text;
  return { ok: res.ok, text: `${res.status}: ${display}` };
}

export function ResultDisplay({ result }: { result: { ok: boolean; text: string } | null }) {
  if (!result) return null;
  return (
    <div
      className={`mt-1 max-h-10 overflow-y-auto break-all text-[10px] ${result.ok ? "text-green-400" : "text-red-400"}`}
    >
      {result.text}
    </div>
  );
}
