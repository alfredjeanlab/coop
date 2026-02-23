// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

import { useEffect, useRef, useState } from "react";

/** Run `fn` once during the first render. */
export function useInit(fn: () => void) {
  const ran = useRef(false);
  if (!ran.current) {
    ran.current = true;
    fn();
  }
}

/** Call `fn` every `delay` ms while `delay` is non-null. Calls immediately on start. */
export function useInterval(fn: () => void, delay: number | null) {
  const fnRef = useRef(fn);
  fnRef.current = fn;
  useEffect(() => {
    if (delay === null) return;
    fnRef.current();
    const id = setInterval(() => fnRef.current(), delay);
    return () => clearInterval(id);
  }, [delay]);
}

export function useLocalStorage<T>(
  key: string,
  defaultValue: T,
): [T, (value: T | ((prev: T) => T)) => void] {
  const [state, setState] = useState<T>(() => {
    const stored = localStorage.getItem(key);
    if (stored === null) return defaultValue;
    try {
      return JSON.parse(stored) as T;
    } catch {
      return defaultValue;
    }
  });

  const setValue = (value: T | ((prev: T) => T)) => {
    const next = typeof value === "function" ? (value as (prev: T) => T)(state) : value;
    setState(next);
    localStorage.setItem(key, JSON.stringify(next));
  };

  return [state, setValue];
}
