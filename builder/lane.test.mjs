// node --test builder/lane.test.mjs (vite from sdk/node_modules for the bundle test)
import { strict as assert } from "node:assert";
import { chmodSync, existsSync, lstatSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, readlinkSync, rmSync, statSync, symlinkSync, utimesSync, writeFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { test } from "node:test";
import { createHash } from "node:crypto";
import { gzipSync } from "node:zlib";
import * as lane from "./lane.mjs";
import { appOf, artifactScope, browserChecks, bundleFunctions, cratesOf, functionEntries, isHttpComponent, lockManifest, serverRefusals, missingFromStore, ociImage, outputsOf, propose, refusedDependencies, sbomOf, SDK_SPEC, stampedFiles, uploadArtifact, withBuild, writeLayout } from "./lane.mjs";

const template = { dependencies: { react: "^19", "@joinedcontext/sdk": "0.1.0" }, devDependencies: { vite: "^8" } };

test("the template's own packages pass, with any version asked for", () => {
  assert.deepEqual(refusedDependencies({ dependencies: { react: "18" }, devDependencies: { vite: "*" } }, template), []);
  assert.deepEqual(refusedDependencies({}, template), []);
});

test("a package the template does not install is refused by name", () => {
  assert.deepEqual(
    refusedDependencies({ dependencies: { react: "^19", "left-pad": "1" }, devDependencies: { "evil-postinstall": "1" } }, template),
    ["evil-postinstall", "left-pad"],
  );
});

// SDK-12, AP-100, AP-105 (T-2617): a sample the forge bootstrap pushes to its own repository
// builds there, so the lane's package check passes on it against the real template: the page of a
// static app at its root, the interface of a fullstack app in ui/. Its Playwright flows are why
// the template installs @playwright/test; the lane never runs them.
test("every sample application passes the lane's package check against the template", () => {
  const root = new URL("..", import.meta.url).pathname;
  const real = JSON.parse(readFileSync(join(root, "sdk/template/package.json"), "utf8"));
  let seen = 0;
  for (const name of readdirSync(join(root, "apps"))) {
    // Only an app whose source is a forge repository builds on the lane.
    const app = join(root, "apps", name, "app.yaml");
    if (!existsSync(app) || !/^  source:\n    git:$/m.test(readFileSync(app, "utf8"))) continue;
    for (const manifest of [join(root, "apps", name, "package.json"), join(root, "apps", name, "ui/package.json")]) {
      if (!existsSync(manifest)) continue;
      seen += 1;
      const pkg = JSON.parse(readFileSync(manifest, "utf8"));
      assert.deepEqual(refusedDependencies(pkg, real), [], manifest);
      // T-2912: the repository is the app's folder alone, so the SDK is named as the template names
      // it; a link out of the tree resolves in this monorepo and nowhere in the forge.
      if (pkg.dependencies?.["@joinedcontext/sdk"] !== undefined) {
        assert.equal(pkg.dependencies["@joinedcontext/sdk"], real.dependencies["@joinedcontext/sdk"], manifest);
      }
      const lock = join(manifest, "..", "pnpm-lock.yaml");
      if (existsSync(lock)) assert.doesNotMatch(readFileSync(lock, "utf8"), /(link|file):\.\.\//, lock);
    }
  }
  assert.ok(seen >= 4, `only ${seen} sample packages found`);
});

// AP-127 (T-2724): the runner image's store holds every package its template's lockfile names
// for the runner's platform; another platform's binaries and the SDK directory are not asked for.
const LOCK = `lockfileVersion: '9.0'

importers:

  .:
    dependencies:
      react:
        specifier: 19.3.0
        version: 19.3.0

packages:

  '@deck.gl/core@9.4.0':
    resolution: {integrity: sha512-x}

  '@joinedcontext/sdk@file:..':
    resolution: {directory: .., type: directory}

  '@rolldown/binding-darwin-arm64@1.2.11':
    resolution: {integrity: sha512-x}
    cpu: [arm64]
    os: [darwin]

  lightningcss-linux-x64-musl@1.33.0:
    resolution: {integrity: sha512-x}
    cpu: [x64]
    os: [linux]
    libc: [musl]

  fsevents-free@1.0.0:
    resolution: {integrity: sha512-x}
    os: ['!darwin']

  react@19.3.0:
    resolution: {integrity: sha512-x}

snapshots:

  left-pad@1.0.0: {}
`;
const linux = { os: "linux", cpu: "x64", libc: "glibc" };

test("a store holding every package of the lockfile for its platform passes", () => {
  const folders = ["@deck.gl+core@9.4.0_@luma.gl+core@9.4.2", "react@19.3.0", "fsevents-free@1.0.0", "lock.yaml"];
  assert.deepEqual(missingFromStore(LOCK, folders, linux), []);
});

test("a package the lockfile names and the store lacks is named, peers or not", () => {
  assert.deepEqual(missingFromStore(LOCK, ["react@19.3.0"], linux), ["@deck.gl/core@9.4.0", "fsevents-free@1.0.0"]);
  // Another version of the same package is not the one the lockfile pins.
  assert.deepEqual(missingFromStore(LOCK, ["@deck.gl+core@9.4.1", "react@19.3.0", "fsevents-free@1.0.0"], linux), ["@deck.gl/core@9.4.0"]);
});

test("the lockfile's os, cpu and libc decide which binaries a platform's store must hold", () => {
  const all = ["@deck.gl+core@9.4.0", "react@19.3.0", "fsevents-free@1.0.0"];
  assert.deepEqual(missingFromStore(LOCK, all, { os: "linux", cpu: "x64", libc: "musl" }), ["lightningcss-linux-x64-musl@1.33.0"]);
  assert.deepEqual(missingFromStore(LOCK, all, { os: "darwin", cpu: "arm64", libc: "glibc" }), ["@rolldown/binding-darwin-arm64@1.2.11"]);
  assert.deepEqual(missingFromStore("lockfileVersion: '9.0'\n\npackages: {}\n", [], linux), []);
});

test("the template's manifest takes the SDK from the directory above, and nothing else changes", () => {
  const pkg = { name: "jc-app", dependencies: { react: "19.3.0", "@joinedcontext/sdk": "0.1.0" }, devDependencies: { vite: "8.3.0" } };
  assert.deepEqual(lockManifest(pkg), { ...pkg, dependencies: { react: "19.3.0", "@joinedcontext/sdk": SDK_SPEC } });
  assert.equal(pkg.dependencies["@joinedcontext/sdk"], "0.1.0", "the input is not changed");
  assert.throws(() => lockManifest({ dependencies: { react: "19.3.0" } }), /does not depend on @joinedcontext\/sdk/);
});

/** `{ name: { specifier, version } }` of a pnpm v9 lockfile's root importer. */
function rootImporter(lock) {
  const found = {};
  let inRoot = false;
  let name = "";
  for (const line of lock.split("\n")) {
    if (/^\S/.test(line)) inRoot = false;
    if (line === "  .:") inRoot = true;
    else if (/^  \S/.test(line)) inRoot = false;
    if (!inRoot) continue;
    const dep = /^      '?([^':]+)'?:$/.exec(line);
    if (dep) found[(name = dep[1])] = {};
    const field = /^        (specifier|version): (.+)$/.exec(line);
    if (field && name) found[name][field[1]] = field[2];
  }
  return found;
}

