import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Served under /apps/news-topics/, so every asset is addressed relative to the page.
export default defineConfig({
  base: "./",
  plugins: [react()],
  // In the portal repository CI links `@joinedcontext/sdk` from `sdk/`, which carries its own
  // `react`; deduping keeps one copy. Installed from the package there is one anyway.
  resolve: {
    dedupe: ["react", "react-dom", "react/jsx-runtime"],
  },
  // The clustering worker is an ES module importing the WASM bundle; keep it one.
  worker: { format: "es" },
  // The module is an asset: its URL in the worker, its bytes in the test (`?inline`).
  assetsInclude: ["**/*.wasm"],
  test: {
    globals: true,
    environment: "jsdom",
    setupFiles: ["./test-setup.ts"],
    include: ["src/**/*.test.{ts,tsx}"],
    // The wasm-bindgen glue is generated and measured by the crate's own coverage (cargo llvm-cov in
    // wasm/, T-3373); the fixtures are test data.
    coverage: { exclude: ["wasm/pkg/**", "src/fixtures/**"] },
  },
});
