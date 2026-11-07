/**
 * T-4950: the live walker's reports, turned into the summary `tasks/file-failures` files one task
 * per finding from. The board deduplicates by key, so a repeated finding must keep its key.
 */
import { describe, expect, it } from "vitest";
import { routeKey, ruleAndLabel, summarise, walkError } from "../scripts/walker-summary";
import type { WalkEntry } from "../scripts/walker-summary";

const P = "/projects/helsinki";
const page = (address: string, route: string, width: number, findings: string[]): WalkEntry => ({
  route,
  address,
  width,
  lang: "en",
  findings,
  screenshot: `/shots/${width}${address.replace(/\W+/g, "-")}.png`,
});

describe("the walker summary (T-4950)", () => {
  it("keeps the section and tab of a route and drops the item a walk opened", () => {
    expect(routeKey("/projects/$project/$plural/$name/edit", `${P}/spaces/air/edit`)).toBe("/projects/$project/spaces/$name/edit");
    expect(routeKey("/organization/$tab/$", "/organization/people/new")).toBe("/organization/people/new");
    expect(routeKey("/organization/$tab/$", "/organization/people/p-17")).toBe("/organization/people/$");
    expect(routeKey("/", "/?lang=en")).toBe("/");
  });

  it("takes data out of a label", () => {
    expect(ruleAndLabel('raw-json: {"type":"Point"}')).toEqual(["raw-json", ""]);
    expect(ruleAndLabel("http: 500 GET /api/v1/projects/helsinki/spaces/urn:ngsi-ld:A:1")).toEqual(["http", "500 GET /api/v1/projects/*/spaces/*"]);
    expect(ruleAndLabel("field-hint: Entity type")).toEqual(["field-hint", ""]);
    expect(ruleAndLabel("axe: scrollable-region-focusable (serious): Scrollable region — pre")).toEqual(["axe", "scrollable-region-focusable"]);
  });

  it("files a repeated finding once, across widths, people, items and the fields of one form", () => {
    const route = "/projects/$project/$plural/$name/edit";
    const steward = [
      page(`${P}/spaces/air/edit`, route, 1440, ["field-hint: Entity type", "field-hint: Where"]),
      page(`${P}/spaces/air/edit`, route, 400, ["field-hint: Entity type"]),
      page(`${P}/endpoints/air/edit`, route, 1440, ["field-hint: Entity type"]),
    ];
    const viewer = [page(`${P}/spaces/water/edit`, route, 400, ["field-hint: Entity type"])];
    const summary = summarise({ steward, viewer }, "run-1", { merged: ["T-4949", "T-4945"] });
    const failing = summary.results.filter((result) => result.verdict === "fail");
    expect(failing.map((result) => result.key)).toEqual([
      "field-hint /projects/$project/spaces/$name/edit",
      "field-hint /projects/$project/endpoints/$name/edit",
    ]);
    expect(failing[0].detail).toContain("Found as: field-hint: Entity type | field-hint: Where");
    expect(failing[0].detail).toContain("steward 1440px en /projects/helsinki/spaces/air/edit");
    expect(failing[0].detail).toContain("viewer 400px en /projects/helsinki/spaces/water/edit");
    expect(failing[0].detail).toContain("Merged since the last clean walk: T-4949, T-4945.");
    expect(summary.check).toBe("walker");
  });

  it("passes a finding of the previous run that is gone from a route this run walked", () => {
    const route = "/projects/$project/$plural/new";
    const before = summarise({ steward: [page(`${P}/spaces/new`, route, 1440, ["field-hint: Name"])], viewer: [page("/", "/", 1440, [])] }, "run-1");
    const after = summarise({ steward: [page(`${P}/spaces/new`, route, 1440, [])], viewer: [page("/", "/", 1440, [])] }, "run-2", { previous: before });
    expect(after.results.filter((result) => result.key.startsWith("field-hint"))).toEqual([
      expect.objectContaining({ key: "field-hint /projects/$project/spaces/new", verdict: "pass" }),
    ]);
    // A route this run did not reach keeps its task open.
    const partial = summarise({ steward: [page("/", "/", 1440, [])], viewer: [page("/", "/", 1440, [])] }, "run-3", { previous: before });
    expect(partial.results.some((result) => result.key.startsWith("field-hint"))).toBe(false);
  });

  it("reports a person's walk that wrote nothing as an error, not as a clean page", () => {
    const summary = summarise({ steward: [page("/", "/", 1440, [])] }, "run-1");
    expect(summary.results).toContainEqual(expect.objectContaining({ key: "run viewer", verdict: "error" }));
    expect(summary.results).toContainEqual(expect.objectContaining({ key: "run steward", verdict: "pass" }));
  });

  it("names the walk log's error when a walk wrote nothing, and a crashed tab as host memory (T-5993)", () => {
    const log = "Running 1 test\n\n    Error: page.waitForTimeout: Page crashed\n\n       at portal.ts:412\n    Error: later";
    const summary = summarise({ steward: [page("/", "/", 1440, [])] }, "run-1", { logs: { viewer: log } });
    const viewer = summary.results.find((result) => result.key === "run viewer");
    expect(viewer?.detail).toMatch(/^Error: page.waitForTimeout: Page crashed A crashed tab is Chromium's renderer killed under host memory pressure/);
    expect(walkError("    Error: page.goto: net::ERR_NAME_NOT_RESOLVED")).toBe("Error: page.goto: net::ERR_NAME_NOT_RESOLVED");
    expect(walkError(undefined)).toBe("See the run log beside the evidence.");
  });
});
