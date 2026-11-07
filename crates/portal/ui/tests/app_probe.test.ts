/** T-2795 (AP-136): the App probe's verdict per App, from what it saw. */
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { firstViewItems, summaryOf, verdictOf } from "../scripts/app-probe";
import type { Layout, Observation } from "../scripts/app-probe";

const WORKING: Observation = {
  project: "helsinki",
  name: "air-quality",
  visibility: "project",
  refused: false,
  dataMs: 2400,
  windowDataMs: 1800,
  consoleErrors: [],
  failedRequests: [],
  layout: [375, 768, 1440, 2560].map((width) => ({ width, h1: 1, sideways: false })),
  items: 12,
  anonymous: "refused",
};

const at = (width: number, change: Partial<Layout>): Layout[] =>
  WORKING.layout.map((layout) => (layout.width === width ? { ...layout, ...change } : layout));

const seen = (change: Partial<Observation>): Observation => ({ ...WORKING, ...change });

describe("the App probe's verdict (AP-136)", () => {
  it("passes an App that read data in both places and turned a stranger away", () => {
    expect(verdictOf(WORKING)).toEqual({
      key: "helsinki/air-quality",
      verdict: "pass",
      title: "opened with data in 2.4 s",
    });
  });

  it.each([
    [{ dataMs: null }, "no row read inside the Portal in 60 s"],
    [{ windowDataMs: null }, "no row read in its own window in 60 s"],
    [{ consoleErrors: ["TypeError: x is undefined\n  at main.js:1"] }, "1 console error: TypeError: x is undefined at main.js:1"],
    [{ failedRequests: ["404 https://x.apps.dev.example/favicon.svg"] }, "1 failed request: 404 https://x.apps.dev.example/favicon.svg"],
    [{ layout: at(768, { h1: 0 }) }, "0 h1 at 768 px, one expected"],
    [{ layout: at(375, { h1: 2 }) }, "2 h1 at 375 px, one expected"],
    [{ layout: at(2560, { sideways: true }) }, "scrolls sideways at 2560 px"],
    [{ items: 0 }, "its first view shows no data item"],
    [{ visibility: "public", anonymous: "refused" }, "a visitor who did not sign in read nothing"],
    [{ anonymous: "data" }, "a visitor who did not sign in read its data"],
    [{ anonymous: "blank" }, "a visitor who did not sign in was not sent to sign in"],
  ] as const)("fails %o as %s", (change, title) => {
    expect(verdictOf(seen(change as Partial<Observation>))).toMatchObject({ verdict: "fail", title });
  });

  it("skips an App whose default group the probe is not in, rather than calling it broken", () => {
    expect(verdictOf(seen({ refused: true, dataMs: null }))).toMatchObject({ verdict: "skip" });
  });

  it("cuts a long console error to one line in the title and keeps every error in the detail", () => {
    const result = verdictOf(seen({ consoleErrors: ["x".repeat(500), "second"] }));
    expect(result.title.length).toBeLessThan(140);
    expect(result.detail?.split("\n")).toHaveLength(2);
  });

  it("does not hold an unmeasured first view or a missing own window against an App", () => {
    expect(verdictOf(seen({ items: null, layout: [] })).verdict).toBe("pass");
  });

  it("a public App read by a stranger passes", () => {
    expect(verdictOf(seen({ visibility: "public", anonymous: "data" })).verdict).toBe("pass");
  });

  it("writes the summary of the check apps", () => {
    const summary = summaryOf([WORKING, seen({ name: "alerts", dataMs: null })], "probe 1");
    expect(summary.check).toBe("apps");
    expect(summary.results.map((r) => [r.key, r.verdict])).toEqual([
      ["helsinki/air-quality", "pass"],
      ["helsinki/alerts", "fail"],
    ]);
  });
});

describe("the data items of a first view (T-3580)", () => {
  const recorded = (name: string): Document =>
    new DOMParser().parseFromString(readFileSync(join(__dirname, "fixtures", "app-probe", `${name}.html`), "utf8"), "text/html");

  it("finds none on a recorded page that has nothing to show", () => {
    expect(firstViewItems(recorded("empty"))).toBe(0);
  });

  it("counts the rows of a recorded page that shows its data", () => {
    expect(firstViewItems(recorded("filled"))).toBeGreaterThanOrEqual(25);
  });

  it("counts markers, chart marks, marked items, stats above zero and drawn canvases, never a header row", () => {
    const page = new DOMParser().parseFromString(
      `<table><thead><tr><th>Name</th></tr></thead><tbody><tr><td> </td></tr></tbody></table>
       <div class="maplibregl-marker"></div><svg><g class="recharts-dot"></g></svg><li data-item>a</li>
       <dl><dd>0</dd><dd>12 345</dd><dd>—</dd></dl><output>1,5</output><canvas></canvas>`,
      "text/html",
    );
    expect(firstViewItems(page)).toBe(6);
  });
});
