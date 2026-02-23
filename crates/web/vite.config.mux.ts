// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { viteSingleFile } from "vite-plugin-singlefile";
import path from "path";

export default defineConfig({
  root: "src/roots/mux",
  plugins: [react(), tailwindcss(), viteSingleFile()],
  resolve: {
    alias: { "@": path.resolve(__dirname, "src") },
  },
  build: {
    outDir: "../../../dist/mux",
    emptyOutDir: true,
  },
  server: {
    proxy: {
      "/api": "http://localhost:7070",
      "/ws": { target: "ws://localhost:7070", ws: true },
    },
  },
});
