import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Served under /apps/air-weather-explorer/, so every asset is addressed relative to the page.
export default defineConfig({
  base: "./",
  plugins: [react()],
  // In the portal repository CI links `@joinedcontext/sdk` from `sdk/`, which carries its own
  // `react`; deduping keeps one copy. Installed from the package there is one anyway.
  resolve: {
    dedupe: ["react", "react-dom", "react/jsx-runtime"],
  },
  // MapLibre's worker and the planner's are ES modules; the statistics' loads the WebAssembly
  // module from the bundle, under a hashed name beside it.
  worker: { format: "es" },
  test: {
    globals: true,
    environment: "jsdom",
    setupFiles: ["./test-setup.ts"],
    include: ["src/**/*.test.{ts,tsx}"],
    // wasm-bindgen's generated glue is not the App's code; the Rust behind it has its own
    // coverage (cargo llvm-cov in wasm/, T-3373).
    coverage: { exclude: ["wasm/pkg/**"] },
  },
});