// AP-126 (T-2724): every package of the template pinned to one version, its committed lockfile
// written from exactly that manifest (the runner image installs it frozen), and each pin the
// version the SDK's own lockfile tests with, so an app builds with what the SDK was tested on.
test("the template pins every package exactly, as its lockfile and the SDK's lockfile do", () => {
  const root = new URL("..", import.meta.url).pathname;
  const pkg = JSON.parse(readFileSync(join(root, "sdk/template/package.json"), "utf8"));
  const wanted = { ...lockManifest(pkg).dependencies, ...pkg.devDependencies };
  for (const [name, version] of Object.entries(wanted)) {
    if (name === "@joinedcontext/sdk") continue;
    assert.match(version, /^\d+\.\d+\.\d+$/, `${name} is pinned to ${version}, not one exact version`);
  }
  const locked = rootImporter(readFileSync(join(root, "sdk/template/pnpm-lock.yaml"), "utf8"));
  assert.deepEqual(
    Object.fromEntries(Object.entries(locked).map(([name, { specifier }]) => [name, specifier]).sort()),
    Object.fromEntries(Object.entries(wanted).sort()),
    "sdk/template/pnpm-lock.yaml is not written from sdk/template/package.json: run `node builder/lane.mjs lock-manifest sdk/template`, `pnpm install --lockfile-only --ignore-scripts` there, and restore package.json",
  );
  const sdk = rootImporter(readFileSync(join(root, "sdk/pnpm-lock.yaml"), "utf8"));
  let shared = 0;
  for (const [name, version] of Object.entries(wanted)) {
    if (!sdk[name]) continue;
    shared += 1;
    assert.equal(sdk[name].version.split("(")[0], version, `the SDK tests with ${name} ${sdk[name].version.split("(")[0]}, the template pins ${version}`);
  }
  assert.ok(shared >= 15, `only ${shared} packages shared with the SDK`);
});

// SDK-24, AP-82 (T-2649). On the lane an app's packages are links into the template's store,
// outside the app, so vitest externalises the packed SDK and Node imports the MapLibre stylesheet
// the SDK imports: "Unknown file extension .css". The lane runs the tests with its own config,
// which is the app's with the SDK inlined, so an app whose vite.config lacks that still passes.
test("an app's tests pass on the lane when the linked SDK imports a stylesheet", () => {
  const root = new URL("..", import.meta.url).pathname;
  const store = mkdtempSync(join(tmpdir(), "lane-store-"));
  mkdirSync(join(store, "node_modules/@joinedcontext/sdk"), { recursive: true });
  for (const name of ["vitest", "vite", "maplibre-gl", "jsdom", ".bin"]) {
    symlinkSync(join(root, "sdk/node_modules", name), join(store, "node_modules", name));
  }
  // The packed SDK's shape: an ES module that imports the map's stylesheet.
  writeFileSync(join(store, "node_modules/@joinedcontext/sdk/package.json"), JSON.stringify({ name: "@joinedcontext/sdk", type: "module", exports: { ".": "./index.js" } }));
  writeFileSync(join(store, "node_modules/@joinedcontext/sdk/index.js"), 'import "maplibre-gl/dist/maplibre-gl.css";\nexport const sdk = "packed";\n');
  const dir = app({
    "vite.config.ts": 'import { defineConfig } from "vite";\nexport default defineConfig({ test: { include: ["src/**/*.test.ts"] } });\n',
    "src/map.test.ts": 'import { expect, test } from "vitest";\nimport { sdk } from "@joinedcontext/sdk";\ntest("the SDK loads", () => { expect(sdk).toBe("packed"); });\n',
  });
  mkdirSync(join(dir, "node_modules"));
  for (const name of readdirSync(join(store, "node_modules"))) {
    symlinkSync(join(store, "node_modules", name), join(dir, "node_modules", name));
  }
  const vitest = (...args) => spawnSync(join(dir, "node_modules/.bin/vitest"), ["run", ...args], { cwd: dir, encoding: "utf8" });

  const own = vitest();
  assert.notEqual(own.status, 0, "the app's own config was expected to hit the linked stylesheet");
  assert.match(own.stdout + own.stderr, /Unknown file extension "\.css"/);

  const config = lane.vitestConfig(dir);
  const laned = vitest("--config", config);
  assert.equal(laned.status, 0, laned.stdout + laned.stderr);
  // The app's own settings still hold: its include found the one test.
  assert.match(laned.stdout, /1 passed/);
});

function app(files) {
  const dir = mkdtempSync(join(tmpdir(), "lane-"));
  for (const [path, text] of Object.entries(files)) {
    mkdirSync(join(dir, path, ".."), { recursive: true });
    writeFileSync(join(dir, path), text);
  }
  return dir;
}

test("functions are the .ts files of functions/, tests and other files left out", () => {
  const dir = app({ "functions/summary.ts": "", "functions/bike-stats.ts": "", "functions/summary.test.ts": "", "functions/notes.md": "" });
  assert.deepEqual(functionEntries(join(dir, "functions")), ["bike-stats", "summary"]);
  assert.deepEqual(functionEntries(join(dir, "nothing-here")), []);
});

test("a function name the route cannot carry fails the build", () => {
  const dir = app({ "functions/Summary.ts": "" });
  assert.throws(() => functionEntries(join(dir, "functions")), /function name/);
});

test("the bundle exports each function by name and leaves the SDK server to the runtime", async () => {
  const dir = app({
    "functions/summary.ts": 'import { json } from "@joinedcontext/sdk/server";\nconst f = (r: unknown) => json({ ok: r !== null });\nexport default f;\n',
    "functions/bike-stats.ts": "export default (): number => 2;\n",
    "functions/summary.test.ts": "throw new Error('a test is never bundled');\n",
  });
  const out = join(dir, "dist");
  assert.equal(await bundleFunctions(dir, out), true);
  const bundle = readFileSync(join(out, "functions.js"), "utf8");
  assert.match(bundle, /from "@joinedcontext\/sdk\/server"/);
  assert.match(bundle, /as "bike-stats"/);
  assert.match(bundle, /as summary|summary as|summary\b.*export|export \{[^}]*summary/s);
  assert.doesNotMatch(bundle, /never bundled/);
});

test("an app with no functions gets no bundle", async () => {
  const dir = app({ "index.html": "<p>hi</p>" });
  assert.equal(await bundleFunctions(dir, join(dir, "dist")), false);
});

// AP-75: the repository name carries the project and the application, split at the one `_`.
test("an application repository names its project and application", () => {
  assert.deepEqual(appOf("joinedcontext/helsinki_city-bikes"), { project: "helsinki", app: "city-bikes" });
  assert.deepEqual(appOf("helsinki_bikes"), { project: "helsinki", app: "bikes" });
  for (const name of ["configuration", "_bikes", "helsinki_", "Helsinki_bikes", "helsinki_bi_kes"]) {
    assert.throws(() => appOf(`joinedcontext/${name}`), /\{project\}_\{app\}/, name);
  }
});

// AP-11, AP-101: the SBOM lists every linked package once, sorted, the same bytes on every build.
test("the SBOM names each package of the store once, scoped ones included", () => {
  const bom = sbomOf(["react@19.2.0", "@deck.gl+core@9.4.0_@luma.gl+core@9.4.2", "react@19.2.0", "lock.yaml", "node_modules"]);
  assert.equal(bom.bomFormat, "CycloneDX");
  assert.deepEqual(
    bom.components.map((c) => c.purl),
    ["pkg:npm/%40deck.gl/core@9.4.0", "pkg:npm/react@19.2.0"],
  );
  assert.equal(JSON.stringify(sbomOf(["b@1", "a@2"])), JSON.stringify(sbomOf(["a@2", "b@1"])));
  assert.deepEqual(sbomOf([]).components, []);
});

