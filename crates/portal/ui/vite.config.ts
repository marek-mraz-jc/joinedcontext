/// <reference types="vitest/config" />
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";
import type { Plugin } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

const PUBLIC_LOCALE = "\0public-locale:";

/**
 * `./locales/<lng>.json?public` is that locale's groups the public pages use (T-5122), listed in
 * src/locales/public-groups.json; the rest of the file loads with the full Portal (src/i18n.ts).
 * Vitest gets the whole file, so a test reads every word at once as it did before the split.
 */
function publicLocales(): Plugin {
  const groups: string[] = JSON.parse(readFileSync(new URL("./src/locales/public-groups.json", import.meta.url), "utf8"));
  return {
    name: "public-locales",
    enforce: "pre",
    async resolveId(source, importer) {
      if (!source.endsWith(".json?public")) return null;
      const file = await this.resolve(source.slice(0, -"?public".length), importer);
      // The id must not end in .json, or Vite's own JSON plugin parses the module this one writes.
      return file ? `${PUBLIC_LOCALE}${file.id}.js` : null;
    },
    load(id) {
      if (!id.startsWith(PUBLIC_LOCALE)) return null;
      const all: Record<string, unknown> = JSON.parse(readFileSync(id.slice(PUBLIC_LOCALE.length, -".js".length), "utf8"));
      const kept = process.env.VITEST ? all : Object.fromEntries(groups.filter((group) => group in all).map((group) => [group, all[group]]));
      return `export default ${JSON.stringify(kept)};`;
    },
  };
}

/**
 * The Latin face of Inter, asked for by the page itself rather than found in the stylesheet once
 * the app draws: text drawn in the fallback font and then swapped re-wraps and moves what follows
 * (T-5165, Lighthouse layout shifts on the catalogue pages). Latin-ext loads as before, when a page
 * uses it.
 */
function preloadLatinFont(): Plugin {
  return {
    name: "preload-latin-font",
    transformIndexHtml(_html, context) {
      const font = Object.keys(context.bundle ?? {}).find((name) => /\/inter-latin-wght-normal-[^/]+\.woff2$/.test(name));
      return font
        ? [{ tag: "link", attrs: { rel: "preload", as: "font", type: "font/woff2", href: `/${font}`, crossorigin: "" }, injectTo: "head" }]
        : [];
    },
  };
}

