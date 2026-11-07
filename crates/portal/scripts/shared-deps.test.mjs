// node --test scripts/shared-deps.test.mjs
// The UI links the SDK's sources (link:../sdk), and each resolves its packages from its own
// node_modules. A package both name at two versions lands in ui/dist twice (T-5514: maplibre-gl
// 6.7.0 and 6.9.0 cost the grid and the map page ~250 KB gz each).
// React is not listed: resolve.dedupe in ui/vite.config.ts bundles the UI's copy for both, and
// moving the UI to the SDK's React 19.3.0 adds 8 KB gz to every page (entry budget 154, T-5514).
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const SHARED = ["maplibre-gl"];

// The versions of `name` a pnpm lockfile resolves: its `  name@version:` keys under packages.
export function lockedVersions(lock, name) {
  const escaped = name.replace(/[.*+?^${}()|[\]\\/]/g, "\\$&");
  const key = new RegExp(`^  '?${escaped}@([^(:'\\s]+)`, "gm");
  return [...new Set([...lock.matchAll(key)].map((m) => m[1]))].sort();
}

export function mismatches(uiLock, sdkLock, names) {
  return names
    .map((name) => ({ name, ui: lockedVersions(uiLock, name), sdk: lockedVersions(sdkLock, name) }))
    .filter(({ ui, sdk }) => ui.length !== 1 || ui.join() !== sdk.join());
}

test("a lockfile's versions are its package keys, a dependent's peer suffix is not one", () => {
  const lock = `packages:

  maplibre-gl@6.7.0:
    resolution: {integrity: sha512-x}

  maplibre-gl-extra@1.0.0:
    resolution: {integrity: sha512-y}

  react-dom@19.3.0(react@19.3.0):
    dependencies:
      react: 19.3.0
`;
  assert.deepEqual(lockedVersions(lock, "maplibre-gl"), ["6.7.0"]);
  assert.deepEqual(lockedVersions(lock, "react-dom"), ["19.3.0"]);
  assert.deepEqual(lockedVersions(lock, "react"), []);
});

test("two versions of one shared package are a mismatch", () => {
  const ui = "  maplibre-gl@6.7.0:\n";
  const sdk = "  maplibre-gl@6.9.0:\n";
  assert.deepEqual(mismatches(ui, sdk, ["maplibre-gl"]), [
    { name: "maplibre-gl", ui: ["6.7.0"], sdk: ["6.9.0"] },
  ]);
  assert.deepEqual(mismatches(sdk, sdk, ["maplibre-gl"]), []);
});

test("the UI and the SDK lock one version of every package both bundle", () => {
  const ui = readFileSync(new URL("../ui/pnpm-lock.yaml", import.meta.url), "utf8");
  const sdk = readFileSync(new URL("../sdk/pnpm-lock.yaml", import.meta.url), "utf8");
  const uiPkg = JSON.parse(readFileSync(new URL("../ui/package.json", import.meta.url), "utf8"));
  const sdkPkg = JSON.parse(readFileSync(new URL("../sdk/package.json", import.meta.url), "utf8"));
  const names = (pkg) => Object.keys({ ...pkg.dependencies, ...pkg.devDependencies, ...pkg.peerDependencies });
  const both = SHARED.filter((n) => names(uiPkg).includes(n) && names(sdkPkg).includes(n));
  assert.deepEqual(both, SHARED, "each shared package is named by both package.json files");
  assert.deepEqual(mismatches(ui, sdk, both), [], "set ui/package.json to the SDK's exact version");
});