const build = {
  digest: `sha256:${"ab".repeat(32)}`,
  commit: "0123456789abcdef0123456789abcdef01234567",
  sdkVersion: "0.4.1",
  builtAt: "2026-09-22T06:00:00Z",
};
const manifest = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "App",
  metadata: { name: "city-bikes", namespace: "helsinki" },
  spec: { kind: "static", lifecycle: "published" },
  status: { phase: "Live", sourceUrl: "https://git.example/x", conditions: [] },
};

// AP-73: the lane writes status.build and nothing else, whatever status the Portal computed.
test("the proposal carries the App as read with status.build alone", () => {
  assert.deepEqual(withBuild(manifest, build).status, { build });
  assert.deepEqual(withBuild(manifest, build).spec, manifest.spec);
  assert.throws(() => withBuild(manifest, { ...build, digest: "sha256:short" }), /sha256 digest/);
  assert.throws(() => withBuild(manifest, { ...build, commit: "main" }), /full commit/);
  assert.throws(() => withBuild({ kind: "Endpoint", metadata: {}, spec: {} }, build), /not answer with an App/);
});

function portal(answers) {
  const calls = [];
  const fetchImpl = async (url, init = {}) => {
    calls.push({ url, method: init.method ?? "GET", headers: init.headers, body: init.body });
    const [status, body] = answers.shift();
    return { ok: status < 300, status, json: async () => body };
  };
  return { calls, fetchImpl };
}

const green = [200, { valid: true, verdict: { ok: true, findings: [] } }];

// AP-80: the lane token travels only as the bearer, to the App's own route, read, checked, then
// proposed (PF-57, T-2636).
test("propose reads the App, checks it and writes it back once with the lane's bearer", async () => {
  const { calls, fetchImpl } = portal([[200, manifest], green, [202, { metadata: { name: "chg-00000042" } }]]);
  const change = await propose("https://portal.example/", "lane-token", "joinedcontext/helsinki_city-bikes", build, fetchImpl);
  assert.equal(change.metadata.name, "chg-00000042");
  assert.deepEqual(calls.map((c) => `${c.method} ${c.url}`), [
    "GET https://portal.example/api/v1/projects/helsinki/apps/city-bikes",
    "PUT https://portal.example/api/v1/projects/helsinki/apps/city-bikes?dryRun=All",
    "PUT https://portal.example/api/v1/projects/helsinki/apps/city-bikes",
  ]);
  assert.equal(calls[2].headers.Authorization, "Bearer lane-token");
  assert.equal(calls[1].body, calls[2].body, "the proposal is the manifest the check judged");
  assert.deepEqual(JSON.parse(calls[2].body).status, { build });
  assert.ok(!calls.some((c) => c.url.includes("lane-token")));
});

// T-2674: a 409 is main moving past the Portal's copy of the App; the lane reads the App again and
// proposes on what it reads now, and gives up with the Portal's reason after eleven tries, 150 s
// of waiting: two of the Portal's 60 s re-reads of main (T-3597).
test("a write main moved past is read again and proposed on the newer App", async () => {
  const newer = { ...manifest, spec: { ...manifest.spec, source: { git: { ref: "b".repeat(40) } } } };
  const moved = [409, { detail: "App 'city-bikes' changed on main after the Portal last read it; read it again" }];
  const waits = [];
  const { calls, fetchImpl } = portal([[200, manifest], green, moved, [200, newer], green, [202, { metadata: { name: "chg-00000043" } }]]);
  const change = await propose("https://portal.example", "lane-token", "joinedcontext/helsinki_city-bikes", build, fetchImpl, async (ms) => waits.push(ms));
  assert.equal(change.metadata.name, "chg-00000043");
  assert.deepEqual(waits, [15_000]);
  assert.deepEqual(JSON.parse(calls[5].body).spec, newer.spec, "the second proposal is the App as main holds it now");

  const answers = [];
  for (let i = 0; i < 11; i += 1) answers.push([200, manifest], green, moved);
  const stuck = portal(answers);
  await assert.rejects(
    propose("https://portal.example", "lane-token", "joinedcontext/helsinki_city-bikes", build, stuck.fetchImpl, async () => {}),
    /answered 409 App 'city-bikes' changed on main/,
  );
  assert.equal(stuck.calls.length, 33, "eleven reads, eleven checks, eleven writes and no more");
});

// T-3597: the Portal re-reads main every 60 s, so a 409 can last two minutes; the check and the
// write alike are tried again until it clears, and past 150 s the Portal's reason is the answer.
test("a 409 on the check or the write that lasts 120 s still ends in a written build", async () => {
  const moved = [409, { detail: "App 'city-bikes' changed on main after the Portal last read it; read it again" }];
  let now = 0;
  const calls = [];
  const fetchImpl = async (url, init = {}) => {
    const method = init.method ?? "GET";
    calls.push(`${method} ${url}`);
    const [status, body] =
      method === "GET" ? [200, manifest] : now < 120_000 ? moved : url.endsWith("?dryRun=All") ? green : [202, { metadata: { name: "chg-00000044" } }];
    return { ok: status < 300, status, json: async () => body };
  };
  const change = await propose("https://portal.example", "lane-token", "joinedcontext/helsinki_city-bikes", build, fetchImpl, async (ms) => {
    now += ms;
  });
  assert.equal(change.metadata.name, "chg-00000044");
  assert.equal(now, 120_000);
  assert.ok(calls.every((c) => !c.includes("lane-token")));

  now = 0;
  const forever = async (url, init = {}) => {
    const method = init.method ?? "GET";
    const [status, body] = method === "GET" ? [200, manifest] : url.endsWith("?dryRun=All") ? green : moved;
    return { ok: status < 300, status, json: async () => body };
  };
  await assert.rejects(
    propose("https://portal.example", "lane-token", "joinedcontext/helsinki_city-bikes", build, forever, async (ms) => {
      now += ms;
    }),
    /answered 409 App 'city-bikes' changed on main/,
  );
  assert.equal(now, 150_000, "two of the Portal's re-reads of main, then the Portal's reason");
});

test("a refusal names the App and the Portal's reason, never the token", async () => {
  const { fetchImpl } = portal([[200, manifest], green, [403, { detail: "status.build is written by the build lane" }]]);
  await assert.rejects(
    propose("https://portal.example", "lane-token", "joinedcontext/helsinki_city-bikes", build, fetchImpl),
    (err) => /helsinki\/city-bikes: the Portal answered 403 status.build is written/.test(err.message) && !err.message.includes("lane-token"),
  );
  const missing = portal([[404, null]]);
  await assert.rejects(
    propose("https://portal.example", "t", "joinedcontext/helsinki_gone", build, missing.fetchImpl),
    /cannot read the App helsinki\/gone: the Portal answered 404/,
  );
  assert.equal(missing.calls.length, 1, "nothing is written after a failed read");
});

test("a check that is not green proposes nothing and says why", async () => {
  const red = portal([[200, manifest], [200, { valid: false, verdict: { ok: false, findings: [{ message: "spec.source.git is required" }] } }]]);
  await assert.rejects(
    propose("https://portal.example", "lane-token", "joinedcontext/helsinki_city-bikes", build, red.fetchImpl),
    /check of status.build for helsinki\/city-bikes is not green: spec.source.git is required/,
  );
  assert.equal(red.calls.length, 2, "nothing is proposed after a red check");
  const refused = portal([[200, manifest], [403, { detail: "the build lane writes status.build and nothing else" }]]);
  await assert.rejects(
    propose("https://portal.example", "lane-token", "joinedcontext/helsinki_city-bikes", build, refused.fetchImpl),
    /answered 403 the build lane writes status.build and nothing else/,
  );
  assert.equal(refused.calls.length, 2);
});

