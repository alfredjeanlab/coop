// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

interface OAuthToastProps {
  url: string;
  onDismiss: () => void;
}

export function OAuthToast({ url, onDismiss }: OAuthToastProps) {
  return (
    <div className="fixed top-4 right-4 z-[200] flex items-center gap-3 rounded-lg border border-purple-500/50 bg-[#1a1a2e] px-4 py-3 shadow-lg">
      <span className="text-sm text-purple-300">OAuth login required</span>
      <button
        type="button"
        className="rounded bg-purple-600 px-3 py-1 text-sm text-white hover:bg-purple-500"
        onClick={() => {
          window.open(url, "_blank");
          onDismiss();
        }}
      >
        Open in new tab
      </button>
      <button
        type="button"
        className="px-0.5 text-zinc-500 hover:text-zinc-300"
        onClick={onDismiss}
        title="Dismiss"
      >
        <svg
          width="14"
          height="14"
          viewBox="0 0 14 14"
          fill="none"
          stroke="currentColor"
          strokeWidth="2"
          strokeLinecap="round"
        >
          <title>Dismiss</title>
          <line x1="3" y1="3" x2="11" y2="11" />
          <line x1="11" y1="3" x2="3" y2="11" />
        </svg>
      </button>
    </div>
  );
}
