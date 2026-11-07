// node --test scripts/web-performance.test.mjs
import { test } from "node:test";
import assert from "node:assert/strict";
import { judge, median, metrics, PAGES, TARGET } from "./web-performance.mjs";

const report = (lcp, cls, tbt) => ({
  audits: {
    "largest-contentful-paint": { numericValue: lcp },
    "cumulative-layout-shift": { numericValue: cls },
    "total-blocking-time": { numericValue: tbt },
  },
});
const page = { slug: "p", ceiling: { lcp: 4000, cls: 0.1, tbt: 600 } };

test("the median of three runs judges the page, one slow run does not", () => {
  assert.equal(median([3, 1, 2]), 2);
  const runs = [report(2000, 0, 100), report(9000, 0, 100), report(2100, 0, 100)].map(metrics);
  assert.deepEqual(judge(page, runs), { page: "p", failed: [], warned: [], median: { lcp: 2100, cls: 0, tbt: 100 } });
});

test("over the ceiling fails, between the target and the ceiling only warns", () => {
  const verdict = judge(page, [report(3000, 0.2, 700)].map(metrics));
  assert.deepEqual(verdict.failed, ["cls 0.2 over its ceiling 0.1", "tbt 700 over its ceiling 600"]);
  assert.deepEqual(verdict.warned, ["lcp 3000 over the target 2500", "cls 0.2 over the target 0.1", "tbt 700 over the target 300"]);
});

test("a page that did not load, or was not measured, fails", () => {
  assert.equal(metrics({ runtimeError: { code: "NO_FCP" }, audits: {} }), null);
  assert.deepEqual(judge(page, [null]).failed, ["the page did not load in every run"]);
  assert.deepEqual(judge(page, []).failed, ["the page did not load in every run"]);
});

test("every ceiling is at or above the target, so a page that meets the target passes", () => {
  for (const p of PAGES) for (const k of Object.keys(TARGET)) assert.ok(p.ceiling[k] >= TARGET[k], `${p.slug} ${k}`);
});
