import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

// The catalogue's live data widget (T-5082): EndpointDataView as one classic script and one
// stylesheet with fixed names, `explorer.js` and `explorer.css`, which the deployment's viz host
// serves beside explore.html (scripts/viz-explorer-bundle.sh). Its policy allows scripts from the
// host alone and no `eval`, so nothing loads later: no code splitting, every asset inlined.
export default defineConfig({
  logLevel: "warn",
  plugins: [react(), tailwindcss()],
  resolve: {
    dedupe: ["react", "react-dom", "react/jsx-runtime"],
    alias: [
      // The catalogue's policy loads no font and the dataset page has its own: system fonts here.
      { find: /^\.\/fonts\.css$/, replacement: fileURLToPath(new URL("./src/explorer/no-map.css", import.meta.url)) },
      { find: /^\.\.?\/(\.\.\/)*i18n$/, replacement: fileURLToPath(new URL("./src/explorer/i18n.ts", import.meta.url)) },
      { find: /^maplibre-gl\/dist\/maplibre-gl\.css$/, replacement: fileURLToPath(new URL("./src/explorer/no-map.css", import.meta.url)) },
      { find: /^(maplibre-gl|terra-draw|terra-draw-maplibre-gl-adapter)$/, replacement: fileURLToPath(new URL("./src/explorer/no-map.ts", import.meta.url)) },
    ],
  },
  // Objects, not JSON.parse strings: only the locale namespaces src/explorer/i18n.ts names ship.
  json: { stringify: false },
  define: { "process.env.NODE_ENV": JSON.stringify("production") },
  publicDir: false,
  build: {
    outDir: "dist-explorer",
    emptyOutDir: true,
    minify: true,
    assetsInlineLimit: 64 * 1024,
    lib: {
      entry: "src/explorer/main.tsx",
      name: "jcExplorer",
      formats: ["iife"],
      fileName: () => "explorer.js",
      cssFileName: "explorer",
    },
  },
});
