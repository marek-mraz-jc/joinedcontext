import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

// The template's config (sdk/template/vite.config.ts) with the WebAssembly worker of T-3327: the
// worker is an ES module, and `wasm/pkg` (written by wasm-bindgen before `vite build`) is part of
// the App's own bundle, its `.wasm` emitted as a hashed asset beside the scripts.
export default defineConfig({
  base: "/",
  plugins: [react()],
  // One React: the SDK linked from a checkout carries its own (the lane installs it packed).
  resolve: { dedupe: ["react", "react-dom"] },
  worker: { format: "es" },
  // The module is an asset: its URL in the worker, its bytes in the test (`?inline`).
  assetsInclude: ["**/*.wasm"],
  test: {
    globals: true,
    environment: "jsdom",
    setupFiles: ["./test-setup.ts"],
    include: ["src/**/*.test.{ts,tsx}"],
    server: { deps: { inline: ["@joinedcontext/sdk"] } },
    // Every source file counts, tested or not; the entry is one startApp call, and wasm-bindgen's
    // glue is not the example's code (its Rust has cargo llvm-cov, T-3416).
    coverage: { include: ["src/**"], exclude: ["src/main.tsx", "src/**/*.test.*", "wasm/pkg/**"] },
  },
});