function jwt(claims) {
  const part = (value) => Buffer.from(JSON.stringify(value)).toString("base64url");
  return `${part({ alg: "HS256" })}.${part(claims)}.sig`;
}
const RUNTIME = jwt({ scp: "Actions.UploadArtifacts:3:5 Actions.Results:3:5" });

// AP-101: the run and job come from the runtime token the forge issued, nothing else.
test("the artifact scope is the run and job of the runtime token", () => {
  assert.deepEqual(artifactScope(RUNTIME), { workflowRunBackendId: "3", workflowJobRunBackendId: "5" });
  assert.throws(() => artifactScope(jwt({ scp: "Actions.UploadArtifacts:3:5" })), /names no run/);
  assert.throws(() => artifactScope("not-a-jwt"), /names no run/);
  assert.throws(() => artifactScope(undefined), /names no run/);
});

function forge(answers) {
  const calls = [];
  const fetchImpl = async (url, init = {}) => {
    calls.push({ url: String(url), method: init.method, headers: init.headers, body: init.body });
    const [status, body] = answers.shift();
    return { ok: status < 300, status, text: async () => (body === undefined ? "" : JSON.stringify(body)) };
  };
  return { calls, fetchImpl };
}

// AP-101, ADR-N-028: create, append, finalize; the signed address is sent to the runner's forge
// origin, its signature kept, and the hash is the SHA-256 of the bytes uploaded.
test("upload creates, appends to the in-cluster origin, and finalizes with the bytes' hash", async () => {
  const signed = "https://forge.public.example/twirp/api/actions/3/artifacts/9/upload?sig=abc&expires=1";
  const { calls, fetchImpl } = forge([[200, { ok: true, signedUploadUrl: signed }], [201], [200, { ok: true }]]);
  const bytes = Buffer.from("bundle");
  const hash = await uploadArtifact("http://gitea-http:3000/api/actions_pipeline/", RUNTIME, "bundle-abc", bytes, fetchImpl);

  const expected = `sha256:${createHash("sha256").update(bytes).digest("hex")}`;
  assert.equal(hash, expected);
  const service = "http://gitea-http:3000/api/actions_pipeline/twirp/github.actions.results.api.v1.ArtifactService/";
  assert.deepEqual(calls.map((c) => `${c.method} ${c.url}`), [
    `POST ${service}CreateArtifact`,
    "PUT http://gitea-http:3000/twirp/api/actions/3/artifacts/9/upload?sig=abc&expires=1&comp=block",
    `POST ${service}FinalizeArtifact`,
  ]);
  assert.deepEqual(JSON.parse(calls[0].body), { workflowRunBackendId: "3", workflowJobRunBackendId: "5", name: "bundle-abc", version: 4 });
  assert.deepEqual(JSON.parse(calls[2].body), { workflowRunBackendId: "3", workflowJobRunBackendId: "5", name: "bundle-abc", size: "6", hash: expected });
  assert.equal(calls[0].headers.Authorization, `Bearer ${RUNTIME}`);
  assert.equal(calls[1].headers.Authorization, undefined, "the signed address carries its own grant");
});

test("a refused upload names the artifact and stops before finalize", async () => {
  const refused = forge([[401, { msg: "bad token" }]]);
  await assert.rejects(uploadArtifact("http://f/api/actions_pipeline", RUNTIME, "sbom-abc", Buffer.from("x"), refused.fetchImpl), /refused CreateArtifact of sbom-abc: 401/);
  const noAddress = forge([[200, { ok: true }]]);
  await assert.rejects(uploadArtifact("http://f/", RUNTIME, "sbom-abc", Buffer.from("x"), noAddress.fetchImpl), /no upload address for sbom-abc/);
  const put = forge([[200, { signedUploadUrl: "http://f/u?sig=1" }], [500]]);
  await assert.rejects(uploadArtifact("http://f/", RUNTIME, "sbom-abc", Buffer.from("x"), put.fetchImpl), /refused the upload of sbom-abc: 500/);
  assert.equal(put.calls.length, 2, "nothing is finalized after a failed upload");
});

// T-2633: dev's forge has ROOT_URL https://host/git/, so it signs /git/twirp/…; the forge itself
// serves /twirp/… and answered 404 to the prefixed path until the prefix was dropped.
test("upload drops the public ROOT_URL path in front of /twirp/", async () => {
  const signed =
    "https://2.28.67.127.sslip.io/git/twirp/github.actions.results.api.v1.ArtifactService/UploadArtifact?sig=s&expires=1&artifactName=bundle-abc&taskID=9";
  const { calls, fetchImpl } = forge([[200, { ok: true, signedUploadUrl: signed }], [201], [200, { ok: true }]]);
  await uploadArtifact("http://gitea-http.dev.svc:3000/", RUNTIME, "bundle-abc", Buffer.from("b"), fetchImpl);
  assert.equal(
    `${calls[1].method} ${calls[1].url}`,
    "PUT http://gitea-http.dev.svc:3000/twirp/github.actions.results.api.v1.ArtifactService/UploadArtifact?sig=s&expires=1&artifactName=bundle-abc&taskID=9&comp=block",
  );
});

// AP-80: the outputs the propose job reads are the four fields, checked before they are written.
test("the build job's outputs are the four fields of status.build, checked", () => {
  assert.equal(outputsOf(build), `digest=${build.digest}\ncommit=${build.commit}\nsdk-version=${build.sdkVersion}\nbuilt-at=${build.builtAt}\n`);
  assert.throws(() => outputsOf({ ...build, sdkVersion: "0.4.1\ndigest=sha256:evil" }), /SDK version/);
  assert.throws(() => outputsOf({ ...build, builtAt: "yesterday" }), /UTC build time/);
  assert.throws(() => outputsOf({ ...build, commit: "" }), /full commit/);
  assert.throws(() => outputsOf(undefined), /sha256 digest/);
});

// AP-105: the image is one gzip layer, a config that runs /app as uid 65532, and an OCI
// manifest whose digest is the SHA-256 of its bytes; the same layer is the same digest.
test("the image of a layer is a digest-addressed OCI manifest, reproducible", () => {
  const layer = Buffer.from("a tar holding /app");
  const image = ociImage(layer);
  const [config, gz, manifestBytes] = image.blobs;
  const manifest = JSON.parse(manifestBytes);
  const digestOf = (bytes) => `sha256:${createHash("sha256").update(bytes).digest("hex")}`;

  assert.equal(image.digest, digestOf(manifestBytes));
  assert.equal(manifest.mediaType, "application/vnd.oci.image.manifest.v1+json");
  assert.deepEqual(manifest.config, { mediaType: "application/vnd.oci.image.config.v1+json", digest: digestOf(config), size: config.length });
  assert.deepEqual(manifest.layers, [{ mediaType: "application/vnd.oci.image.layer.v1.tar+gzip", digest: digestOf(gz), size: gz.length }]);
  const parsed = JSON.parse(config);
  assert.deepEqual(parsed.config, { Entrypoint: ["/app"], User: "65532:65532", WorkingDir: "/" });
  assert.deepEqual(parsed.rootfs.diff_ids, [digestOf(layer)]);
  assert.ok(!("created" in parsed), "no time in the config");
  assert.equal(JSON.parse(image.index).manifests[0].digest, image.digest);
  assert.equal(ociImage(Buffer.from("a tar holding /app")).digest, image.digest);
  assert.notEqual(ociImage(Buffer.from("another /app")).digest, image.digest);
});

