import { existsSync } from "node:fs";
import { dirname, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig, type Plugin } from "vite";
import react from "@vitejs/plugin-react";

const sdk = (file: string): string => fileURLToPath(new URL(`./src/sdk/${file}`, import.meta.url));

/** `@joinedcontext/sdk` and its entry points resolved to this checkout, for the template's tests and build. */
export const sdkAlias = [
  { find: /^@joinedcontext\/sdk$/, replacement: sdk("index.ts") },
  { find: /^@joinedcontext\/sdk\/(grid|server|testing)$/, replacement: sdk("$1.ts") },
  { find: /^@joinedcontext\/sdk\/style\.css$/, replacement: sdk("style.css") },
];

const TEMPLATE = fileURLToPath(new URL("./template", import.meta.url));
const EXTENSIONS = ["", ".ts", ".tsx", ".js", ".json", "/index.ts", "/index.tsx"];

/**
 * A sample is the files it changes over the template (samples/README.md, T-2778), the way a
 * model's answer sits over it: a relative import of a file the sample does not hold is the
 * template's file at the same place.
 */
export const sampleOverlay: Plugin = {
  name: "jc-sample-overlay",
  enforce: "pre",
  async resolveId(source, importer) {
    if (!importer || !source.startsWith(".")) return null;
    const at = /[\\/]samples[\\/][^\\/]+[\\/]/.exec(importer);
    if (!at) return null;
    const target = resolve(dirname(importer), source);
    if (EXTENSIONS.some((extension) => existsSync(target + extension))) return null;
    const inTemplate = resolve(TEMPLATE, relative(importer.slice(0, at.index + at[0].length), target));
    return this.resolve(inTemplate, importer, { skipSelf: true });
  },
};

// One script and one stylesheet with fixed names: the Portal inlines both into the preview
// document, which is the only way a bundle reaches a sandboxed frame that has no session to
// fetch assets with (Architecture/19 §1.2). No hashes, no code splitting, no base path.
export default defineConfig({
  base: "./",
  plugins: [react(), sampleOverlay],
  resolve: { alias: sdkAlias },
  build: {
    cssCodeSplit: false,
    // The typeface rides inside kit.css: the kit document's policy allows only `data:` fonts.
    assetsInlineLimit: 64 * 1024,
    rollupOptions: {
      output: {
        entryFileNames: "kit.js",
        chunkFileNames: "kit-[name].js",
        assetFileNames: "kit.[ext]",
        manualChunks: () => "kit",
      },
    },
  },
  test: {
    globals: true,
    environment: "jsdom",
    // Vitest forks one worker per core; five parallel full runs on the shared 16-core host drove load to 78
    // (T-4979). Outside CI cap at 3, VITEST_MAX_WORKERS overrides; CI keeps vitest's default.
    maxWorkers: Number(process.env.VITEST_MAX_WORKERS) || (process.env.CI ? undefined : 3),
    setupFiles: ["./tests/setup.ts"],
    include: ["tests/**/*.test.ts", "tests/**/*.test.tsx", "template/src/**/*.test.{ts,tsx}",
      "template-wasm/src/**/*.test.{ts,tsx}", "template/functions/**/*.test.ts", "samples/*/src/**/*.test.{ts,tsx}",
      "samples/*/functions/**/*.test.ts"],
  },
});
