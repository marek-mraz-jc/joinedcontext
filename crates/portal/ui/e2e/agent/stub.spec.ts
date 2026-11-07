/**
 * T-5981 — the user agent's loop on a stub page with no model: recorded answers drive
 * runScenario through a search, a bad answer (retried), a new tab and back, a held write and done, so CI proves the
 * runner (tree, actions, shots, write hold, run JSON) without a network call or a key.
 */
import { mkdtempSync, readFileSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { expect, test } from "@playwright/test";
import { holdWrites, mcpReadOnly, recordedModel, runScenario } from "./run";

const PAGE = `<!doctype html><html lang="en"><title>Open data</title><main>
<h1>Open data</h1>
<label>Search datasets <input type="search" id="q"></label>
<button id="go">Search</button>
<ul id="hits"></ul>
<a href="/licence" target="_blank">Licence</a>
<button id="propose" hidden>Propose dataset</button>
<p id="said" role="status"></p>
<script>
document.getElementById("go").onclick = () => {
  const q = document.getElementById("q").value;
  document.getElementById("hits").innerHTML = q ? '<li><a href="#air">Air quality ' + q + '</a></li>' : "";
  document.getElementById("propose").hidden = false;
  fetch("/api/v1/projects/p/drafts/DataSource/air", { method: "PUT", body: "{}" });
};
document.getElementById("propose").onclick = async () => {
  const answer = await fetch("/api/v1/projects/p/datasources", { method: "POST", body: "{}" });
  document.getElementById("said").textContent = "Proposal answered " + answer.status;
};
</script></main></html>`;

test("the user agent plays recorded actions to done, with one retry and one held write", async ({
  page,
  context,
}) => {
  const dir = mkdtempSync(join(tmpdir(), "ui-agent-"));
  const held: string[] = [];
  const drafts = new Set<string>();
  // A draft passes the hold (to this stub answer) and is recorded for removeDrafts.
  await context.route(
    "https://stub.test/api/v1/projects/p/drafts/**",
    (route) => route.fulfill({ status: 200, body: "{}" }),
  );
  await holdWrites(context, ["stub.test"], held, drafts);
  await context.route("https://stub.test/", (route) =>
    route.fulfill({ contentType: "text/html", body: PAGE }),
  );
  await context.route("https://stub.test/licence", (route) =>
    route.fulfill({
      contentType: "text/html",
      body: "<title>Licence</title><h1>CC BY 4.0</h1>",
    }),
  );
  await page.goto("https://stub.test/");
  const model = recordedModel([
    '{"kind":"fill","role":"searchbox","name":"Search datasets","value":"Bystrica"}',
    "I think I should click Search now.",
    '{"kind":"click","role":"button","name":"Search"}',
    '{"kind":"click","role":"link","name":"Licence"}',
    '{"kind":"back"}',
    '{"kind":"click","role":"button","name":"Propose dataset"}',
    '{"kind":"done","answer":"Found Air quality Bystrica; the proposal was held"}',
  ]);
  const run = await runScenario({
    page,
    scenario: {
      id: "S-STUB",
      segment: "ui",
      as: "citizen",
      says: "Find air quality",
      expect: "done in <= 4 steps",
    },
    persona: "citizen",
    width: 1440,
    lang: "en",
    model,
    dir,
    hosts: ["stub.test"],
    held,
  });
  expect(run.outcome).toBe("done");
  expect(run.budget).toBe(4);
  expect(run.steps.map((s) => s.action.kind)).toEqual([
    "fill",
    "click",
    "click",
    "back",
    "click",
    "done",
  ]);
  expect(run.steps[1].retried).toBe(true);
  expect(run.heldWrites).toEqual(["POST /api/v1/projects/p/datasources"]);
  expect([...drafts]).toEqual(["/api/v1/projects/p/drafts/DataSource/air"]);
  expect(run.steps[2].result).toContain(
    "a new tab opened, now on https://stub.test/licence",
  );
  expect(run.steps[3].result).toBe(
    "closed the tab, back on https://stub.test/",
  );
  expect(run.steps[4].result).toContain(
    "held POST /api/v1/projects/p/datasources",
  );
  await expect(page.getByRole("status")).toHaveText("Proposal answered 409");
  expect(run.steps.every((s) => existsSync(join(dir, s.shot)))).toBe(true);
  expect(JSON.parse(readFileSync(join(dir, "run.json"), "utf8")).outcome).toBe(
    "done",
  );
});

test("the user agent stops at twice the step budget and on a missing control", async ({
  page,
}) => {
  const dir = mkdtempSync(join(tmpdir(), "ui-agent-"));
  await page.route("https://stub.test/", (route) =>
    route.fulfill({ contentType: "text/html", body: PAGE }),
  );
  await page.goto("https://stub.test/");
  const model = recordedModel(
    Array(10).fill('{"kind":"click","role":"button","name":"Nowhere"}'),
  );
  const run = await runScenario({
    page,
    scenario: {
      id: "S-STUB",
      segment: "ui",
      as: "citizen",
      says: "Loop",
      expect: "done in <= 2 steps",
    },
    persona: "citizen",
    width: 400,
    lang: "en",
    model,
    dir,
    hosts: ["stub.test"],
  });
  expect(run.outcome).toBe("budget");
  expect(run.steps).toHaveLength(4);
  expect(run.steps[0].result).toBe('no button named "Nowhere" is on the page');
});

test("the write hold passes MCP reads and holds MCP writes", () => {
  expect(mcpReadOnly('{"jsonrpc":"2.0","id":1,"method":"tools/list"}')).toBe(
    true,
  );
  expect(
    mcpReadOnly(
      '[{"method":"initialize"},{"method":"tools/call","params":{"name":"query_entities"}}]',
    ),
  ).toBe(true);
  expect(
    mcpReadOnly('{"method":"tools/call","params":{"name":"create_entity"}}'),
  ).toBe(false);
  expect(
    mcpReadOnly(
      '{"method":"tools/call","params":{"name":"jc_change_approve"}}',
    ),
  ).toBe(false);
  expect(mcpReadOnly("not json")).toBe(false);
});
