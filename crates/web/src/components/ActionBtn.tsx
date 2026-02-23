// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

const variantStyles = {
  danger: "border-red-800 text-red-400 hover:border-red-400 hover:text-red-300",
  success: "border-green-800 text-green-400 hover:border-green-400 hover:text-green-300",
  warn: "border-amber-700 text-amber-400 hover:border-amber-400 hover:text-amber-300",
  default: "border-zinc-600 text-zinc-400 hover:border-zinc-500 hover:text-white",
};

interface ActionBtnProps {
  children: React.ReactNode;
  variant?: "success" | "danger" | "warn";
  dashed?: boolean;
  onClick?: () => void;
  className?: string;
}

export function ActionBtn({ children, variant, dashed, onClick, className }: ActionBtnProps) {
  return (
    <button
      type="button"
      className={`whitespace-nowrap rounded bg-[#2a2a2a] px-2.5 py-0.5 font-mono text-[11px] transition-colors active:bg-[#333] ${variantStyles[variant ?? "default"]} ${dashed ? "border-dashed opacity-85" : "border"} ${className || ""}`}
      onClick={onClick}
    >
      {children}
    </button>
  );
}
