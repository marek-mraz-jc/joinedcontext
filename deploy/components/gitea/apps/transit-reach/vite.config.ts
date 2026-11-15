import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Served under /apps/transit-reach/, so every asset, the WebAssembly module included, is addressed
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
    // The module's bindings are generated; its Rust is measured by cargo llvm-cov (T-3373).
    coverage: { exclude: ["wasm/pkg/**"] },
  },
});
