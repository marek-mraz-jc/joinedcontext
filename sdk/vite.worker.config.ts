import { defineConfig } from "vite";

// The second bundle: MapLibre's worker with its shared module folded in, one file, no imports
// left, so a blob URL can carry it inside the preview document (src/map-worker.ts).
export default defineConfig({
  build: {
    outDir: "dist",
    emptyOutDir: false,
    lib: {
      // The library's own worker module is the entry: an entry is never shaken away, while a
      // side-effect import of it is, because the package declares no side effects.
      entry: "node_modules/maplibre-gl/dist/maplibre-gl-worker.mjs",
      formats: ["es"],
      fileName: () => "kit-worker.js",
    },
    rollupOptions: { output: { inlineDynamicImports: true } },
  },
});
