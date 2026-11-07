import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  // The Portal serves the bundle under `/apps/bbsk-ukazovatele/` (AP-14), so every asset is
  // addressed relative to the index and never from the host root.
  base: "./",
  // `@joinedcontext/sdk` is linked from this repository and carries its own `react` in
  // `sdk/node_modules`; without deduping, its components render against a second copy of React
  // whose hook dispatcher is null and every hook throws. A published app resolves the SDK from
  // the registry and has one copy, so this is the price of building it beside its source.
  // The SDK's district map and `map-worker.ts` must hand the worker to one copy of MapLibre.
  resolve: {
    dedupe: ["react", "react-dom", "react/jsx-runtime", "maplibre-gl"],
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
