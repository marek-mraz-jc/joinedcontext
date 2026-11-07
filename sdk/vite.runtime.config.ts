import { readFileSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { join } from "node:path";
import { defineConfig } from "vite";
import type { Plugin } from "vite";
import react from "@vitejs/plugin-react";

// The SDK runtime of the code preview (Architecture/20 §5, SDK-16): one ES module per import
// name SDK-12 lets interface code use, plus the chunks they share, so React is one instance
// for the application and the SDK. The Portal puts every file into the preview document's
// import map as a `data:` module; a `data:` module has no address to resolve `./chunk.js`
// against, so every import between these files is rewritten to its import-map name and
// `runtime.json` lists which name each file answers to.

const OUT = "dist/runtime";
const PREFIX = "@jc/runtime/";

/** Import name → where it comes from. React's packages are CommonJS: see `entryCode`. */
const NAMES: Record<string, string> = {
  react: "react",
  "react/jsx-runtime": "react/jsx-runtime",
  "react-dom/client": "react-dom/client",
  echarts: "echarts",
  recharts: "recharts",
  "maplibre-gl": "maplibre-gl",
  "@deck.gl/core": "@deck.gl/core",
  "@deck.gl/layers": "@deck.gl/layers",
  "@deck.gl/aggregation-layers": "@deck.gl/aggregation-layers",
  "@deck.gl/mapbox": "@deck.gl/mapbox",
  "@joinedcontext/sdk": "/src/sdk/index.ts",
  "@joinedcontext/sdk/grid": "/src/sdk/grid.ts",
};
const COMMONJS = new Set(["react", "react/jsx-runtime", "react-dom/client"]);

const fileOf = (name: string): string => `${name.replace(/^@/, "").replace(/[/.]/g, "-")}.js`;
const VIRTUAL = "\0jc-runtime:";

/** `export *` carries no names out of a CommonJS module, so they are listed one by one. */
function entryCode(name: string): string {
  const source = JSON.stringify(NAMES[name]);
  // The SDK's stylesheet is imported by `main.tsx`, which the Portal strips: it rides in sdk.css.
  if (name === "@joinedcontext/sdk") return `import "/src/sdk/style.css";\nexport * from ${source};`;
  if (!COMMONJS.has(name)) return `export * from ${source};`;
  const keys = Object.keys(createRequire(import.meta.url)(name)).filter((k) => k !== "default" && /^[A-Za-z_$][\w$]*$/.test(k));
  return `import * as m from ${source};\nconst d = m.default ?? m;\nexport default d;\nexport const { ${keys.join(", ")} } = d;`;
}

function runtime(): Plugin {
  return {
    name: "jc-runtime",
    resolveId: (id) => (id.startsWith(VIRTUAL) ? id : null),
    load: (id) => (id.startsWith(VIRTUAL) ? entryCode(id.slice(VIRTUAL.length)) : null),
    writeBundle(options, bundle) {
      const dir = options.dir ?? OUT;
      const names: Record<string, string> = {};
      const entries = new Map(Object.keys(NAMES).map((name) => [fileOf(name), name]));
      for (const file of Object.keys(bundle)) {
        if (file.endsWith(".js")) names[entries.get(file) ?? PREFIX + file] = file;
      }
      const specifier = new Map(Object.entries(names).map(([name, file]) => [file, name]));
      for (const [file, output] of Object.entries(bundle)) {
        if (output.type !== "chunk") continue;
        let code = readFileSync(join(dir, file), "utf8");
        for (const dep of [...output.imports, ...output.dynamicImports]) {
          const to = JSON.stringify(specifier.get(dep));
          for (const quote of [`"`, `'`, "`"]) code = code.replaceAll(`${quote}./${dep}${quote}`, to);
        }
        if (/(?:from|import)\s*\(?\s*["'`]\.\.?\//.test(code)) {
          throw new Error(`${file} still imports by relative path`);
        }
        writeFileSync(join(dir, file), code);
      }
      writeFileSync(join(dir, "runtime.json"), `${JSON.stringify({ names }, null, 2)}\n`);
    },
  };
}

export default defineConfig({
  logLevel: "warn",
  plugins: [react(), runtime()],
  define: { "process.env.NODE_ENV": JSON.stringify("production") },
  build: {
    outDir: OUT,
    emptyOutDir: true,
    minify: true,
    cssCodeSplit: false,
    // The typeface rides inside sdk.css: a preview frame's policy allows only `data:` fonts.
    assetsInlineLimit: 64 * 1024,
    // Vite's preloader resolves dependencies against `import.meta.url`, which a `data:` module lacks.
    modulePreload: false,
    rollupOptions: {
      input: Object.fromEntries(Object.keys(NAMES).map((name) => [fileOf(name).slice(0, -3), VIRTUAL + name])),
      // An entry keeps every export: application code may import any of them.
      preserveEntrySignatures: "strict",
      output: {
        format: "es",
        entryFileNames: "[name].js",
        chunkFileNames: "[name].js",
        assetFileNames: "sdk.[ext]",
      },
    },
  },
});
