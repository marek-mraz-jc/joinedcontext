import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  // The Portal serves the bundle under `/apps/banskabystrica-ovzdusie/` (AP-14), so every asset is
  // addressed relative to the index and never from the host root.
  base: "./",
  // `@joinedcontext/sdk` is linked from this repository and carries its own `react` in
  // `sdk/node_modules`; without deduping, its components render against a second copy of React
  // whose hook dispatcher is null and every hook throws. A published app resolves the SDK from
  // the registry and has one copy, so this is the price of building it beside its source.
  resolve: {
    dedupe: ["react", "react-dom", "react/jsx-runtime"],
  },
  // MapLibre's worker is an ES module importing the library's shared chunk; keep it one.
  worker: { format: "es" },
  test: {
    globals: true,
    environment: "jsdom",
    setupFiles: ["./test-setup.ts"],
    include: ["src/**/*.test.{ts,tsx}"],
  },
});
