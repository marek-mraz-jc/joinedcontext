import assert from "node:assert/strict";
import { test } from "node:test";
import { SECTIONS, bundle, plain, sectionBlocks } from "./guide-sections.mjs";

const PAGE = `---
title: X
---

# X

## 1. First

Not this one.

## 2. Publishing an Endpoint

Open **Endpoints** and read [the API](../API/x.md) with \`curl\`.
It goes on.

- one
  wrapped
- two

1. first
2. second

:::tip
A tip.
:::

| a | b |
|---|---|

![shot](img/x.png)

### Audiences

\`\`\`bash
curl /api/v1
\`\`\`

\`\`\`mermaid
graph TD
\`\`\`

## 3. Next
Not this either.
`;

test("a section becomes plain text blocks and stops at the next H2", () => {
  const { heading, blocks } = sectionBlocks(PAGE, 2);
  assert.equal(heading, "Publishing an Endpoint");
  assert.deepEqual(blocks, [
    ["p", "Open Endpoints and read the API with curl. It goes on."],
    ["ul", ["one wrapped", "two"]],
    ["ol", ["first", "second"]],
    ["p", "A tip."],
    ["h", "Audiences"],
    ["pre", "curl /api/v1"],
  ]);
});

test("inline markup and HTML leave only their words", () => {
  assert.equal(plain("a <script>x</script> *b* __c__ `d` [e](f) ![g](h) snake_case_name"), "a x b c d e snake_case_name");
});

test("a missing section and a short pin are refused", () => {
  assert.throws(() => sectionBlocks(PAGE, 9), /no section "## 9\."/);
  assert.throws(() => bundle("main", () => PAGE), /full docs commit sha/);
});

test("bundle reads every helped page's section under the pin", () => {
  const read = (path) => {
    assert.ok(path.startsWith("User-Guide/") && path.endsWith(".md"));
    return Array.from({ length: 8 }, (_, i) => `## ${i + 1}. S${i + 1}\n\nText ${i + 1}.\n`).join("\n");
  };
  const out = bundle("a".repeat(40), read);
  assert.equal(out.docsCommit, "a".repeat(40));
  assert.deepEqual(Object.keys(out.sections), Object.keys(SECTIONS));
  assert.deepEqual(out.sections.policies.blocks, [["p", "Text 6."]]);
});
