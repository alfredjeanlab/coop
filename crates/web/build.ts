// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

import { $ } from "bun";
import { rename, mkdir } from "fs/promises";
import path from "path";

const root = import.meta.dirname;
const dist = path.join(root, "dist");

await mkdir(dist, { recursive: true });

console.log("Building terminal...");
await $`bunx vite build --config vite.config.terminal.ts`.cwd(root);
await rename(
  path.join(dist, "terminal", "index.html"),
  path.join(dist, "terminal.html"),
);
await $`rm -rf ${path.join(dist, "terminal")}`;

console.log("Building mux...");
await $`bunx vite build --config vite.config.mux.ts`.cwd(root);
await rename(
  path.join(dist, "mux", "index.html"),
  path.join(dist, "mux.html"),
);
await $`rm -rf ${path.join(dist, "mux")}`;

console.log("Done. Output:");
console.log("  dist/terminal.html");
console.log("  dist/mux.html");
