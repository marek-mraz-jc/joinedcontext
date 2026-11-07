import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Served under /apps/helsinki-events/, so every asset is addressed relative to the page.
export default defineConfig({
  base: "./",
  plugins: [react()],
  // In the portal repository CI links `@joinedcontext/sdk` from `sdk/`, which carries its own
  // `react`; deduping keeps one copy. Installed from the package there is one anyway.
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
