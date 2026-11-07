// node --test scripts/publish-guide-shots.test.mjs (T-3270): the journeys' guide shots into the docs.
import { test } from "node:test";
import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { chmodSync, existsSync, mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const SCRIPT = join(dirname(fileURLToPath(import.meta.url)), "publish-guide-shots.sh");

/** A results dir with `shots` ({lang: [names]}), a docs clone, and a quantizer that copies. */
function setup(shots) {
  const dir = mkdtempSync(join(tmpdir(), "guide-shots-"));
  for (const [lang, names] of Object.entries(shots)) {
    mkdirSync(join(dir, "results", lang), { recursive: true });
    for (const name of names) writeFileSync(join(dir, "results", lang, `${name}.png`), `${lang}:${name}`);
  }
  const docs = join(dir, "docs");
  mkdirSync(join(docs, "User-Guide"), { recursive: true });
  writeFileSync(join(docs, "User-Guide", "00-intro.md"), "# Intro\n");
  const git = (...args) => execFileSync("git", ["-C", docs, ...args], { stdio: "pipe" }).toString().trim();
  git("init", "-q");
  git("config", "user.email", "ci@example.org");
  git("config", "user.name", "ci");
  git("add", "-A");
  git("commit", "-qm", "base");
  // pngquant's own arguments: ... --output OUT -- IN.
  const quant = join(dir, "quant.sh");
  writeFileSync(quant, '#!/bin/sh\nwhile [ "$1" != "--output" ]; do shift; done\ncp "$4" "$2"\n');
  chmodSync(quant, 0o755);
  const run = (env = {}) =>
    spawnSync("sh", [SCRIPT, join(dir, "results"), docs], { env: { ...process.env, PNGQUANT: quant, ...env }, encoding: "utf8" });
  return { docs, git, run };
}

test("puts each shot pair into User-Guide/img and commits once, and nothing when nothing changed", () => {
  const { docs, git, run } = setup({ en: ["space-1", "space-2"], sk: ["space-1", "space-2"] });
  const first = run();
  assert.equal(first.status, 0, first.stderr);
  for (const lang of ["en", "sk"]) assert.ok(existsSync(join(docs, "User-Guide/img", lang, "space-2.png")));
  assert.equal(git("rev-list", "--count", "HEAD"), "2");
  assert.match(git("log", "-1", "--format=%s"), /2 screenshots from the live journeys/);
  const again = run();
  assert.equal(again.status, 0, again.stderr);
  assert.match(again.stdout, /none changed/);
  assert.equal(git("rev-list", "--count", "HEAD"), "2");
});

test("refuses a shot in one language only, a name that is not a shot name, and an empty run", () => {
  for (const [shots, said] of [
    [{ en: ["space-1"], sk: [] }, /en only/],
    [{ en: ["space-1"], sk: ["space-1", "space-2"] }, /sk only/],
    [{ en: ["Space_1"], sk: ["Space_1"] }, /not a shot name/],
    [{ en: [], sk: [] }, /no shot/],
  ]) {
    const { git, run } = setup(shots);
    const result = run();
    assert.notEqual(result.status, 0, JSON.stringify(shots));
    assert.match(result.stderr, said);
    assert.equal(git("rev-list", "--count", "HEAD"), "1", "nothing is committed");
  }
});

test("refuses a shot over the size budget and commits nothing", () => {
  const { git, run } = setup({ en: ["space-1"], sk: ["space-1"] });
  const result = run({ GUIDE_SHOT_MAX_BYTES: "3" });
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /en\/space-1.png is 10 bytes, over 3/);
  assert.equal(git("rev-list", "--count", "HEAD"), "1");
});
