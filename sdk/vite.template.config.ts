import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { sdkAlias } from "./vite.config";

// The template (SDK-19) built against this checkout's SDK instead of a published package, so CI
// proves the files a run starts from compile and bundle with the SDK they ship beside.
export default defineConfig({
  root: fileURLToPath(new URL("./template", import.meta.url)),
  plugins: [react()],
  resolve: { alias: sdkAlias },
  build: { outDir: "dist", emptyOutDir: true },
  worker: { format: "es" },
});
