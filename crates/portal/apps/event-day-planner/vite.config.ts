import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Served under /apps/event-day-planner/, so every asset is addressed relative to the page.
export default defineConfig({
  base: "./",
  plugins: [react()],
  // In the portal repository CI links `@joinedcontext/sdk` from `sdk/`, which carries its own
  // `react`; deduping keeps one copy. Installed from the package there is one anyway.
  resolve: {
    dedupe: ["react", "react-dom", "react/jsx-runtime"],
  },
  // MapLibre's worker and the planner's are ES modules; the day planner's loads the WebAssembly
  // module from the bundle, under a hashed name beside it.
  worker: { format: "es" },
  test: {
    globals: true,
    environment: "jsdom",
    setupFiles: ["./test-setup.ts"],
    include: ["src/**/*.test.{ts,tsx}"],
    // The wasm-bindgen glue is generated; the planner's own code is measured by its Rust coverage
    // (cargo llvm-cov in wasm/, T-3373), the fixtures are test data.
    coverage: { exclude: ["wasm/pkg/**", "src/fixtures/**"] },
  },
});
