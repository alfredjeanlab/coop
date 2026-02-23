// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

interface SectionProps {
  label: React.ReactNode;
  headerRight?: React.ReactNode;
  children: React.ReactNode;
}

export function Section({ label, headerRight, children }: SectionProps) {
  return (
    <div className="border-b border-[#2a2a2a] p-2">
      <div className="mb-1 flex items-center justify-between text-[10px] font-semibold uppercase tracking-wide text-zinc-600">
        <span>{label}</span>
        {headerRight}
      </div>
      {children}
    </div>
  );
}