test("the layout holds oci-layout, index.json and each blob under its digest", () => {
  const dir = mkdtempSync(join(tmpdir(), "layout-"));
  const image = ociImage(Buffer.from("x"));
  writeLayout(image, dir);
  assert.equal(readFileSync(join(dir, "oci-layout"), "utf8"), '{"imageLayoutVersion":"1.0.0"}');
  assert.deepEqual(readFileSync(join(dir, "index.json")), image.index);
  for (const blob of image.blobs) {
    const hex = createHash("sha256").update(blob).digest("hex");
    assert.deepEqual(readFileSync(join(dir, "blobs", "sha256", hex)), blob);
  }
});

// AP-105: the SBOM names every registry crate of the lock, and no path crate (the app itself).
test("the crates of a Cargo.lock are SBOM components, registry crates only", () => {
  const lock = [
    "version = 4",
    "",
    "[[package]]",
    'name = "air-quality"',
    'version = "0.1.0"',
    "",
    "[[package]]",
    'name = "axum"',
    'version = "0.8.4"',
    'source = "registry+https://github.com/rust-lang/crates.io-index"',
    'checksum = "abc"',
    "",
    "[[package]]",
    'name = "anyhow"',
    'version = "1.0.98"',
    'source = "registry+https://github.com/rust-lang/crates.io-index"',
    "",
    "[[package]]",
    'name = "from-git"',
    'version = "0.1.0"',
    'source = "git+https://example.org/x#abc"',
  ].join("\n");
  assert.deepEqual(cratesOf(lock), [
    { type: "library", name: "anyhow", version: "1.0.98", purl: "pkg:cargo/anyhow@1.0.98" },
    { type: "library", name: "axum", version: "0.8.4", purl: "pkg:cargo/axum@0.8.4" },
  ]);
  assert.deepEqual(cratesOf(""), []);
});

test("a fullstack build starts from a copy of the precompiled dependencies, times kept (AP-106, T-2794)", () => {
  const dir = mkdtempSync(join(tmpdir(), "lane-seed-"));
  const from = join(dir, "seed");
  mkdirSync(join(from, "release", "deps"), { recursive: true });
  const rlib = join(from, "release", "deps", "libserde-1.rlib");
  writeFileSync(rlib, "compiled");
  const built = new Date("2026-09-01T00:00:00Z");
  utimesSync(rlib, built, built);
  const to = join(dir, "job", "target");
  mkdirSync(to, { recursive: true });
  writeFileSync(join(to, "left-by-an-earlier-job"), "x");

  assert.match(lane.seed(from, to), /are the start of this build/);
  const copied = join(to, "release", "deps", "libserde-1.rlib");
  assert.equal(readFileSync(copied, "utf8"), "compiled");
  // Cargo reads a dependency as fresh by its output's time; a copy stamped now would still be
  // fresh, but a copy older than its dependents would rebuild them.
  assert.equal(statSync(copied).mtime.getTime(), built.getTime());
  assert.equal(existsSync(join(to, "left-by-an-earlier-job")), false, "nothing of an earlier job survives");

  // A copy, not a link: what the build writes never reaches the seed.
  writeFileSync(copied, "rebuilt by the app");
  assert.equal(readFileSync(rlib, "utf8"), "compiled");
});

test("a missing seed, or one that fails to copy, leaves an empty target and never fails the build", () => {
  const dir = mkdtempSync(join(tmpdir(), "lane-seed-"));
  const to = join(dir, "target");
  assert.match(lane.seed(join(dir, "absent"), to), /no precompiled dependencies/);
  assert.deepEqual(readdirSync(to), []);

  // Half a copy is worse than none: a torn artifact would be read as compiled.
  const from = join(dir, "seed");
  mkdirSync(join(from, "a"), { recursive: true });
  writeFileSync(join(from, "a", "first.rlib"), "ok");
  writeFileSync(join(from, "z-unreadable.rlib"), "secret");
  chmodSync(join(from, "z-unreadable.rlib"), 0o000);
  const root = process.getuid?.() === 0;
  const said = lane.seed(from, to);
  if (root) {
    // root reads a mode-000 file, so there is nothing to fail here; the copy is whole.
    assert.match(said, /are the start of this build/);
  } else {
    assert.match(said, /could not be copied .*: every crate is compiled/);
    assert.deepEqual(readdirSync(to), []);
  }
  chmodSync(join(from, "z-unreadable.rlib"), 0o644);
});

/** A seed and a cache folder side by side, the seed holding one compiled dependency. */
function cacheFixture() {
  const dir = mkdtempSync(join(tmpdir(), "lane-cache-"));
  const seedDir = join(dir, "seed");
  mkdirSync(join(seedDir, "release", "deps"), { recursive: true });
  writeFileSync(join(seedDir, "release", "deps", "libserde-1.rlib"), "from the image");
  const cache = join(dir, "cache");
  mkdirSync(cache);
  return { dir, seedDir, cache };
}

