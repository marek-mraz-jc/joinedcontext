// The build lane's checks, its functions bundle, its SBOM and its proposal (AP-80, AP-82, AP-84,
// AP-101, ADR-N-028).
//
//   node lane.mjs deps <app-dir>       refuses a package the template does not install (SDK-12)
//   node lane.mjs vitest-config <app-dir>  writes the config the tests run with, the app's own plus the SDK inlined
//   node lane.mjs functions <app-dir> <out-dir>
//                                      bundles functions/*.ts into <out-dir>/functions.js
//   node lane.mjs app <owner/repo>     prints "{project} {app}" of an application repository
//   node lane.mjs sbom <node_modules> <out-file>
//                                      the CycloneDX SBOM of the packages the build linked
//   node lane.mjs image <layer.tar> <dir>
//                                      writes a fullstack App's OCI image layout from its one
//                                      layer and prints the manifest digest (AP-105)
//   node lane.mjs upload <build-dir>   uploads bundle-{commit} and sbom-{commit} as the run's
//                                      artifacts and writes the build to $GITHUB_OUTPUT
//   node lane.mjs seed <from> <to>     starts a fullstack build's target/ from the image's
//                                      precompiled dependencies (AP-106), never failing a build
//   node lane.mjs lock-manifest <dir> points <dir>/package.json's SDK at the directory above, as the
//                                      template's lockfile resolves it (AP-126)
//   node lane.mjs store-check <pnpm-lock.yaml> <node_modules>
//                                      fails when a package the lockfile names for this platform
//                                      is not in the store (AP-127)
//   node lane.mjs restore <cache> <key> <seed> <to>
//                                      starts it from the App's own build cache when it holds a
//                                      complete entry for <key>, else from the seed (AP-131)
//   node lane.mjs save <from> <cache> <key>
//                                      keeps a green build's target/ as the App's cache entry
//   node lane.mjs unstamped <bundle-dir>
//                                      fails when a file of the bundle carries the preview's
//                                      data-jc-src stamp (SDK-46)
//   node lane.mjs propose <owner/repo> proposes status.build as the lane, from the build job's
//                                      outputs (JC_DIGEST, JC_COMMIT, JC_SDK_VERSION, JC_BUILT_AT)
//                                      to JC_PORTAL_URL with JC_LANE_TOKEN
//   node lane.mjs browser-checks <app-dir> <bundle-dir> <report-dir>
//                                      runs the App's own Playwright suite over the bundle just
//                                      built, on the image's Chromium, with the report and its
//                                      screenshots in <report-dir>; an App with no e2e/ is not
//                                      checked and says so (T-2827)
//   node lane.mjs server-check <app-dir>
//                                      refuses a server/ crate whose source names a socket, the
//                                      environment, the file system or a process (AP-147)
//   node lane.mjs test-project <project.json.gz> <work-dir>
//                                      a run's sandbox (SDK-38): writes the version's files, links
//                                      the template, runs vitest and tsc -b as the lane does and prints
//                                      one line `JC-TESTS {json}` the Portal reads from the pod's log
//
// Lives beside /opt/template/node_modules in the image, so `vite` resolves to the template's.

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { gunzipSync, gzipSync } from "node:zlib";
import { appendFileSync, cpSync, existsSync, mkdirSync, readdirSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";

const TEMPLATE = new URL("./package.json", import.meta.url);
// The name a function is called by, the same rule the Portal applies to a run's functions.
const FUNCTION = /^[a-z][a-z0-9-]{0,39}$/;

// A project and an application name as the forge repository `{project}_{app}` carries them (AP-75).
const LABEL = /^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$/;

const names = (pkg) =>
  Object.keys({ ...(pkg.dependencies ?? {}), ...(pkg.devDependencies ?? {}) });

/** Every package the app asks for that the template does not install. */
export function refusedDependencies(app, template) {
  const allowed = new Set(names(template));
  return names(app).filter((name) => !allowed.has(name)).sort();
}

/** The lane's vitest config, beside the app's so its root is the app (T-2649). */
export const VITEST_CONFIG = ".jc-vitest.config.mjs";

/**
 * Writes the config the lane runs an app's tests with: the app's own `vite.config`, if it has
 * one, with the packed SDK inlined. The app's packages are links into the template's store
 * outside the app, so vitest externalises the SDK and Node would import the stylesheet the SDK
 * imports, which it cannot (SDK-24, AP-82). The template's config says the same; this holds it
 * for an app whose config does not.
 */
export function vitestConfig(appDir) {
  const own = ["vite.config.ts", "vite.config.mts", "vite.config.js", "vite.config.mjs"].find((name) =>
    existsSync(join(appDir, name)),
  );
  const lines = [
    "// Written by the build lane (T-2649), removed before the next build.",
    'import { mergeConfig } from "vitest/config";',
    ...(own ? [`import app from "./${own}";`] : []),
    'const lane = { test: { server: { deps: { inline: ["@joinedcontext/sdk"] } } } };',
    own
      ? "export default typeof app === \"function\" ? async (env) => mergeConfig(await app(env), lane) : mergeConfig(app, lane);"
      : "export default lane;",
  ];
  const path = join(appDir, VITEST_CONFIG);
  writeFileSync(path, lines.join("\n") + "\n");
  return path;
}

/** A path of a run's project: relative, plain segments, never `node_modules` (SDK-11, SDK-38). */
const PROJECT_PATH = /^[A-Za-z0-9_@+-][A-Za-z0-9_.@+-]*(\/[A-Za-z0-9_@+-][A-Za-z0-9_.@+-]*)*$/;

/**
 * Writes a run's version into `dir` (SDK-38). The files are untrusted: a path that is absolute,
 * climbs out with `..`, hides in a dot folder or names `node_modules` is refused before anything
 * is written, so the sandbox links nothing the model chose.
 */
export function writeProject(files, dir) {
  if (files === null || typeof files !== "object" || Array.isArray(files)) {
    throw new Error("the project is not an object of paths to file contents");
  }
  const entries = Object.entries(files);
  for (const [path, content] of entries) {
    if (!PROJECT_PATH.test(path) || path.split("/")[0] === "node_modules") {
      throw new Error(`${JSON.stringify(path)} is not a path a project may hold`);
    }
    if (typeof content !== "string") throw new Error(`${path} is not text`);
  }
  for (const [path, content] of entries) {
    const target = join(dir, path);
    mkdirSync(dirname(target), { recursive: true });
    writeFileSync(target, content);
  }
}

/**
 * The template's installed packages linked into `dir/node_modules`, as `build-app` links them: a
 * writable folder of links, since Vite writes its cache there (AP-82).
 */
export function linkTemplate(dir, store = join(dirname(new URL(import.meta.url).pathname), "node_modules")) {
  const modules = join(dir, "node_modules");
  rmSync(modules, { recursive: true, force: true });
  mkdirSync(modules);
  for (const entry of readdirSync(store)) {
    symlinkSync(join(store, entry), join(modules, entry));
  }
}

const MAX_FAILURES = 20;
const MAX_MESSAGE = 2000;
const ANSI = /\u001b\[[0-9;]*m/g;
const EMPTY = { outcome: "passed", passed: 0, failed: 0, failures: [] };
const cut = (text, limit) => {
  const plain = String(text ?? "").replace(ANSI, "").trim();
  return plain.length > limit ? `${plain.slice(0, limit)}…` : plain;
};

/**
 * What vitest's JSON report says, as the Portal reads it (SDK-38, API/04 §4): the counts, and up
 * to 20 failures with the file relative to the project, the test's full name and its message cut
 * at 2,000 characters. A file that fails before any test runs (a syntax error, an import the SDK
 * does not have) is a failure of its own.
 */
export function testSummary(report, appDir) {
  const failures = [];
  let passed = 0;
  let failed = 0;
  for (const file of report?.testResults ?? []) {
    const name = typeof file.name === "string" ? relative(appDir, file.name) : "";
    const tests = file.assertionResults ?? [];
    for (const one of tests) {
      if (one.status === "passed") passed += 1;
      if (one.status !== "failed") continue;
      failed += 1;
      failures.push({ file: name, name: cut(one.fullName ?? one.title, 300), message: cut((one.failureMessages ?? []).join("\n"), MAX_MESSAGE) });
    }
    if (file.status === "failed" && !tests.some((one) => one.status === "failed")) {
      failed += 1;
      failures.push({ file: name, name: "(the file did not load)", message: cut(file.message, MAX_MESSAGE) });
    }
  }
  return {
    outcome: failed > 0 ? "failed" : "passed",
    passed,
    failed,
    failures: failures.slice(0, MAX_FAILURES),
  };
}

/**
 * What `cargo test --no-fail-fast` printed, as the Portal reads it (SDK-38, T-3618): libtest's
 * `test … ok` and `test … FAILED` lines, each failure with its `---- name stdout ----` section
 * as the message and the file its panic names, under `server/`. A run that failed without one
 * `test result:` line never got to its tests: the crate did not build, and the compiler's
 * errors are the one failure.
 */
export function cargoSummary(stdout, stderr, status) {
  const out = String(stdout ?? "").replace(ANSI, "");
  let passed = 0;
  let failed = 0;
  const names = [];
  for (const line of out.split("\n")) {
    const one = /^test (\S+) \.\.\. (ok|FAILED)$/.exec(line.trim());
    if (!one) continue;
    if (one[2] === "ok") passed += 1;
    else {
      failed += 1;
      names.push(one[1]);
    }
  }
  const failures = names.map((name) => {
    const at = out.indexOf(`---- ${name} stdout ----`);
    const rest = at < 0 ? "" : out.slice(at).split("\n").slice(1).join("\n");
    const message = rest.split(/\n(?:---- \S+ stdout ----|failures:)\n/)[0] ?? "";
    const file = /panicked at ([^:\s]+\.rs):\d+/.exec(message)?.[1];
    return { file: file ? `server/${file}` : "server/", name: cut(name, 300), message: cut(message, MAX_MESSAGE) };
  });
  if (status !== 0 && !/^test result:/m.test(out)) {
    const errors = String(stderr ?? "").replace(ANSI, "");
    const first = errors.search(/^error/m);
    failed += 1;
    failures.unshift({ file: "server/", name: "(the crate did not build)", message: cut(first < 0 ? errors : errors.slice(first), MAX_MESSAGE) });
  }
  return { outcome: failed > 0 ? "failed" : "passed", passed, failed, failures: failures.slice(0, MAX_FAILURES) };
}

/**
 * What `tsc -b` printed, as the Portal reads it: one failure per error, under the file it names,
 * so the model sees what the build lane would refuse (T-6429). A line that starts no error goes
 * with the error above it. A run that failed without one error line is one failure of its own.
 */
export function typeSummary(stdout, stderr, status) {
  const failures = [];
  for (const line of `${stdout ?? ""}\n${stderr ?? ""}`.replace(ANSI, "").split("\n")) {
    const one = /^(.+?)\((\d+),\d+\): error TS\d+/.exec(line);
    if (one) failures.push({ file: one[1], name: `(type error at line ${one[2]})`, message: cut(line, MAX_MESSAGE) });
    else if (failures.length > 0 && line.trim() !== "") {
      const last = failures[failures.length - 1];
      last.message = cut(`${last.message}\n${line}`, MAX_MESSAGE);
    }
  }
  if (status !== 0 && failures.length === 0) {
    failures.push({ file: "tsconfig.json", name: "(the type check did not run)", message: cut(`${stdout ?? ""}${stderr ?? ""}`.replace(ANSI, ""), MAX_MESSAGE) });
  }
  return { outcome: failures.length > 0 ? "failed" : "passed", passed: 0, failed: failures.length, failures: failures.slice(0, MAX_FAILURES) };
}

/** The sum of two summaries, a failure in either failing it. */
function summed(a, b) {
  return {
    outcome: a.failed + b.failed > 0 ? "failed" : "passed",
    passed: a.passed + b.passed,
    failed: a.failed + b.failed,
    failures: [...a.failures, ...b.failures].slice(0, MAX_FAILURES),
  };
}

/**
 * `tsc -b` of a version, the check the build lane runs after its tests (T-6429). A project with no
 * `tsconfig.json` is the lane's `build: {}` shape, and a store without tsc has nothing to run.
 */
function typeCheck(work, env, timeoutMs) {
  const tsc = join(work, "node_modules", ".bin", "tsc");
  if (!existsSync(join(work, "tsconfig.json")) || !existsSync(tsc)) return EMPTY;
  if (timeoutMs <= 0) return { ...EMPTY, skipped: "the tests left no time for the type check, so the build lane alone runs it" };
  const run = spawnSync(tsc, ["-b"], { cwd: work, env, encoding: "utf8", timeout: timeoutMs, maxBuffer: 16 * 1024 * 1024 });
  if (run.error?.code === "ETIMEDOUT" || run.signal) {
    return { ...EMPTY, skipped: "the type check did not finish in the time left, so the build lane alone runs it" };
  }
  return typeSummary(run.stdout, run.stderr, run.status);
}

/**
 * The rust runner's crate store and the precompiled dependencies of the wasm template's server
 * (builder/Dockerfile, T-3618). The job's cargo home and target sit at the paths the seed was
 * compiled at, so the fingerprints match and only the App's own crate is compiled.
 */
const CARGO = { bin: "cargo", store: "/opt/cargo", seed: "/opt/cargo/test-seed", home: "/tmp/build/cargo", target: "/tmp/build/target" };

/**
 * `cargo test` of the version's `server/` (T-3618): offline and `--locked` against the store,
 * linked read-only into a cargo home of the job's own, from the seeded target, within `timeoutMs`.
 * An image without the store reports nothing for it: the build lane still tests the crate.
 */
export function cargoTests(work, { timeoutMs, cargo = CARGO } = {}) {
  const registry = join(cargo.store, "registry");
  if (!existsSync(join(registry, "index")) || !existsSync(join(registry, "cache"))) {
    return { skipped: "this sandbox carries no crate store, so server/ is tested by the build lane alone" };
  }
  rmSync(cargo.home, { recursive: true, force: true });
  mkdirSync(join(cargo.home, "registry"), { recursive: true });
  symlinkSync(join(registry, "index"), join(cargo.home, "registry", "index"));
  symlinkSync(join(registry, "cache"), join(cargo.home, "registry", "cache"));
  if (existsSync(join(cargo.store, "git"))) symlinkSync(join(cargo.store, "git"), join(cargo.home, "git"));
  seed(cargo.seed, cargo.target);
  const run = spawnSync(cargo.bin, ["test", "--offline", "--locked", "--no-fail-fast"], {
    cwd: join(work, "server"),
    env: {
      PATH: process.env.PATH ?? "/usr/local/cargo/bin:/usr/local/bin:/usr/bin:/bin",
      // The rust image's toolchain is found through RUSTUP_HOME; nothing else of the pod's
      // environment reaches the model's code.
      ...(process.env.RUSTUP_HOME ? { RUSTUP_HOME: process.env.RUSTUP_HOME } : {}),
      HOME: work,
      CARGO_HOME: cargo.home,
      CARGO_TARGET_DIR: cargo.target,
      CARGO_NET_OFFLINE: "true",
      CARGO_TERM_COLOR: "never",
    },
    encoding: "utf8",
    timeout: Math.max(timeoutMs, 1_000),
    maxBuffer: 16 * 1024 * 1024,
  });
  if (run.error?.code === "ENOENT") return { skipped: "this sandbox has no cargo, so server/ is tested by the build lane alone" };
  if (run.error?.code === "ETIMEDOUT" || run.signal) return { error: `the server's tests did not finish within ${Math.round(timeoutMs / 1000)} s` };
  return cargoSummary(run.stdout, run.stderr, run.status);
}

/**
 * One run of a version's tests in the sandbox (SDK-38): the project from the Portal's gzipped
 * JSON, the template linked, the lane's vitest config, one `vitest run` with a JSON report, `tsc -b`,
 * then `cargo test` of a `wasm` App's `server/` in what time is left (T-3618). vitest's exit status
 * counts too: an unhandled error fails the build lane's run while every test in the report passed
 * (T-6429). The answer is all of them summed, or `{outcome: "error", reason}` when the tests could
 * not run at all.
 */
export function testProject(input, work, { store, timeoutMs = 150_000, cargo = CARGO } = {}) {
  const started = Date.now();
  let files;
  try {
    files = JSON.parse(gunzipSync(readFileSync(input)).toString("utf8"));
  } catch (err) {
    return { outcome: "error", reason: `the project could not be read: ${err instanceof Error ? err.message : String(err)}` };
  }
  rmSync(work, { recursive: true, force: true });
  mkdirSync(work, { recursive: true });
  try {
    writeProject(files, work);
  } catch (err) {
    return { outcome: "error", reason: err instanceof Error ? err.message : String(err) };
  }
  const packageJson = join(work, "package.json");
  if (existsSync(packageJson)) {
    const refused = refusedDependencies(JSON.parse(readFileSync(packageJson, "utf8")), JSON.parse(readFileSync(TEMPLATE, "utf8")));
    if (refused.length > 0) return { outcome: "error", reason: `the SDK template installs no ${refused.join(", ")} (SDK-12)` };
  }
  linkTemplate(work, store);
  const config = vitestConfig(work);
  const report = join(work, ".jc-tests.json");
  // Nothing of the pod's environment reaches the model's code beyond what node needs.
  const env = { PATH: process.env.PATH ?? "/usr/local/bin:/usr/bin:/bin", HOME: work, CI: "true", NODE_ENV: "test" };
  const run = spawnSync(join(work, "node_modules", ".bin", "vitest"), ["run", "--config", config, "--reporter=json", "--reporter=dot", `--outputFile.json=${report}`, "--passWithNoTests"], {
    cwd: work,
    env,
    encoding: "utf8",
    timeout: timeoutMs,
    maxBuffer: 16 * 1024 * 1024,
  });
  const durationMs = Date.now() - started;
  if (run.error?.code === "ETIMEDOUT" || run.signal) {
    return { outcome: "error", reason: `the tests did not finish within ${Math.round(timeoutMs / 1000)} s`, durationMs };
  }
  if (!existsSync(report)) {
    return { outcome: "error", reason: `vitest wrote no report: ${cut(run.stderr || run.stdout, MAX_MESSAGE)}`, durationMs };
  }
  let ui;
  try {
    ui = testSummary(JSON.parse(readFileSync(report, "utf8")), work);
  } catch (err) {
    return { outcome: "error", reason: `the report could not be read: ${err instanceof Error ? err.message : String(err)}`, durationMs };
  }
  if (run.status !== 0 && ui.failed === 0) {
    const printed = `${run.stderr ?? ""}${run.stdout ?? ""}`.replace(ANSI, "");
    const at = printed.search(/Unhandled/);
    ui = summed(ui, { ...EMPTY, failed: 1, failures: [{ file: "vitest", name: "(an error outside the tests)", message: cut(at < 0 ? printed : printed.slice(at), MAX_MESSAGE) }] });
  }
  const types = typeCheck(work, env, timeoutMs - (Date.now() - started));
  const checked = summed(ui, types);
  if (!existsSync(join(work, "server", "Cargo.toml"))) {
    return { ...checked, ...(types.skipped ? { reason: types.skipped } : {}), durationMs: Date.now() - started };
  }
  const server = cargoTests(work, { timeoutMs: timeoutMs - (Date.now() - started), cargo });
  const total = Date.now() - started;
  if (server.error) return { outcome: "error", reason: server.error, durationMs: total };
  if (server.skipped) return { ...checked, reason: server.skipped, durationMs: total };
  return { ...summed(checked, server), durationMs: total };
}

/** The functions of `functions/`, by name; a test file is not a function. */
export function functionEntries(dir) {
  if (!existsSync(dir)) return [];
  const entries = [];
  for (const file of readdirSync(dir, { withFileTypes: true })) {
    if (!file.isFile() || !file.name.endsWith(".ts") || file.name.endsWith(".test.ts")) continue;
    const name = file.name.slice(0, -".ts".length);
    if (!FUNCTION.test(name)) {
      throw new Error(
        `functions/${file.name}: a function name is lowercase letters, digits and '-', at most 40, starting with a letter`,
      );
    }
    entries.push(name);
  }
  return entries.sort();
}

/** One ES module exporting every function under its name, `@joinedcontext/sdk/server` left to the runtime. */
export async function bundleFunctions(appDir, outDir) {
  const dir = join(appDir, "functions");
  const entries = functionEntries(dir);
  if (entries.length === 0) return false;
  const entry = join(appDir, ".jc-functions-entry.ts");
  writeFileSync(
    entry,
    entries
      .map((name) => `export { default as ${JSON.stringify(name)} } from ${JSON.stringify(join(dir, `${name}.ts`))};`)
      .join("\n") + "\n",
  );
  const { build } = await import("vite");
  await build({
    configFile: false,
    logLevel: "warn",
    root: appDir,
    build: {
      outDir: resolve(outDir),
      emptyOutDir: false,
      minify: false,
      target: "es2022",
      lib: { entry, formats: ["es"], fileName: () => "functions.js" },
      rollupOptions: { external: ["@joinedcontext/sdk/server"] },
    },
  });
  return true;
}

/** The project and the application a repository `owner/{project}_{app}` belongs to (AP-75). */
export function appOf(repository) {
  const name = String(repository).split("/").pop() ?? "";
  const cut = name.indexOf("_");
  const project = name.slice(0, cut);
  const app = name.slice(cut + 1);
  if (cut < 1 || !LABEL.test(project) || !LABEL.test(app)) {
    throw new Error(`${name}: an application repository is named {project}_{app}`);
  }
  return { project, app };
}

/** One package of pnpm's store folder, `@scope+name@1.2.3_peer…` → `@scope/name`, `1.2.3`. */
function storePackage(folder) {
  const plain = folder.split("_")[0];
  const at = plain.lastIndexOf("@");
  if (at <= 0) return null;
  return { name: plain.slice(0, at).replace("+", "/"), version: plain.slice(at + 1) };
}

/**
 * The SDK is this release's own build, never a registry package: a template's lockfile resolves
 * it from the directory above the template, `sdk/` in the repository and the unpacked SDK in the
 * runner image, so both install the same packages (AP-126, AP-127).
 */
export const SDK_SPEC = "file:..";

/** The template's manifest as its lockfile was written from: the SDK taken from `SDK_SPEC`. */
export function lockManifest(pkg) {
  if (!pkg.dependencies?.["@joinedcontext/sdk"]) throw new Error("the template does not depend on @joinedcontext/sdk");
  return { ...pkg, dependencies: { ...pkg.dependencies, "@joinedcontext/sdk": SDK_SPEC } };
}

/** Whether a lockfile's `os`/`cpu`/`libc` list admits `value`: absent, listed, or not denied with `!`. */
function admits(list, value) {
  if (!list) return true;
  const items = list.split(",").map((item) => item.trim().replace(/^'|'$/g, "")).filter(Boolean);
  const denied = items.filter((item) => item.startsWith("!")).map((item) => item.slice(1));
  return denied.length > 0 ? !denied.includes(value) : items.includes(value);
}

/**
 * The `name@version` entries of a pnpm v9 lockfile's `packages:` that a store for `platform`
 * ({ os, cpu, libc }) must hold but whose folder is missing from `folders` (the names in
 * `node_modules/.pnpm`), sorted (AP-127). Packages built for another platform are not installed
 * there and are skipped; a directory or link dependency (the SDK) is not a registry package.
 */
export function missingFromStore(lock, folders, platform) {
  const have = new Set(folders.map((folder) => folder.split("_")[0]));
  const entries = [];
  let section = "";
  for (const line of String(lock).split("\n")) {
    if (/^\S/.test(line)) {
      section = line.trim();
      continue;
    }
    if (section !== "packages:") continue;
    const key = /^  '?([^' ][^']*?)'?:\s*$/.exec(line)?.[1];
    if (key) {
      entries.push({ key, fields: {} });
      continue;
    }
    const field = /^    (os|cpu|libc): \[(.*)\]\s*$/.exec(line);
    if (field && entries.length > 0) entries[entries.length - 1].fields[field[1]] = field[2];
  }
  const missing = [];
  for (const { key, fields } of entries) {
    const at = key.lastIndexOf("@");
    const [name, version] = [key.slice(0, at), key.slice(at + 1)];
    if (at <= 0 || version.includes(":")) continue;
    if (!admits(fields.os, platform.os) || !admits(fields.cpu, platform.cpu) || !admits(fields.libc, platform.libc)) continue;
    if (!have.has(`${name.replace("/", "+")}@${version}`)) missing.push(key);
  }
  return missing.sort();
}

/** The CycloneDX 1.5 SBOM of the packages in pnpm's store folders, sorted, without a timestamp. */
/** The `wasi:http` handler a server component must export (AP-143, AP-151). */
export const HTTP_HANDLER = "wasi:http/incoming-handler@";

/**
 * Whether `bytes` are a WebAssembly component (not a core module) whose own export section names
 * `wasi:http/incoming-handler`: the preamble is the component layer's, and the top-level sections
 * are walked by their sizes, so a name inside a nested module or a custom section does not count.
 */
/**
 * What a `wasm` App's server source may not name (AP-147): the host gives a component no socket,
 * environment, file system or process, so code that reaches for one is refused before it builds,
 * with the line, instead of failing at run time. Line comments are not code.
 */
const SERVER_REFUSED = [
  [/\bstd::net\b|\b(TcpStream|TcpListener|UdpSocket)\b|\bwasi::sockets\b/, "a socket"],
  [/\bstd::env\b|\b(option_)?env!\s*\(|\bwasi::cli::environment\b/, "the environment"],
  [/\bstd::fs\b|\bwasi::filesystem\b|\binclude_(str|bytes)!\s*\(/, "the file system"],
  [/\bstd::process\b/, "a process"],
];

/** `{path: source}` of server/src → one `path:line: names X (AP-147)` per refused line. */
export function serverRefusals(files) {
  const refusals = [];
  for (const [path, source] of Object.entries(files)) {
    source.split("\n").forEach((line, i) => {
      const code = line.replace(/\/\/.*$/, "");
      for (const [pattern, what] of SERVER_REFUSED) {
        if (pattern.test(code)) refusals.push(`${path}:${i + 1}: names ${what}, which a component never has (AP-147)`);
      }
    });
  }
  return refusals;
}

function rustSources(dir, prefix = "server/src") {
  const files = {};
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) Object.assign(files, rustSources(path, `${prefix}/${entry.name}`));
    else if (entry.name.endsWith(".rs")) files[`${prefix}/${entry.name}`] = readFileSync(path, "utf8");
  }
  return files;
}

export function isHttpComponent(bytes) {
  const preamble = [0x00, 0x61, 0x73, 0x6d, 0x0d, 0x00, 0x01, 0x00];
  if (bytes.length < preamble.length || preamble.some((b, i) => bytes[i] !== b)) return false;
  const leb = (at) => {
    let value = 0;
    let shift = 0;
    for (let i = at; i < bytes.length && i < at + 5; i += 1) {
      value += (bytes[i] & 0x7f) * 2 ** shift;
      shift += 7;
      if ((bytes[i] & 0x80) === 0) return [value, i + 1];
    }
    return [-1, bytes.length];
  };
  const name = Buffer.from(HTTP_HANDLER);
  let at = preamble.length;
  while (at < bytes.length) {
    const id = bytes[at];
    const [size, body] = leb(at + 1);
    if (size < 0 || body + size > bytes.length) return false;
    // Section 11 is the component's export section.
    if (id === 11 && Buffer.from(bytes.subarray(body, body + size)).includes(name)) return true;
    at = body + size;
  }
  return false;
}

export function sbomOf(folders) {
  const seen = new Map();
  for (const folder of folders) {
    const pkg = storePackage(folder);
    if (pkg && pkg.version) seen.set(`${pkg.name}@${pkg.version}`, pkg);
  }
  const components = [...seen.keys()].sort().map((key) => {
    const { name, version } = seen.get(key);
    return { type: "library", name, version, purl: `pkg:npm/${name.replace("@", "%40")}@${version}` };
  });
  return { bomFormat: "CycloneDX", specVersion: "1.5", version: 1, components };
}

/** The crates a `Cargo.lock` names from a registry, as SBOM components; path crates are the app. */
export function cratesOf(lock) {
  const crates = [];
  for (const block of String(lock).split(/^\[\[package\]\]$/m).slice(1)) {
    const field = (key) => new RegExp(`^${key} = "([^"]*)"$`, "m").exec(block)?.[1];
    const [name, version, source] = [field("name"), field("version"), field("source")];
    if (name && version && source?.startsWith("registry+")) {
      crates.push({ type: "library", name, version, purl: `pkg:cargo/${name}@${version}` });
    }
  }
  return crates.sort((a, b) => `${a.name}@${a.version}`.localeCompare(`${b.name}@${b.version}`));
}

const sha256 = (bytes) => `sha256:${createHash("sha256").update(bytes).digest("hex")}`;

/**
 * A fullstack App's image from its one layer (`/app`, a tar): the gzip layer, a config that runs
 * `/app` as uid 65532, and the OCI manifest, as the blobs of an OCI image layout. No time and no
 * host goes into any of them, so the same layer is the same digest (AP-105).
 */
export function ociImage(layerTar) {
  const layer = gzipSync(layerTar, { level: 9 });
  const config = Buffer.from(
    JSON.stringify({
      architecture: "amd64",
      os: "linux",
      config: { Entrypoint: ["/app"], User: "65532:65532", WorkingDir: "/" },
      rootfs: { type: "layers", diff_ids: [sha256(layerTar)] },
    }),
  );
  const manifest = Buffer.from(
    JSON.stringify({
      schemaVersion: 2,
      mediaType: "application/vnd.oci.image.manifest.v1+json",
      config: { mediaType: "application/vnd.oci.image.config.v1+json", digest: sha256(config), size: config.length },
      layers: [{ mediaType: "application/vnd.oci.image.layer.v1.tar+gzip", digest: sha256(layer), size: layer.length }],
    }),
  );
  const digest = sha256(manifest);
  const index = Buffer.from(
    JSON.stringify({
      schemaVersion: 2,
      mediaType: "application/vnd.oci.image.index.v1+json",
      manifests: [{ mediaType: "application/vnd.oci.image.manifest.v1+json", digest, size: manifest.length }],
    }),
  );
  return { digest, index, blobs: [config, layer, manifest] };
}

/** Writes `image` as an OCI image layout under `dir`: `oci-layout`, `index.json`, `blobs/sha256/*`. */
export function writeLayout(image, dir) {
  mkdirSync(join(dir, "blobs", "sha256"), { recursive: true });
  writeFileSync(join(dir, "oci-layout"), '{"imageLayoutVersion":"1.0.0"}');
  writeFileSync(join(dir, "index.json"), image.index);
  for (const blob of image.blobs) {
    writeFileSync(join(dir, "blobs", "sha256", sha256(blob).slice("sha256:".length)), blob);
  }
}

const DIGEST = /^sha256:[0-9a-f]{64}$/;
const COMMIT = /^[0-9a-f]{40}$/;

/** The App as read, with `status` replaced by the build and nothing else: the lane writes one field (AP-73). */
export function withBuild(app, build) {
  if (!DIGEST.test(build?.digest ?? "") || !COMMIT.test(build?.commit ?? "")) {
    throw new Error("build.json holds no sha256 digest and full commit");
  }
  if (app?.kind !== "App" || !app.metadata || !app.spec) {
    throw new Error("the Portal did not answer with an App");
  }
  const { apiVersion, kind, metadata, spec } = app;
  return { apiVersion, kind, metadata, spec, status: { build } };
}

/** The run and job a runtime token was issued for (`scp: Actions.Results:{run}:{job}`). */
export function artifactScope(runtimeToken) {
  let claims = {};
  try {
    claims = JSON.parse(Buffer.from(String(runtimeToken).split(".")[1] ?? "", "base64url").toString());
  } catch {
    // A token that is not a JWT has no scope, which the check below says.
  }
  const scope = /(?:^| )Actions\.Results:([^: ]+):([^: ]+)/.exec(String(claims.scp ?? ""));
  if (!scope) throw new Error("ACTIONS_RUNTIME_TOKEN names no run to upload to");
  return { workflowRunBackendId: scope[1], workflowJobRunBackendId: scope[2] };
}

/**
 * Uploads `bytes` as the run's artifact `name` through the forge's artifact API (v4): create,
 * append, finalize. The upload address the forge signs carries its public ROOT_URL, which the
 * runner cannot reach, so it is sent to the origin of `ACTIONS_RESULTS_URL` with the signed query
 * kept as it is. The path starts at `/twirp/`: a ROOT_URL with a path (`https://host/git/`) signs
 * `/git/twirp/…`, which only the public proxy strips, and the forge itself answers 404 (T-2633).
 */
export async function uploadArtifact(resultsUrl, runtimeToken, name, bytes, fetchImpl = fetch) {
  const base = new URL(resultsUrl);
  const ids = artifactScope(runtimeToken);
  const service = new URL("twirp/github.actions.results.api.v1.ArtifactService/", `${base.origin}${base.pathname.replace(/\/*$/, "/")}`);
  const call = async (method, body) => {
    const response = await fetchImpl(new URL(method, service), {
      method: "POST",
      headers: { Authorization: `Bearer ${runtimeToken}`, "Content-Type": "application/json" },
      body: JSON.stringify(body),
    });
    const text = await response.text();
    if (!response.ok) throw new Error(`the forge refused ${method} of ${name}: ${response.status} ${text.slice(0, 200)}`);
    return JSON.parse(text || "{}");
  };
  const created = await call("CreateArtifact", { ...ids, name, version: 4 });
  if (!created.signedUploadUrl) throw new Error(`the forge gave no upload address for ${name}`);
  const signed = new URL(created.signedUploadUrl);
  const path = signed.pathname.slice(Math.max(0, signed.pathname.indexOf("/twirp/")));
  const upload = new URL(`${path}${signed.search}`, base.origin);
  upload.searchParams.set("comp", "block");
  const put = await fetchImpl(upload, { method: "PUT", headers: { "Content-Type": "application/octet-stream" }, body: bytes });
  if (!put.ok) throw new Error(`the forge refused the upload of ${name}: ${put.status}`);
  const hash = `sha256:${createHash("sha256").update(bytes).digest("hex")}`;
  await call("FinalizeArtifact", { ...ids, name, size: String(bytes.length), hash });
  return hash;
}

/** The build job's outputs, one `key=value` line each, the four fields checked first. */
export function outputsOf(build) {
  const { digest, commit, sdkVersion, builtAt } = build ?? {};
  if (!DIGEST.test(digest ?? "") || !COMMIT.test(commit ?? "")) {
    throw new Error("build.json holds no sha256 digest and full commit");
  }
  if (!/^[0-9A-Za-z.+-]{1,64}$/.test(sdkVersion ?? "") || !/^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ$/.test(builtAt ?? "")) {
    throw new Error("build.json holds no SDK version and UTC build time");
  }
  return `digest=${digest}\ncommit=${commit}\nsdk-version=${sdkVersion}\nbuilt-at=${builtAt}\n`;
}

/**
 * Reads the App, checks it back with `status.build`, then proposes it; the Change the Portal
 * answers. The REST door takes a proposal only after a green check of the same manifest
 * (PF-57), so the check is a call of its own, not a formality (T-2636).
 */
/**
 * A write the Portal answers 409 met a main that moved past the Portal's copy of the App, as when
 * the forge bootstrap pins a new commit while the build runs (T-2674): the App is read again and
 * the build proposed on it, at most this often, this far apart. The Portal re-reads main every
 * 60 s (JC_PORTAL_SYNC_INTERVAL), so the 150 s between the first try and the last cover two of
 * its re-reads (T-3597).
 */
const ATTEMPTS = 11;
const APART_MS = 15_000;

export async function propose(api, token, repository, build, fetchImpl = fetch, wait = (ms) => new Promise((done) => setTimeout(done, ms))) {
  const { project, app } = appOf(repository);
  const url = `${api.replace(/\/+$/, "")}/api/v1/projects/${project}/apps/${app}`;
  const headers = { Authorization: `Bearer ${token}`, Accept: "application/json" };
  const refused = async (what, response) => {
    let detail = "";
    try {
      detail = (await response.json())?.detail ?? "";
    } catch {
      // A body that is not a problem document says nothing more than its status.
    }
    return new Error(`${what} ${project}/${app}: the Portal answered ${response.status} ${detail}`.trim());
  };
  for (let attempt = 1; ; attempt += 1) {
    const read = await fetchImpl(url, { headers });
    if (!read.ok) throw await refused("cannot read the App", read);
    const body = withBuild(await read.json(), build);
    const checked = await fetchImpl(`${url}?dryRun=All`, {
      method: "PUT",
      headers: { ...headers, "Content-Type": "application/json" },
      body: JSON.stringify(body),
    });
    if (checked.status === 409 && attempt < ATTEMPTS) {
      await wait(APART_MS);
      continue;
    }
    if (!checked.ok) throw await refused("status.build was refused for", checked);
    const verdict = (await checked.json())?.verdict;
    if (verdict?.ok !== true) {
      const findings = (verdict?.findings ?? []).map((f) => f?.message ?? f?.code ?? "").filter(Boolean).join("; ");
      throw new Error(`the check of status.build for ${project}/${app} is not green${findings ? `: ${findings}` : ""}`);
    }
    const written = await fetchImpl(url, {
      method: "PUT",
      headers: { ...headers, "Content-Type": "application/json" },
      body: JSON.stringify(body),
    });
    if (written.status === 409 && attempt < ATTEMPTS) {
      await wait(APART_MS);
      continue;
    }
    if (!written.ok) throw await refused("status.build was refused for", written);
    return written.json();
  }
}

/**
 * A job's `target/` started from the dependencies this release precompiled, in CI, from its
 * reference apps' lockfiles (AP-106, T-2794), so a fullstack build compiles its own crate and the
 * dependencies its lock does not share with them rather than every crate. A copy with its times
 * kept, never a link: nothing a build writes reaches the image or the next job, and the work
 * directory is still wiped after every job (AP-81). A seed that is missing, or that fails to copy
 * part way, leaves an empty `to` and a clean build: it never fails one. Returns what it did.
 */
export function seed(from, to) {
  rmSync(to, { recursive: true, force: true });
  if (!existsSync(from)) {
    mkdirSync(to, { recursive: true });
    return "no precompiled dependencies in this image: every crate is compiled";
  }
  try {
    cpSync(from, to, { recursive: true, preserveTimestamps: true, verbatimSymlinks: true });
    return `the precompiled dependencies of ${from} are the start of this build`;
  } catch (err) {
    rmSync(to, { recursive: true, force: true });
    mkdirSync(to, { recursive: true });
    return `the precompiled dependencies could not be copied (${err instanceof Error ? err.message : String(err)}): every crate is compiled`;
  }
}

/** The App's cache entry and the file that says it is complete and for which key (AP-131). */
const ENTRY = "target";
const MARKER = "target.key";

/**
 * A job's `target/` started from the App's own build cache (AP-131), which only this App's build
 * pods mount: a copy with its times kept, so cargo rebuilds only what changed. The entry counts
 * when its marker names `key`, the digest of the App's `Cargo.lock` and the toolchain. Anything
 * else (no cache mounted, no entry, another key, a copy that fails part way) falls back to the
 * image's seed and never fails a build. Returns what it did.
 */
export function restore(cache, key, seedFrom, to) {
  const marker = join(cache, MARKER);
  let found = "";
  try {
    found = existsSync(marker) ? readFileSync(marker, "utf8").trim() : "";
  } catch {
    found = "";
  }
  if (found === "" || found !== key) {
    const why = !existsSync(cache) ? "no build cache is mounted" : found === "" ? "the build cache holds no complete entry" : "the build cache is for another Cargo.lock or toolchain";
    return `${why}; ${seed(seedFrom, to)}`;
  }
  rmSync(to, { recursive: true, force: true });
  try {
    cpSync(join(cache, ENTRY), to, { recursive: true, preserveTimestamps: true, verbatimSymlinks: true });
    return "the App's build cache is the start of this build";
  } catch (err) {
    return `the build cache could not be copied (${err instanceof Error ? err.message : String(err)}); ${seed(seedFrom, to)}`;
  }
}

/**
 * Keeps a green build's `target/` as the App's one cache entry (AP-131). The marker goes first
 * and comes back last, so a save cut short (a full volume, a pod that ends) leaves an entry
 * `restore` ignores rather than half a target it would trust. A save that fails is said and
 * never fails the build that made it. Returns what it did.
 */
export function save(from, cache, key) {
  if (!existsSync(cache)) return "no build cache is mounted: nothing kept";
  const entry = join(cache, ENTRY);
  try {
    rmSync(join(cache, MARKER), { force: true });
    rmSync(entry, { recursive: true, force: true });
    cpSync(from, entry, { recursive: true, preserveTimestamps: true, verbatimSymlinks: true });
    writeFileSync(join(cache, MARKER), `${key}\n`);
    return "this build's target/ is the App's build cache";
  } catch (err) {
    rmSync(entry, { recursive: true, force: true });
    return `the build cache was not kept (${err instanceof Error ? err.message : String(err)}): the next build starts from the seed`;
  }
}

/**
 * The App's browser checks (T-2827, SDK-12, UI-84): its own `e2e/` suite, run by Playwright over
 * the bundle this build produced. The suite serves `dist/`, so `dist/` is made the bundle, and
 * whatever the repository held there is never what gets checked. A red check fails the build;
 * an App without `e2e/` and `playwright.config.ts` is not blocked, and the log says it was not
 * checked. The report, with the screenshot of every page at every width, lands in `reportDir`.
 * Returns the suite's exit status.
 */
export function browserChecks(appDir, bundleDir, reportDir, run = spawnSync) {
  if (!existsSync(join(appDir, "playwright.config.ts")) || !existsSync(join(appDir, "e2e"))) {
    console.log("no browser checks: the App has no e2e/ and playwright.config.ts (T-2827)");
    return 0;
  }
  const dist = join(appDir, "dist");
  rmSync(dist, { recursive: true, force: true });
  symlinkSync(resolve(bundleDir), dist);
  rmSync(reportDir, { recursive: true, force: true });
  const result = run(join(appDir, "node_modules", ".bin", "playwright"), ["test", "--reporter=line,html"], {
    cwd: appDir,
    stdio: "inherit",
    env: { ...process.env, CI: "true", PLAYWRIGHT_HTML_OUTPUT_DIR: resolve(reportDir), PLAYWRIGHT_HTML_OPEN: "never" },
  });
  if (result.error) {
    console.error(`the browser checks did not start: ${result.error.message}`);
    return 1;
  }
  return result.status ?? 1;
}

/**
 * The files of a bundle that carry the preview's `data-jc-src` stamp (SDK-46): only the Portal's
 * preview transpiler writes it, so a published bundle that holds one leaks source positions. A
 * stamp is the attribute with its `{file}:{line}` value, as JSX props or as HTML; the attribute's
 * bare name is the SDK's select module, which every bundle that keeps it carries (T-3578).
 */
const STAMP = /data-jc-src["']?\s*[:=]\s*["'][^"'\n]*:\d+["']/;

export function stampedFiles(bundleDir) {
  return readdirSync(bundleDir, { recursive: true, withFileTypes: true })
    .filter((entry) => entry.isFile() && /\.(?:html|m?js)$/.test(entry.name))
    .map((entry) => join(entry.parentPath, entry.name))
    .filter((path) => STAMP.test(readFileSync(path, "utf8")))
    .map((path) => relative(bundleDir, path))
    .sort();
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const [command, appDir, outDir] = process.argv.slice(2);
  try {
    if (command === "deps" && appDir) {
      const read = (path) => JSON.parse(readFileSync(path, "utf8"));
      const refused = refusedDependencies(read(join(appDir, "package.json")), read(TEMPLATE));
      if (refused.length > 0) {
        throw new Error(`the SDK template installs no ${refused.join(", ")} (SDK-12)`);
      }
    } else if (command === "vitest-config" && appDir) {
      console.log(vitestConfig(resolve(appDir)));
    } else if (command === "functions" && appDir && outDir) {
      await bundleFunctions(resolve(appDir), outDir);
    } else if (command === "app" && appDir) {
      const { project, app } = appOf(appDir);
      console.log(`${project} ${app}`);
    } else if (command === "sbom" && appDir && outDir) {
      const store = join(appDir, ".pnpm");
      const folders = existsSync(store) ? readdirSync(store) : [];
      const sbom = sbomOf(folders);
      // A fullstack App: the crates its lock names are in the image too; a ui or wasm App's
      // WebAssembly crates and server component are in its bundle (AP-142, AP-151).
      for (const lock of process.argv.slice(5)) sbom.components.push(...cratesOf(readFileSync(lock, "utf8")));
      writeFileSync(outDir, JSON.stringify(sbom, null, 2) + "\n");
    } else if (command === "server-check" && appDir) {
      const src = join(resolve(appDir), "server", "src");
      const refusals = existsSync(src) ? serverRefusals(rustSources(src)) : [];
      if (refusals.length > 0) throw new Error(refusals.join("\n"));
      console.log("server/src names no socket, environment, file system or process (AP-147)");
    } else if (command === "component-check" && appDir) {
      if (!isHttpComponent(readFileSync(appDir))) {
        throw new Error(`${appDir} is not a component exporting ${HTTP_HANDLER}… (AP-143)`);
      }
      console.log(`${appDir} is a component exporting ${HTTP_HANDLER}…`);
    } else if (command === "unstamped" && appDir) {
      const stamped = stampedFiles(appDir);
      if (stamped.length > 0) throw new Error(`the bundle carries the preview's data-jc-src in ${stamped.join(", ")} (SDK-46)`);
    } else if (command === "image" && appDir && outDir) {
      const image = ociImage(readFileSync(appDir));
      writeLayout(image, outDir);
      console.log(image.digest);
    } else if (command === "upload" && appDir) {
      const env = process.env;
      if (!env.ACTIONS_RESULTS_URL || !env.ACTIONS_RUNTIME_TOKEN || !env.GITHUB_OUTPUT) {
        throw new Error("upload runs in a workflow job: ACTIONS_RESULTS_URL, ACTIONS_RUNTIME_TOKEN and GITHUB_OUTPUT are not set");
      }
      const build = JSON.parse(readFileSync(join(appDir, "build.json"), "utf8"));
      const outputs = outputsOf(build);
      // A fullstack build leaves its image layout as image.tar; its digest is the manifest's,
      // which the Portal checks inside the layout (AP-105, AP-107).
      if (existsSync(join(appDir, "image.tar"))) {
        await uploadArtifact(env.ACTIONS_RESULTS_URL, env.ACTIONS_RUNTIME_TOKEN, `image-${build.commit}`, readFileSync(join(appDir, "image.tar")));
      } else {
        const bundle = await uploadArtifact(env.ACTIONS_RESULTS_URL, env.ACTIONS_RUNTIME_TOKEN, `bundle-${build.commit}`, readFileSync(join(appDir, "bundle.tar.gz")));
        if (bundle !== build.digest) throw new Error(`the bundle uploaded is ${bundle}, not the ${build.digest} that was built`);
      }
      await uploadArtifact(env.ACTIONS_RESULTS_URL, env.ACTIONS_RUNTIME_TOKEN, `sbom-${build.commit}`, readFileSync(join(appDir, "sbom.cdx.json")));
      // The browser checks' report and screenshots, when the App has them (T-2827).
      if (existsSync(join(appDir, "report.tar.gz"))) {
        await uploadArtifact(env.ACTIONS_RESULTS_URL, env.ACTIONS_RUNTIME_TOKEN, `report-${build.commit}`, readFileSync(join(appDir, "report.tar.gz")));
      }
      appendFileSync(env.GITHUB_OUTPUT, outputs);
      console.log(`uploaded the build of ${build.commit} as ${build.digest}`);
    } else if (command === "browser-checks" && appDir && outDir && process.argv[5]) {
      process.exitCode = browserChecks(resolve(appDir), outDir, process.argv[5]);
    } else if (command === "test-project" && appDir && outDir) {
      // One line the Portal reads from the pod's log, whatever happened (SDK-38).
      const timeoutMs = Number.parseInt(process.env.JC_TEST_TIMEOUT_MS ?? "", 10);
      console.log(`JC-TESTS ${JSON.stringify(testProject(appDir, outDir, { timeoutMs: timeoutMs > 0 ? timeoutMs : 150_000 }))}`);
    } else if (command === "seed" && appDir && outDir) {
      console.log(seed(appDir, outDir));
    } else if (command === "lock-manifest" && appDir) {
      const path = join(appDir, "package.json");
      writeFileSync(path, `${JSON.stringify(lockManifest(JSON.parse(readFileSync(path, "utf8"))), null, 2)}\n`);
    } else if (command === "store-check" && appDir && outDir) {
      // The runner images are Debian (glibc); a musl libc would say so in its loader's name.
      const libc = existsSync("/lib/ld-musl-x86_64.so.1") || existsSync("/lib/ld-musl-aarch64.so.1") ? "musl" : "glibc";
      const store = join(outDir, ".pnpm");
      const missing = missingFromStore(readFileSync(appDir, "utf8"), existsSync(store) ? readdirSync(store) : [], {
        os: process.platform,
        cpu: process.arch,
        libc,
      });
      if (missing.length > 0) throw new Error(`the store in ${outDir} lacks ${missing.length} package(s) ${appDir} names: ${missing.join(", ")}`);
      if (!existsSync(join(outDir, "@joinedcontext", "sdk", "package.json"))) throw new Error(`the store in ${outDir} holds no @joinedcontext/sdk`);
      console.log(`the store in ${outDir} holds every package ${appDir} names`);
    } else if (command === "restore" && appDir && outDir && process.argv[5] && process.argv[6]) {
      console.log(restore(appDir, outDir, process.argv[5], process.argv[6]));
    } else if (command === "save" && appDir && outDir && process.argv[5]) {
      console.log(save(appDir, outDir, process.argv[5]));
    } else if (command === "propose" && appDir) {
      // The runner gives every job the Portal's in-cluster address (AP-81).
      const api = process.env.JC_PORTAL_URL ?? "";
      const token = process.env.JC_LANE_TOKEN ?? "";
      if (!/^https?:\/\//.test(api)) throw new Error("JC_PORTAL_URL is not an http(s) URL");
      if (!token) throw new Error("JC_LANE_TOKEN is not set");
      const build = {
        digest: process.env.JC_DIGEST,
        commit: process.env.JC_COMMIT,
        sdkVersion: process.env.JC_SDK_VERSION,
        builtAt: process.env.JC_BUILT_AT,
      };
      outputsOf(build);
      const change = await propose(api, token, appDir, build);
      console.log(`proposed status.build: ${change?.metadata?.name ?? "accepted"}`);
    } else {
      throw new Error(
        "usage: lane.mjs deps <app-dir> | lock-manifest <dir> | store-check <pnpm-lock.yaml> <node_modules> | seed <from> <to> | restore <cache> <key> <seed> <to> | save <from> <cache> <key> | functions <app-dir> <out-dir> | app <owner/repo> | sbom <node_modules> <out> | image <layer.tar> <dir> | server-check <app-dir> | component-check <file.wasm> | upload <build-dir> | browser-checks <app-dir> <bundle-dir> <report-dir> | unstamped <bundle-dir> | propose <owner/repo>",
      );
    }
  } catch (err) {
    console.error(err instanceof Error ? err.message : String(err));
    process.exit(1);
  }
}
