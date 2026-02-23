// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

interface DropOverlayProps {
  active: boolean;
}

export function DropOverlay({ active }: DropOverlayProps) {
  if (!active) return null;

  return (
    <div className="fixed inset-0 z-[1000] flex items-center justify-center border-3 border-dashed border-blue-400 bg-zinc-900/85">
      <span className="font-mono text-xl font-semibold tracking-wide text-blue-400">
        Drop file to upload
      </span>
    </div>
  );
}
