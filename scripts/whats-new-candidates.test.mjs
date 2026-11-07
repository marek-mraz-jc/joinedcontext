// node --test scripts/whats-new-candidates.test.mjs
import { test } from "node:test";
import assert from "node:assert/strict";
import { candidate } from "./whats-new-candidates.mjs";

const task = (repo, group, merged) =>
  `---\nid: T-1\ntitle: "Search everything"\nrepo: ${repo}\ngroup: ${group}\n---\n` +
  (merged ? `## ${merged} (integrator): merged on main of ${repo} as abc\n` : "");

test("a Portal task merged since the day, in a group a person sees, is a candidate", () => {
  assert.deepEqual(candidate("T-3238-search.md", task("joinedcontext-portal", "ease-navigation", "2026-10-07"), "2026-09-23"), {
    id: "T-3238",
    merged: "2026-10-07",
    group: "ease-navigation",
    title: "Search everything",
  });
});

test("another repository, an unseen group, an older merge or no merge at all is not", () => {
  assert.equal(candidate("T-1.md", task("joinedcontext-platform", "ease-navigation", "2026-10-07"), "2026-09-23"), null);
  assert.equal(candidate("T-1.md", task("joinedcontext-portal", "ci-red", "2026-10-07"), "2026-09-23"), null);
  assert.equal(candidate("T-1.md", task("joinedcontext-portal", "security-findings", "2026-10-07"), "2026-09-23"), null);
  assert.equal(candidate("T-1.md", task("joinedcontext-portal", "ease-navigation", "2026-09-01"), "2026-09-23"), null);
  assert.equal(candidate("T-1.md", task("joinedcontext-portal", "ease-navigation", null), "2026-09-23"), null);
});
