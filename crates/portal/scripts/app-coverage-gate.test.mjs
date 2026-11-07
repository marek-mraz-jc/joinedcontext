// node --test scripts/app-coverage-gate.test.mjs (T-3373)
import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { addedLines, parsePending, readControls, shortfalls, thresholds, verdict } from "./app-coverage-gate.mjs";

const LIMITS = { lines: 95, statements: 95, functions: 95, branches: 90, rustLines: 95 };
const summary = (pct) => ({ total: { lines: { pct }, statements: { pct }, functions: { pct }, branches: { pct } } });
const touchedAll = { untouched: [] };

test("the thresholds are the shared ones and none is below the owner's line", () => {
  const shared = thresholds();
  assert.deepEqual(shared, LIMITS);
});

test("an App below a threshold fails, one above passes", () => {
  const pending = new Map();
  const low = verdict("a", { summary: summary(80), controls: touchedAll }, pending, LIMITS);
  assert.equal(low.ok, false);
  assert.ok(low.lines.some((line) => line.startsWith("lines 80 %")));
  assert.deepEqual(verdict("a", { summary: summary(96), controls: touchedAll }, pending, LIMITS), { ok: true, lines: [] });
  // Branches have their own line: 92 passes it, 89 does not.
  const branches = { total: { lines: { pct: 99 }, statements: { pct: 99 }, functions: { pct: 99 }, branches: { pct: 89 } } };
  assert.deepEqual(shortfalls({ summary: branches, controls: touchedAll }, LIMITS), ["branches 89 % < 90 %"]);
});

test("a control no test exercised fails the App", () => {
  const dir = mkdtempSync(join(tmpdir(), "gate-"));
  writeFileSync(join(dir, "one.json"), JSON.stringify({ rendered: ["button: Save", "button: Delete"], exercised: ["button: Save"] }));
  writeFileSync(join(dir, "two.json"), JSON.stringify({ rendered: ["link: Help"], exercised: ["link: Help"] }));
  const controls = readControls(dir);
  assert.deepEqual(controls, { untouched: ["button: Delete"] });
  const result = verdict("a", { summary: summary(99), controls }, new Map(), LIMITS);
  assert.equal(result.ok, false);
  assert.deepEqual(result.lines, ["control no test exercises: button: Delete"]);
});

test("an App with no report at all fails rather than passing by silence", () => {
  const result = verdict("a", {}, new Map(), LIMITS);
  assert.equal(result.ok, false);
  assert.equal(result.lines.length, 2);
  assert.equal(readControls(join(tmpdir(), "no-such-dir-for-the-gate")), undefined);
});

test("Rust below its line fails; a run without a Rust part is not asked for one", () => {
  const rust = { data: [{ totals: { lines: { percent: 90.25 } } }] };
  assert.deepEqual(shortfalls({ summary: summary(99), controls: touchedAll, rust }, LIMITS), ["rust lines 90.3 % < 95 %"]);
  assert.deepEqual(shortfalls({ summary: summary(99), controls: touchedAll }, LIMITS), []);
});

test("a pending App is reported, not failed, and a pending App that passes must leave the list", () => {
  const pending = new Map([["a", "T-3374"]]);
  const low = verdict("a", { summary: summary(50), controls: touchedAll }, pending, LIMITS);
  assert.equal(low.ok, true);
  assert.ok(low.lines.every((line) => line.startsWith("pending (T-3374): ")));
  const done = verdict("a", { summary: summary(99), controls: touchedAll }, pending, LIMITS);
  assert.equal(done.ok, false);
  assert.match(done.lines[0], /remove its line/);
});

test("the pending list only shrinks and every line names its task", () => {
  assert.deepEqual(addedLines("a T-1\nb T-2\n", "a T-1\n"), []);
  assert.deepEqual(addedLines("a T-1\n", "a T-1\nc T-3\n"), ["c T-3"]);
  assert.deepEqual([...parsePending("# note\n\na T-1 # why\n").entries()], [["a", "T-1"]]);
  assert.throws(() => parsePending("a\n"), /not "<app> <T-id>"/);
  assert.throws(() => parsePending("A T-1\n"), /not "<app> <T-id>"/);
});
