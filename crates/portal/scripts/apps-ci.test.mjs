// node --test scripts/apps-ci.test.mjs (T-3373): which Apps a change makes the fast lane run.
import { test } from "node:test";
import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const SCRIPT = join(dirname(fileURLToPath(import.meta.url)), "apps-ci.sh");

/** A repository with two Apps and the SDK, one commit, then `files` changed in a second. */
function repo(files) {
  const dir = mkdtempSync(join(tmpdir(), "apps-ci-"));
  const git = (...args) => execFileSync("git", ["-C", dir, ...args], { stdio: "pipe" }).toString().trim();
  git("init", "-q");
  git("config", "user.email", "ci@example.org");
  git("config", "user.name", "ci");
  for (const path of ["apps/a/src/x.ts", "apps/ab/src/x.ts", "sdk/src/y.ts", "README.md"]) {
    mkdirSync(join(dir, dirname(path)), { recursive: true });
    writeFileSync(join(dir, path), "1\n");
  }
  mkdirSync(join(dir, "scripts"));
  writeFileSync(join(dir, "scripts/apps-ci.sh"), execFileSync("cat", [SCRIPT]));
  git("add", "-A");
  git("commit", "-qm", "base");
  const base = git("rev-parse", "HEAD");
  for (const path of files) {
    mkdirSync(join(dir, dirname(path)), { recursive: true });
    writeFileSync(join(dir, path), "2\n");
  }
  git("add", "-A");
  git("commit", "-qm", "change", "--allow-empty");
  return { dir, base };
}

const touched = ({ dir, base }, path, env = {}) =>
  spawnSync("sh", ["scripts/apps-ci.sh", "touched", path], { cwd: dir, env: { ...process.env, APPS_BASE: base, APPS_ALL: "", ...env } }).status === 0;

test("a change inside one App runs that App alone, not one whose name starts the same", () => {
  const change = repo(["apps/a/src/x.ts"]);
  assert.equal(touched(change, "apps/a/"), true);
  assert.equal(touched(change, "apps/ab/"), false);
  assert.equal(touched(change, "sdk/examples/react-rust-wasm"), false);
});

test("a change to the SDK, the builder or the gate runs every App", () => {
  for (const path of ["sdk/src/y.ts", "builder/build-wasm.sh", "scripts/app-coverage-run.sh", "apps/coverage-thresholds.json", ".github/workflows/ci.yml"]) {
    assert.equal(touched(repo([path]), "apps/ab/"), true, path);
  }
});

test("a change elsewhere runs no App, and APPS_ALL or an unknown base runs them all", () => {
  const change = repo(["README.md"]);
  assert.equal(touched(change, "apps/a/"), false);
  assert.equal(touched(change, "apps/a/", { APPS_ALL: "1" }), true);
  assert.equal(touched({ ...change, base: "0000000000000000000000000000000000000000" }, "apps/a/"), true);
});

const ours = ({ dir, base }, path, shard) =>
  spawnSync("sh", ["scripts/apps-ci.sh", "ours", path], { cwd: dir, env: { ...process.env, APPS_BASE: base, APPS_ALL: "", APPS_SHARD: shard } }).status === 0;

test("APPS_SHARD puts every touched App in exactly one shard, an untouched one in none", () => {
  const change = repo(["sdk/src/y.ts"]);
  for (const path of ["apps/a/", "apps/ab", "apps/b/ui", "sdk/examples/plain-html-events"]) {
    const shards = ["1/3", "2/3", "3/3"].filter((shard) => ours(change, path, shard));
    assert.equal(shards.length, 1, `${path}: ${shards}`);
    assert.equal(ours(change, path, ""), true, `${path} unsharded`);
  }
  // A trailing slash names the same App, so it never runs twice or not at all.
  assert.equal(ours(change, "apps/a/", "1/3"), ours(change, "apps/a", "1/3"));
  const untouched = repo(["README.md"]);
  assert.equal(["1/3", "2/3", "3/3"].some((shard) => ours(untouched, "apps/a/", shard)), false);
});

test("an install without a lockfile takes no release younger than a day (T-3527)", () => {
  const root = join(dirname(SCRIPT), "..");
  const script = execFileSync("cat", [SCRIPT]).toString();
  const exported = script.indexOf('export npm_config_minimum_release_age="${npm_config_minimum_release_age:-1440}"');
  assert.ok(exported >= 0, "apps-ci.sh sets the release age");
  assert.ok(exported < script.indexOf("--no-frozen-lockfile"), "before its first unlocked install");
  // Every workflow job that installs without a lockfile carries the same setting.
  for (const name of ["ci.yml", "ci-full.yml"]) {
    const jobs = execFileSync("cat", [join(root, ".github/workflows", name)]).toString().split(/\n  (?=[a-z][\w-]*:\n)/);
    for (const job of jobs.filter((body) => body.includes("--no-frozen-lockfile"))) {
      assert.match(job, /\n {4}env:\n(?: {6}.*\n)*? {6}npm_config_minimum_release_age: 1440\n/, `${name}: ${job.split("\n")[0]}`);
    }
  }
});

test("every App's test setup gives findBy*/waitFor 5 s, the lane runs them under coverage (T-4350)", () => {
  const root = join(dirname(fileURLToPath(import.meta.url)), "..");
  const setups = execFileSync("git", ["-C", root, "ls-files", "--", "apps/**/test-setup.ts", "sdk/examples/**/test-setup.ts", "sdk/template/test-setup.ts"])
    .toString().trim().split("\n").filter(Boolean);
  assert.ok(setups.length >= 30, `found only ${setups.length} test setups`);
  const short = setups.filter((path) => !/configure\(\{ asyncUtilTimeout: 5_000 \}\)/.test(execFileSync("cat", [join(root, path)]).toString()));
  assert.deepEqual(short, [], "a test setup keeps the library's 1 s default");
});