export default defineConfig({
  plugins: [publicLocales(), preloadLatinFont(), react(), tailwindcss()],
  // The Portal writes each answer's CSP nonce over this slot (src/csp.rs NONCE_SLOT, T-5828): on
  // the shell's tags and in <meta property="csp-nonce"> for src/cspNonce.ts.
  html: { cspNonce: "__JC_CSP_NONCE__" },
  // `@joinedcontext/sdk` is linked from this repository (T-1439) and carries its own `react` in
  // `sdk/node_modules`. Without deduping, a component from it renders against a second copy of
  // React whose hook dispatcher is null — every hook inside the grid throws "invalid hook call"
  // (measured: `Cannot read properties of null (reading 'useMemo')`).
  resolve: {
    dedupe: ["react", "react-dom", "react/jsx-runtime", "maplibre-gl"],
  },
  build: {
    outDir: "dist",
    rolldownOptions: {
      // The shared controls (src/components/ui) run nothing when imported, so a page that takes a
      // Button from the barrel does not also download the menus and dialogs beside it (T-5122:
      // the public entry carried Radix and floating-ui, about 50 KB gzipped, for a sign-in button).
      // tests/ui_pure.test.ts keeps that folder free of top-level statements.
      treeshake: { moduleSideEffects: (id: string) => !id.includes("/src/components/ui/") },
      // MapLibre (about 250 KB gzipped) gets a chunk of its own, which the entity grid and the
      // map view share, so neither carries it and a page with both loads it once (T-5514).
      output: {
        codeSplitting: { groups: [{ name: "maplibre-gl", test: /[\\/]node_modules[\\/]\.pnpm[\\/]maplibre-gl@/ }] },
      },
    },
  },
  // The policy the Rust server sends (src/server.rs), so the e2e journeys run under the same
  // rules as dev: a library that needs `eval` fails here before it fails there.
  preview: {
    headers: {
      "Content-Security-Policy":
        "default-src 'self'; base-uri 'self'; object-src 'none'; frame-ancestors 'self'; " +
        "form-action 'self'; img-src 'self' data: blob:; style-src 'self' 'unsafe-inline'; " +
        "font-src 'self' data:; connect-src 'self'; worker-src 'self' blob:",
    },
  },
  server: {
    // `@joinedcontext/sdk` is `link:../sdk` and its package entry is TypeScript source, not a
    // built bundle, so Vite serves it over `/@fs/` — which `server.fs.allow` guards. Vite 8
    // derives that allow-list from the nearest package root, `ui/`, where Vite 5 walked up to
    // the `.git` directory; `../sdk` became unreachable and every test file that imports the
    // SDK failed to collect with "Cannot find module /@fs/.../sdk/src/…", 86 of 142 of them.
    // The sibling package is named on its own rather than the repository root, so nothing else
    // outside `ui/` is served.
    fs: { allow: [".", "../sdk"] },
    port: 5173,
    proxy: {
      "/api": "http://localhost:8080",
    },
  },
  test: {
    environment: "jsdom",
    // Transforming this suite's modules takes about 7 s and was redone on every invocation; the
    // fast lane runs a handful of files at a time, dozens of times a day. Cached on disk under
    // node_modules/.vite, keyed by content, so a changed file still transforms.
    fsModuleCache: true,
    // i18next-icu's ESM build default-imports intl-messageformat, whose CJS entry has no
    // __esModule marker — Node's interop then hands back the namespace object and
    // `new IntlMessageFormat()` throws. Inlining makes vitest resolve both through Vite,
    // which picks the ESM build the browser bundle already uses.
    server: { deps: { inline: ["i18next-icu", "intl-messageformat"] } },
    setupFiles: ["./tests/setup.ts"],
    // The form engine loads lazily in the Portal (T-3280); a test reads a form the moment it opens,
    // as a person does once the engine is loaded, so the dialogs import it directly here.
    // `tests/lazy_schema_form.test.tsx` imports `SchemaFormLoader` itself and covers the wait.
    alias: [
      {
        find: /^.*\/forms\/LazySchemaForm$/,
        replacement: fileURLToPath(new URL("./src/components/forms/SchemaForm.tsx", import.meta.url)),
      },
      {
        find: /^.*\/layout\/LazyShell$/,
        replacement: fileURLToPath(new URL("./src/components/layout/Shell.tsx", import.meta.url)),
      },
    ],
    globals: true,
    css: true,
    // Vitest forks one worker per core; five parallel full runs on the shared 16-core host drove load to 78
    // (T-4979). Outside CI cap at 3, VITEST_MAX_WORKERS overrides; CI keeps vitest's default.
    maxWorkers: Number(process.env.VITEST_MAX_WORKERS) || (process.env.CI ? undefined : 3),
    include: ["tests/**/*.test.{ts,tsx}"],
    // Rendering the whole app (router, i18n, rjsf) takes seconds on a shared CPU, and the
    // 5 s default would kill a test in the middle of an assertion that is merely slow.
    testTimeout: 20_000,
    // Coverage is measured, never targeted (T-2136): the thresholds below are what the suite
    // reached on 2026-09-20, so the number can rise and cannot fall. `ci-full` runs
    // `pnpm vitest run --coverage`; the fast lane does not, because instrumenting every module
    // costs more than the lane's whole budget.
    coverage: {
      provider: "v8",
      include: ["src/**/*.{ts,tsx}"],
      // The bundle entry, the generated API types and the CSS-only modules have nothing to
      // cover: `main.tsx` mounts React and `schema.d.ts` is types.
      exclude: ["src/main.tsx", "src/api/schema.d.ts", "src/**/*.d.ts"],
      reporter: ["text-summary", "json-summary"],
      reportsDirectory: "coverage",
      // Measured on 2026-09-20 over the whole suite (217 files, 2 136 cases): lines 91.17,
      // statements 90.71, functions 88.91, branches 81.68. Each threshold is that number floored
      // to the whole percent, so a single line does not flap the build and the figure can only be
      // raised by hand when the suite has earned it.
      thresholds: { lines: 91, statements: 90, functions: 88, branches: 81 },
    },
  },
});
