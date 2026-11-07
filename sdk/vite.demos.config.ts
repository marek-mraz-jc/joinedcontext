import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { sampleOverlay, sdkAlias } from "./vite.config";

// The App templates' live demos (T-3306, AP-141): every sample in one bundle under
// `/templates/`, which the Portal embeds and serves at `/templates/{name}/` (AP-12's CSP).
export default defineConfig({
  root: fileURLToPath(new URL("./demos", import.meta.url)),
  base: "/templates/",
  plugins: [react(), sampleOverlay],
  resolve: { alias: sdkAlias },
  build: { outDir: "../dist/demos", emptyOutDir: true, target: "es2022", assetsInlineLimit: 64 * 1024 },
  worker: { format: "es" },
});
