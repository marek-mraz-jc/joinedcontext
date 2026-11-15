import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

export default defineConfig({
  // An App is the whole of its own host, `{name}.apps.{domain}` (AP-133), so its assets live at
  // the root, and a deep link such as `/stations/5` still finds `/assets/…`, which a relative
  // base would look for under `/stations/`.
  base: "/",
  plugins: [react()],
  // In the portal repository CI links `@joinedcontext/sdk` from `sdk/`, which carries its own
  // `react`; deduping keeps one copy. Installed from the package there is one anyway.
  resolve: {
    dedupe: ["react", "react-dom", "react/jsx-runtime"],
  },
  // MapLibre's worker is an ES module importing the library's shared chunk (map-worker.ts).
  worker: { format: "es" },
  test: {
    globals: true,
    environment: "jsdom",
    setupFiles: ["./test-setup.ts"],
    include: ["src/**/*.test.{ts,tsx}"],
    // The published SDK imports its map's stylesheet; inlined, Vite handles the CSS that Node
    // cannot import (the build lane installs the packed SDK, not its sources, ADR-N-026).
    server: { deps: { inline: ["@joinedcontext/sdk"] } },
  },
});
