import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Served under /apps/kpi-forecast/, so every asset, the WebAssembly module included, is addressed
// relative to the page; the module is a hashed file of the bundle (AP-142).
export default defineConfig({
  base: "./",
  plugins: [react()],
  resolve: {
    dedupe: ["react", "react-dom", "react/jsx-runtime"],
  },
  // The analysis worker is an ES module.
  worker: { format: "es" },
  test: {
    globals: true,
    environment: "jsdom",
    setupFiles: ["./test-setup.ts"],
    include: ["src/**/*.test.{ts,tsx}"],
    // wasm-bindgen's glue is generated; the Rust behind it is measured by cargo llvm-cov in wasm/
    // (T-3373), as air-weather-explorer does.
    coverage: { exclude: ["wasm/pkg/**", "src/test-analyser.ts", "src/fixtures/**"] },
  },
});
