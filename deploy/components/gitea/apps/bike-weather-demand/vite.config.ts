import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Served under /apps/bike-weather-demand/, so every asset, the WebAssembly module included, is addressed
// relative to the page; the module is a hashed file of the bundle (AP-142).
export default defineConfig({
  base: "./",
  plugins: [react()],
  resolve: {
    dedupe: ["react", "react-dom", "react/jsx-runtime"],
  },
  // MapLibre's worker and the analysis worker are ES modules.
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
