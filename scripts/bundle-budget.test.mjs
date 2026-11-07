// node --test scripts/bundle-budget.test.mjs
import { test } from "node:test";
import assert from "node:assert/strict";
import { closure, entryScripts, staticImports, strayWorkers, weightKb } from "./bundle-budget.mjs";

test("the entry and its preloads are what every page loads, a lazy chunk is not", () => {
  const html = `<script type="module" crossorigin src="/assets/index-abc.js"></script>
<link rel="modulepreload" crossorigin href="/assets/vendor-def.js">
<link rel="stylesheet" href="/assets/index.css">`;
  assert.deepEqual(entryScripts(html), ["assets/index-abc.js", "assets/vendor-def.js"]);
});

test("the weight is gzipped kilobytes", () => {
  assert.equal(weightKb(Buffer.alloc(0)), 0);
  assert.ok(weightKb(Buffer.from("a".repeat(200_000))) < 5, "a repeated byte compresses away");
});

test("a route's chunks are its static imports, followed through; a lazy import is not one", () => {
  const files = {
    "assets/Page-a.js": 'import{t as e}from"./ui-b.js";import"./side-c.js";const l=()=>import("./Lazy-d.js");',
    "assets/ui-b.js": 'import{x}from "./jsx-e.js";',
    "assets/side-c.js": 'import{t}from"./ui-b.js";',
    "assets/jsx-e.js": "export const x=1;",
  };
  assert.deepEqual(staticImports(files["assets/Page-a.js"]), ["assets/ui-b.js", "assets/side-c.js"]);
  assert.deepEqual(
    [...closure("assets/Page-a.js", (chunk) => files[chunk])].sort(),
    ["assets/Page-a.js", "assets/jsx-e.js", "assets/side-c.js", "assets/ui-b.js"],
  );
});

test("only the editor and map workers ship; a language service worker is stray", () => {
  const assets = [
    "editor.worker-DKXjC3Lf.js",
    "maplibre-gl-worker-AbPoOmO0.js",
    "ts.worker-Dj4vg72D.js",
    "json.worker-izi4BK3w.js",
    "workers-BgUmImN5.js",
    "MonacoSourceView-Cil9iIqt.js",
  ];
  assert.deepEqual(strayWorkers(assets), ["ts.worker-Dj4vg72D.js", "json.worker-izi4BK3w.js"]);
});