test("a second build of the same lock starts from the App's cache, times kept (AP-131, T-2794)", () => {
  const { dir, seedDir, cache } = cacheFixture();
  const first = join(dir, "first", "target");
  assert.match(lane.restore(cache, "k1", seedDir, first), /no complete entry; the precompiled dependencies .* are the start/);
  // The first build compiled the App's own crate and a dependency the seed does not hold.
  mkdirSync(join(first, "release", "deps"), { recursive: true });
  const own = join(first, "release", "deps", "libapp-1.rlib");
  writeFileSync(own, "the app's own");
  const built = new Date("2026-09-20T00:00:00Z");
  utimesSync(own, built, built);
  assert.match(lane.save(first, cache, "k1"), /is the App's build cache/);

  const second = join(dir, "second", "target");
  assert.match(lane.restore(cache, "k1", seedDir, second), /the App's build cache is the start/);
  const again = join(second, "release", "deps", "libapp-1.rlib");
  assert.equal(readFileSync(again, "utf8"), "the app's own");
  assert.equal(statSync(again).mtime.getTime(), built.getTime(), "cargo reads it as compiled");

  // A copy: what the second build writes changes the cache only when it is saved.
  writeFileSync(again, "edited");
  assert.equal(readFileSync(join(cache, "target", "release", "deps", "libapp-1.rlib"), "utf8"), "the app's own");
});

test("another lock or toolchain, a torn entry or no cache at all start from the seed", () => {
  const { dir, seedDir, cache } = cacheFixture();
  const built = join(dir, "built");
  mkdirSync(built);
  writeFileSync(join(built, "stale.rlib"), "for the old lock");
  lane.save(built, cache, "old-lock");

  const to = join(dir, "target");
  assert.match(lane.restore(cache, "new-lock", seedDir, to), /for another Cargo\.lock or toolchain; the precompiled/);
  assert.equal(existsSync(join(to, "stale.rlib")), false);
  assert.equal(existsSync(join(to, "release", "deps", "libserde-1.rlib")), true);

  // A save cut short leaves its entry and no marker: it is never trusted.
  rmSync(join(cache, "target.key"));
  assert.match(lane.restore(cache, "old-lock", seedDir, to), /no complete entry/);
  assert.equal(existsSync(join(to, "stale.rlib")), false);

  // A marker that cannot be read counts as none.
  mkdirSync(join(cache, "target.key"));
  assert.match(lane.restore(cache, "old-lock", seedDir, to), /no complete entry/);

  const nothing = join(dir, "no-cache");
  assert.match(lane.restore(nothing, "k", seedDir, to), /no build cache is mounted/);
  assert.match(lane.save(built, nothing, "k"), /nothing kept/);
  assert.equal(existsSync(nothing), false, "the lane never creates a cache the pod did not mount");
});

test("a save that fails leaves no entry, and the build it follows still succeeds", () => {
  const { dir, cache } = cacheFixture();
  const said = lane.save(join(dir, "no-such-target"), cache, "k");
  assert.match(said, /was not kept .*: the next build starts from the seed/);
  assert.deepEqual(readdirSync(cache), []);
});

test("the lane's command line runs restore and save", () => {
  const { dir, seedDir, cache } = cacheFixture();
  const to = join(dir, "target");
  const run = (...args) => spawnSync(process.execPath, [new URL("./lane.mjs", import.meta.url).pathname, ...args], { encoding: "utf8" });
  const restored = run("restore", cache, "k", seedDir, to);
  assert.equal(restored.status, 0, restored.stderr);
  assert.match(restored.stdout, /no complete entry/);
  const saved = run("save", to, cache, "k");
  assert.equal(saved.status, 0, saved.stderr);
  assert.equal(readFileSync(join(cache, "target.key"), "utf8").trim(), "k");
});

// SDK-38: the run's sandbox writes only plain relative paths of the version, never over the links
// it makes itself, and refuses the whole project before writing any of it.
test("a project path that is absolute, climbs out, hides or names node_modules is refused before anything is written", () => {
  for (const path of ["/etc/passwd", "../outside.ts", "src/../../x.ts", "node_modules/vitest/index.js", ".gitea/workflows/build.yml", "src/.hidden.ts", "src//a.ts", ""]) {
    const dir = mkdtempSync(join(tmpdir(), "jc-project-"));
    assert.throws(() => lane.writeProject({ "src/ok.ts": "export {}", [path]: "x" }, dir), /is not a path a project may hold/, path);
    assert.deepEqual(readdirSync(dir), [], `${path}: nothing written`);
  }
  const dir = mkdtempSync(join(tmpdir(), "jc-project-"));
  assert.throws(() => lane.writeProject({ "src/a.ts": 7 }, dir), /is not text/);
  assert.throws(() => lane.writeProject(["src/a.ts"], dir), /not an object/);
  lane.writeProject({ "src/pages/Map.test.tsx": "a", "package.json": "{}" }, dir);
  assert.equal(readFileSync(join(dir, "src/pages/Map.test.tsx"), "utf8"), "a");
});

test("the summary counts the tests, names each failure by file and full name, and a file that did not load", () => {
  const app = "/tmp/app";
  const report = {
    testResults: [
      {
        name: "/tmp/app/src/pages/AlertDesk.test.tsx",
        status: "failed",
        assertionResults: [
          { status: "passed", fullName: "AlertDesk lists the alerts" },
          { status: "failed", fullName: "AlertDesk opens an alert", failureMessages: ["\u001b[31mTestingLibraryElementError: Unable to find role button\u001b[39m"] },
        ],
      },
      { name: "/tmp/app/functions/summary.test.ts", status: "failed", message: "SyntaxError: Unexpected token", assertionResults: [] },
      { name: "/tmp/app/src/App.test.tsx", status: "passed", assertionResults: [{ status: "passed", fullName: "App renders" }, { status: "skipped", fullName: "later" }] },
    ],
  };
  assert.deepEqual(lane.testSummary(report, app), {
    outcome: "failed",
    passed: 2,
    failed: 2,
    failures: [
      { file: "src/pages/AlertDesk.test.tsx", name: "AlertDesk opens an alert", message: "TestingLibraryElementError: Unable to find role button" },
      { file: "functions/summary.test.ts", name: "(the file did not load)", message: "SyntaxError: Unexpected token" },
    ],
  });
  assert.deepEqual(lane.testSummary({ testResults: [] }, app), { outcome: "passed", passed: 0, failed: 0, failures: [] });
  assert.deepEqual(lane.testSummary(null, app), { outcome: "passed", passed: 0, failed: 0, failures: [] });
});

test("the summary keeps twenty failures and cuts each message", () => {
  const many = Array.from({ length: 30 }, (_, i) => ({ status: "failed", fullName: `t${i}`, failureMessages: ["x".repeat(5000)] }));
  const summary = lane.testSummary({ testResults: [{ name: "/a/b.test.ts", status: "failed", assertionResults: many }] }, "/a");
  assert.equal(summary.failed, 30);
  assert.equal(summary.failures.length, 20);
  assert.equal(summary.failures[0].message.length, 2001);
});

// SDK-38 end to end on the real vitest: a passing and a failing test, a timeout and a project
// that is not one, each one answer the Portal can read.
const store = join(import.meta.dirname, "node_modules");
const project = (files) => {
  const dir = mkdtempSync(join(tmpdir(), "jc-sandbox-"));
  const input = join(dir, "project.json.gz");
  writeFileSync(input, gzipSync(JSON.stringify(files)));
  return { input, work: join(dir, "app") };
};

test("the sandbox runs the version's tests and reports the one that fails", { skip: !existsSync(join(store, ".bin", "vitest")) && "vitest is not linked" }, () => {
  const { input, work } = project({
    "src/sum.test.ts": 'import { expect, test } from "vitest";\ntest("adds", () => expect(1 + 1).toBe(2));\ntest("subtracts", () => expect(2 - 1).toBe(3));\n',
  });
  const result = lane.testProject(input, work, { store });
  assert.equal(result.outcome, "failed", JSON.stringify(result));
  assert.equal(result.passed, 1);
  assert.equal(result.failed, 1);
  assert.equal(result.failures[0].file, "src/sum.test.ts");
  assert.equal(result.failures[0].name, "subtracts");
  assert.match(result.failures[0].message, /expected 1 to be 3/);
});

// T-6429: Publish opened on "97 passed" and the build lane then refused the version. The sandbox
// fails on what the lane fails on: a type error, and an error vitest caught outside any test.
const typed = !existsSync(join(store, ".bin", "tsc")) && "tsc is not linked";
const TSCONFIG = JSON.stringify({ compilerOptions: { strict: true, noEmit: true, module: "ESNext", moduleResolution: "bundler", target: "ES2022", types: [] }, include: ["src/sum.ts"] });
const PASSING = { "src/sum.test.ts": 'import { expect, test } from "vitest";\ntest("adds", () => expect(1 + 1).toBe(2));\n' };

test("the sandbox fails a version the type check refuses and names the line", { skip: typed }, () => {
  const { input, work } = project({ ...PASSING, "tsconfig.json": TSCONFIG, "src/sum.ts": 'export const name = "a";\nexport const size: number = name.category;\n' });
  const result = lane.testProject(input, work, { store });
  assert.equal(result.outcome, "failed", JSON.stringify(result));
  assert.equal(result.passed, 1);
  assert.equal(result.failures[0].file, "src/sum.ts");
  assert.equal(result.failures[0].name, "(type error at line 2)");
  assert.match(result.failures[0].message, /TS2339: Property 'category' does not exist/);
});

test("the sandbox passes a version that type checks", { skip: typed }, () => {
  const { input, work } = project({ ...PASSING, "tsconfig.json": TSCONFIG, "src/sum.ts": "export const size: number = 1;\n" });
  assert.equal(lane.testProject(input, work, { store }).outcome, "passed");
});

test("the sandbox fails a version whose tests pass beside an unhandled rejection", { skip: !existsSync(join(store, ".bin", "vitest")) && "vitest is not linked" }, () => {
  const { input, work } = project({
    "src/sum.test.ts": 'import { expect, test } from "vitest";\ntest("adds", async () => {\n  void Promise.reject(new Error("WebGL2 is required"));\n  await new Promise((done) => setTimeout(done, 50));\n  expect(1).toBe(1);\n});\n',
  });
  const result = lane.testProject(input, work, { store });
  assert.equal(result.outcome, "failed", JSON.stringify(result));
  assert.equal(result.passed, 1);
  assert.equal(result.failures[0].name, "(an error outside the tests)");
  assert.match(result.failures[0].message, /WebGL2 is required/);
});

test("tsc's output is one failure per error, its detail lines kept, and a silent failure still fails", () => {
  const two = lane.typeSummary("\u001b[31msrc/a.tsx(3,1): error TS2322: Type 'a' is not 'b'.\n  Detail line.\nsrc/b.ts(9,2): error TS2304: Cannot find name 'c'.\n", "", 2);
  assert.equal(two.failed, 2);
  assert.equal(two.failures[1].file, "src/b.ts");
  assert.match(two.failures[0].message, /Detail line/);
  assert.equal(lane.typeSummary("", "", 0).outcome, "passed");
  assert.equal(lane.typeSummary("error TS5083: Cannot read file", "", 1).failures[0].name, "(the type check did not run)");
});

test("the sandbox says when the tests pass, and when they could not run", { skip: !existsSync(join(store, ".bin", "vitest")) && "vitest is not linked" }, () => {
  const ok = project({ "src/sum.test.ts": 'import { expect, test } from "vitest";\ntest("adds", () => expect(1 + 1).toBe(2));\n' });
  assert.equal(lane.testProject(ok.input, ok.work, { store }).outcome, "passed");

  const slow = project({ "src/sum.test.ts": 'import { test } from "vitest";\ntest("never", () => new Promise(() => {}), 60_000);\n' });
  const late = lane.testProject(slow.input, slow.work, { store, timeoutMs: 3_000 });
  assert.equal(late.outcome, "error");
  assert.match(late.reason, /did not finish within 3 s/);

  const bad = project({ "../escape.ts": "x" });
  assert.match(lane.testProject(bad.input, bad.work, { store }).reason, /is not a path a project may hold/);

  const dir = mkdtempSync(join(tmpdir(), "jc-sandbox-"));
  writeFileSync(join(dir, "project.json.gz"), "not gzip");
  assert.match(lane.testProject(join(dir, "project.json.gz"), join(dir, "app"), { store }).reason, /could not be read/);
});

// T-2827: the App's own browser checks run over the bundle this build made, and their verdict is the build's.
function appWithChecks({ e2e = true } = {}) {
  const root = mkdtempSync(join(tmpdir(), "lane-browser-"));
  const app = join(root, "app");
  const bundle = join(root, "bundle");
  mkdirSync(app);
  mkdirSync(bundle);
  writeFileSync(join(bundle, "index.html"), "<!doctype html>");
  if (e2e) {
    mkdirSync(join(app, "e2e"));
    writeFileSync(join(app, "e2e", "app.spec.ts"), "");
    writeFileSync(join(app, "playwright.config.ts"), "");
  }
  return { root, app, bundle, report: join(root, "report") };
}

test("an App without e2e/ is not checked, says so and is not blocked", () => {
  const { app, bundle, report } = appWithChecks({ e2e: false });
  const calls = [];
  assert.equal(browserChecks(app, bundle, report, (...args) => calls.push(args)), 0);
  assert.deepEqual(calls, []);
  assert.equal(existsSync(join(app, "dist")), false);
});

test("the checks run the App's Playwright over the bundle just built, never a dist/ the repository holds", () => {
  const { app, bundle, report } = appWithChecks();
  mkdirSync(join(app, "dist"));
  writeFileSync(join(app, "dist", "index.html"), "a stale bundle the repository committed");
  const calls = [];
  const status = browserChecks(app, bundle, report, (command, args, options) => {
    calls.push({ command, args, options });
    return { status: 0 };
  });
  assert.equal(status, 0);
  assert.ok(lstatSync(join(app, "dist")).isSymbolicLink());
  assert.equal(readlinkSync(join(app, "dist")), bundle);
  assert.equal(calls.length, 1);
  assert.equal(calls[0].command, join(app, "node_modules", ".bin", "playwright"));
  assert.deepEqual(calls[0].args, ["test", "--reporter=line,html"]);
  assert.equal(calls[0].options.cwd, app);
  assert.equal(calls[0].options.env.PLAYWRIGHT_HTML_OUTPUT_DIR, report);
  assert.equal(calls[0].options.env.CI, "true");
});

test("a red check fails the build, and a browser that does not start fails it too", () => {
  const { app, bundle, report } = appWithChecks();
  assert.equal(browserChecks(app, bundle, report, () => ({ status: 1 })), 1);
  assert.equal(browserChecks(app, bundle, report, () => ({ status: null, error: new Error("spawn playwright ENOENT") })), 1);
  assert.equal(browserChecks(app, bundle, report, () => ({ status: null, signal: "SIGKILL" })), 1);
});

// AP-151: a `wasm` App's server must be a component exporting wasi:http/incoming-handler.
test("a component is told apart from a core module, by its own export section", () => {
  const name = [...Buffer.from("wasi:http/incoming-handler@0.2.6")];
  const section = (id, body) => [id, body.length, ...body];
  const component = (...sections) => Uint8Array.from([0x00, 0x61, 0x73, 0x6d, 0x0d, 0x00, 0x01, 0x00, ...sections.flat()]);
  assert.equal(isHttpComponent(component(section(11, [1, 0, name.length, ...name, 1, 0]))), true);
  // The name in a custom section, or a component that exports something else, is no handler.
  assert.equal(isHttpComponent(component(section(0, [4, ...Buffer.from("note"), ...name]))), false);
  assert.equal(isHttpComponent(component(section(11, [1, 0, 3, ...Buffer.from("run"), 1, 0]))), false);
  // A core module (version 1, layer 0) holding the same bytes is not a component.
  assert.equal(isHttpComponent(Uint8Array.from([0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, ...section(11, name)])), false);
  // A section that claims more bytes than the file has, and no file at all.
  assert.equal(isHttpComponent(component([11, 0x7f, ...name])), false);
  assert.equal(isHttpComponent(new Uint8Array(0)), false);
});

test("a bundle that carries the preview's data-jc-src stamp is named, a clean one passes (SDK-46)", () => {
  const dir = mkdtempSync(join(tmpdir(), "jc-unstamped-"));
  try {
    mkdirSync(join(dir, "assets"));
    writeFileSync(join(dir, "index.html"), "<div id=root></div>");
    writeFileSync(join(dir, "assets", "index-1.js"), 'jsx("h2", { children: n })');
    writeFileSync(join(dir, "assets", "notes.txt"), "data-jc-src in prose is not code");
    // T-3578: the SDK's own select module names the attribute it reads; that is no stamp, and
    // every App bundle that keeps the module failed the lane on it.
    writeFileSync(
      join(dir, "assets", "sdk-1.js"),
      'const e="data-jc-src";addEventListener("click",t=>{t.target.closest(`[${e}]`)?.getAttribute(e)});export{e as SOURCE_ATTRIBUTE};',
    );
    assert.deepEqual(stampedFiles(dir), []);
    writeFileSync(join(dir, "assets", "index-2.js"), 'jsx("h2", { "data-jc-src": "src/App.tsx:4" })');
    assert.deepEqual(stampedFiles(dir), [join("assets", "index-2.js")]);
    // Minified, and in a page: a stamp is the attribute with a file and a line.
    writeFileSync(join(dir, "assets", "index-3.js"), 'jsx("p",{"data-jc-src":"src/pages/Card.tsx:12",children:n})');
    writeFileSync(join(dir, "page.html"), '<p data-jc-src="src/App.tsx:7">x</p>');
    assert.deepEqual(stampedFiles(dir), [join("assets", "index-2.js"), join("assets", "index-3.js"), "page.html"]);
    const cli = spawnSync(process.execPath, [new URL("./lane.mjs", import.meta.url).pathname, "unstamped", dir], { encoding: "utf8" });
    assert.equal(cli.status, 1);
    assert.match(cli.stderr, /assets\/index-2\.js, assets\/index-3\.js, page\.html \(SDK-46\)/);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

// AP-147: server code that names a socket, the environment, the file system or a process is
// refused with its line; a comment that names one is not code, and the SDK's own calls pass.
test("the server source may name no socket, environment, file system or process", () => {
  const refused = serverRefusals({
    "server/src/lib.rs": [
      "use jc_app_sdk::{gateway, sql};",
      "// std::env is not for components",
      "let key = std::env::var(\"KEY\");",
      "let s = TcpStream::connect(\"10.0.0.1:5432\");",
      "let f = std::fs::read_to_string(\"/etc/passwd\");",
      "const T: &str = env!(\"TOKEN\");",
      "std::process::exit(1);",
    ].join("\n"),
    "server/src/ok.rs": "let rows = sql::query(\"select 1\", &[]);\nlet envelope = 1; // an envelope is not env!",
  });
  assert.deepEqual(refused.map((line) => line.split(": names ")[0]), [
    "server/src/lib.rs:3",
    "server/src/lib.rs:4",
    "server/src/lib.rs:5",
    "server/src/lib.rs:6",
    "server/src/lib.rs:7",
  ]);
  assert.match(refused[0], /the environment, which a component never has \(AP-147\)/);
  assert.match(refused[1], /a socket/);
  assert.match(refused[2], /the file system/);
  assert.deepEqual(serverRefusals({}), []);
});

// T-3618: a `wasm` App's server tests, as libtest prints them and as the sandbox runs them.
const LIBTEST = `
running 3 tests
test tests::adds ... ok
test tests::rows_round_trip ... ok
test tests::subtracts ... FAILED

failures:

---- tests::subtracts stdout ----

thread 'tests::subtracts' panicked at src/lib.rs:9:9:
assertion \`left == right\` failed
  left: 1
 right: 3
note: run with \`RUST_BACKTRACE=1\` environment variable to display a backtrace


failures:
    tests::subtracts

test result: FAILED. 2 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
`;

test("cargo test's output reads as the sandbox's report: counts, names, messages and files", () => {
  const red = lane.cargoSummary(LIBTEST, "error: test failed, to rerun pass `--lib`", 101);
  assert.equal(red.outcome, "failed");
  assert.equal(red.passed, 2);
  assert.equal(red.failed, 1);
  assert.deepEqual(Object.keys(red.failures[0]), ["file", "name", "message"]);
  assert.equal(red.failures[0].file, "server/src/lib.rs");
  assert.equal(red.failures[0].name, "tests::subtracts");
  assert.match(red.failures[0].message, /left: 1\n right: 3/);
  assert.doesNotMatch(red.failures[0].message, /failures:/);

  const green = lane.cargoSummary("running 1 test\ntest tests::adds ... ok\n\ntest result: ok. 1 passed; 0 failed\n", "", 0);
  assert.deepEqual(green, { outcome: "passed", passed: 1, failed: 0, failures: [] });

  const broken = lane.cargoSummary("", "   Compiling app-server v0.1.0\nerror[E0425]: cannot find value `x` in this scope\n --> src/lib.rs:3:5\n", 101);
  assert.equal(broken.outcome, "failed");
  assert.equal(broken.failures[0].name, "(the crate did not build)");
  assert.match(broken.failures[0].message, /^error\[E0425\]: cannot find value `x`/);
});

const cargoOn = spawnSync("cargo", ["--version"]).status === 0;
// The lane hands cargo a HOME of the job's own; a rustup install in the user's home is then
// found through RUSTUP_HOME, as the rust image sets it.
if (cargoOn && !process.env.RUSTUP_HOME && process.env.HOME && existsSync(join(process.env.HOME, ".rustup"))) {
  process.env.RUSTUP_HOME = join(process.env.HOME, ".rustup");
}

/** A store with nothing in it, enough for a crate without dependencies to build offline. */
function emptyCargo(dir, bin = "cargo") {
  mkdirSync(join(dir, "store", "registry", "index"), { recursive: true });
  mkdirSync(join(dir, "store", "registry", "cache"), { recursive: true });
  return { bin, store: join(dir, "store"), seed: join(dir, "no-seed"), home: join(dir, "cargo-home"), target: join(dir, "target") };
}

const SERVER = {
  "server/Cargo.toml": '[package]\nname = "app-server"\nversion = "0.1.0"\nedition = "2021"\n\n[workspace]\n',
  "server/Cargo.lock": '# This file is automatically @generated by Cargo.\nversion = 4\n\n[[package]]\nname = "app-server"\nversion = "0.1.0"\n',
  "server/src/lib.rs":
    "pub fn add(a: i32, b: i32) -> i32 { a + b }\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn adds() { assert_eq!(super::add(1, 1), 2); }\n    #[test]\n    fn subtracts() { assert_eq!(super::add(2, -1), 3); }\n}\n",
};

test("a red server test makes the sandbox's report red beside the interface's", { skip: (!existsSync(join(store, ".bin", "vitest")) && "vitest is not linked") || (!cargoOn && "no cargo") }, () => {
  const { input, work } = project({
    ...SERVER,
    "src/sum.test.ts": 'import { expect, test } from "vitest";\ntest("adds", () => expect(1 + 1).toBe(2));\n',
  });
  const result = lane.testProject(input, work, { store, timeoutMs: 120_000, cargo: emptyCargo(dirname(work)) });
  assert.equal(result.outcome, "failed", JSON.stringify(result));
  assert.equal(result.passed, 2, JSON.stringify(result));
  assert.equal(result.failed, 1);
  assert.equal(result.failures[0].name, "tests::subtracts");
  assert.equal(result.failures[0].file, "server/src/lib.rs");
});

test("the server's tests are reported as not run where the image has no crate store or no cargo", { skip: !existsSync(join(store, ".bin", "vitest")) && "vitest is not linked" }, () => {
  const { input, work } = project({ ...SERVER, "src/sum.test.ts": 'import { expect, test } from "vitest";\ntest("adds", () => expect(1 + 1).toBe(2));\n' });
  const none = lane.testProject(input, work, { store, cargo: { ...emptyCargo(dirname(work)), store: join(dirname(work), "missing") } });
  assert.equal(none.outcome, "passed");
  assert.match(none.reason, /no crate store/);
  const noCargo = lane.testProject(input, work, { store, cargo: emptyCargo(dirname(work), join(dirname(work), "no-cargo")) });
  assert.match(noCargo.reason, /no cargo/);
});
