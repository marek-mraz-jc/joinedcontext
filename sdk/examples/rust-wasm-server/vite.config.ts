import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

// The interface of the server WASM example (T-3341): a static bundle, its server the component in
// joinedcontext-platform examples/apps/rust-wasm-server, reached at /apps/{name}/api.
export default defineConfig({
  base: "/",
  plugins: [react()],
  test: { globals: true, environment: "jsdom", setupFiles: ["./test-setup.ts"], include: ["src/**/*.test.{ts,tsx}"] },
});
