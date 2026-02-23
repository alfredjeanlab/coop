// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

import { useRef } from "react";
import type { ApiResult } from "@/lib/types";

function getToken(): string | null {
  return new URLSearchParams(window.location.search).get("token");
}

function authHeaders(): Record<string, string> {
  const token = getToken();
  const headers: Record<string, string> = { "Content-Type": "application/json" };
  if (token) headers["Authorization"] = `Bearer ${token}`;
  return headers;
}

async function request(method: string, path: string, body?: unknown): Promise<ApiResult> {
  const res = await fetch(`${location.origin}${path}`, {
    method,
    headers: authHeaders(),
    body: body !== undefined ? JSON.stringify(body) : undefined,
  });
  const text = await res.text();
  let json: unknown;
  try {
    json = JSON.parse(text);
  } catch {
    json = null;
  }
  return { ok: res.ok, status: res.status, json, text };
}

export function useApiClient() {
  // Stable refs so callbacks never change identity
  const apiGet = useRef((path: string) => request("GET", path)).current;
  const apiPost = useRef((path: string, body?: unknown) => request("POST", path, body)).current;
  const apiPut = useRef((path: string, body?: unknown) => request("PUT", path, body)).current;
  const apiDelete = useRef((path: string) => request("DELETE", path)).current;

  return { apiGet, apiPost, apiPut, apiDelete };
}

// Non-hook versions for use outside React components
export const apiGet = (path: string) => request("GET", path);
export const apiPost = (path: string, body?: unknown) => request("POST", path, body);
export const apiPut = (path: string, body?: unknown) => request("PUT", path, body);
export const apiDelete = (path: string) => request("DELETE", path);
