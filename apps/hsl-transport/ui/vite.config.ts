import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// `base` is a build-time constant here and JC_BASE_PATH is the run-time one; they are the
// same value because the reconciler serves the app under exactly one path (AP-14).
export default defineConfig({
  base: process.env.JC_BASE_PATH ?? "/",
  plugins: [react()],
  // `@joinedcontext/sdk` is linked from this repository and carries its own `react` in
  // `sdk/node_modules`; deduping keeps one copy. Built from its own repository there is one anyway.
  resolve: {
    dedupe: ["react", "react-dom", "react/jsx-runtime"],
  },
  // MapLibre's worker is an ES module importing the library's shared chunk; keep it one.
  worker: { format: "es" },
  test: {
    globals: true,
    environment: "jsdom",
    setupFiles: ["./tests/setup.ts"],
    // The attribution test reads the stylesheet itself (`index.css?raw`), which vitest empties
    // unless the file is processed (T-3045).
    css: { include: [/index\.css/] },
    include: ["tests/**/*.test.tsx"],
  },
});
