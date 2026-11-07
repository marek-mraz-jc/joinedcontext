// Before vite reads it: pnpm sets `NODE_ENV=development` for a `run` script, and
// `@vitejs/plugin-react` picks the JSX runtime from it — without this the published package would
// ship `react/jsx-dev-runtime`, which warns on every render and pulls development React into an
// application's build. `mode` and `define` are both too late to change that.
process.env.NODE_ENV = "production";

import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

/**
 * The published `@joinedcontext/sdk` (SDK-17, AP-11): the entry points a generated
 * application imports, built as ES modules beside their type declarations, with the stylesheet
 * and the tokens copied as they are.
 *
 * A published application is built in CI from its committed project, so the package it installs
 * has to be a library: its own `tsc -b` would otherwise typecheck this checkout's source with
 * this checkout's devDependencies, which a consumer does not have. React and the map and chart
 * libraries stay external — the application pins the same versions and one copy of React is the
 * only copy that works. The type declarations beside them come from `tsconfig.package.json`,
 * because `tsc` already emits them and a plugin for it would be a dependency for nothing.
 */
const index = fileURLToPath(new URL("./src/sdk/index.ts", import.meta.url));

/**
 * `./style.css` is the build's one CSS asset, and the entries import only the components' CSS:
 * `src/sdk/style.css` (MapLibre's sheet, the typeface, the attribution underline) is what an App
 * imports as `@joinedcontext/sdk/style.css`, so it goes in first (T-3578). Only here: importing it
 * from `index.ts` would restyle the Portal, which links the SDK's source.
 */
const sdkStylesheet = {
  name: "jc-sdk-stylesheet",
  transform: (code: string, id: string) => (id === index ? `import "./style.css";\n${code}` : null),
};

export default defineConfig({
  mode: "production",
  plugins: [react(), sdkStylesheet],
  define: { "process.env.NODE_ENV": JSON.stringify("production") },
  build: {
    outDir: "dist/package",
    emptyOutDir: true,
    cssCodeSplit: false,
    // The typeface rides inside the stylesheet, as it does in the kit build.
    assetsInlineLimit: 64 * 1024,
    lib: {
      entry: {
        index,
        grid: fileURLToPath(new URL("./src/sdk/grid.ts", import.meta.url)),
        server: fileURLToPath(new URL("./src/sdk/server.ts", import.meta.url)),
        testing: fileURLToPath(new URL("./src/sdk/testing.ts", import.meta.url)),
        responsive: fileURLToPath(new URL("./src/sdk/responsive.ts", import.meta.url)),
      },
      formats: ["es"],
    },
    rollupOptions: {
      external: (id) =>
        !id.startsWith(".") && !id.startsWith("/") && !id.startsWith("\0") && id !== "vite",
      output: {
        entryFileNames: "[name].js",
        chunkFileNames: "chunks/[name]-[hash].js",
        assetFileNames: "style.css",
      },
    },
  },
});
