/**
 * The space beside one endpoint (T-1435; UI-69, EP-07, EP-61).
 *
 * What a steward cannot otherwise see is the endpoint's effect on real data, so the cases are about
 * exactly that: an attribute the projection hides, an entity the policy or the filter leaves out,
 * two sides that agree, a space this person may not read, and the read itself — the right side is
 * asked for the ids the left side answered, in query strings a gateway will accept.
 */
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import { EntityCompare, DEFAULT_COMPARE_LABELS as L } from "../src/grid/EntityCompare";
import { parseGridConfig } from "../src/grid/config";
import { fixtureSource, idChunks, SourceError } from "../src/grid/source";
import type { EntitySource, GridQuery } from "../src/grid/source";

const TYPE = "AirQualityObserved";
const RADVAN = "urn:ngsi-ld:AirQualityObserved:bb:ovzdusie:radvan-01";
const SASOVA = "urn:ngsi-ld:AirQualityObserved:bb:ovzdusie:sasova-01";

/** What the space holds: two stations, each with a measurement and the sensor behind it. */
const IN_SPACE = [
  {
    id: RADVAN,
    type: TYPE,
    pm10: { type: "Property", value: 12, unitCode: "GQ" },
    source: { type: "Property", value: "sensor-A" },
  },
  {
    id: SASOVA,
    type: TYPE,
    pm10: { type: "Property", value: 30, unitCode: "GQ" },
    source: { type: "Property", value: "sensor-B" },
  },
];

const config = parseGridConfig({
  source: { kind: "fixture", name: "test" },
  type: TYPE,
  pageSize: 10,
}).config!;

/** A source that records what it was asked for, so the reads themselves can be asserted. */
function watched(source: EntitySource) {
  const asked: GridQuery[] = [];
  return {
    asked,
    source: {
      ...source,
      query: (q: GridQuery, page: { offset: number; limit: number }) => {
        asked.push(q);
        return source.query(q, page);
      },
    } as EntitySource,
  };
}

function show(right: EntitySource, left: EntitySource = fixtureSource(IN_SPACE)) {
  return render(<EntityCompare config={config} left={left} right={right} />);
}

const sideOf = (name: string) => screen.getByRole("heading", { name }).parentElement as HTMLElement;

describe("the space beside an endpoint", () => {
  it("shades an attribute the projection hides, and shows nothing of it on the right", async () => {
    // The endpoint answers both entities without `source`, which is what a hidden attribute is.
    const projected = IN_SPACE.map(({ source: _source, ...rest }) => rest);
    show(fixtureSource(projected));

    await waitFor(() => expect(screen.getAllByText("12 µg/m³").length).toBe(2));
    const left = sideOf(L.left);
    const shaded = within(left)
      .getAllByRole("gridcell")
      .filter((cell) => cell.getAttribute("data-mark") === "column");
    // One shaded cell per row, and it says why.
    expect(shaded).toHaveLength(2);
    expect(shaded[0]).toHaveAttribute("title", L.hiddenColumn);
    expect(shaded.map((cell) => cell.textContent)).toEqual(["sensor-A", "sensor-B"]);
    // Nothing of the hidden attribute is drawn on the right, not even an empty column.
    expect(within(sideOf(L.right)).queryByText("sensor-A")).toBeNull();
  });

  it("strikes through an entity the endpoint does not answer", async () => {
    show(fixtureSource([IN_SPACE[0]]));

    await waitFor(() => expect(screen.getAllByText("12 µg/m³").length).toBe(2));
    const rows = within(sideOf(L.left)).getAllByRole("row");
    const struck = rows.filter((row) => row.getAttribute("data-mark") === "row");
    expect(struck).toHaveLength(1);
    // The row reads by its URN's end, the whole id its opener's tooltip (T-5619).
    expect(within(struck[0]).getByRole("button", { name: `Open: ${SASOVA.split(":").at(-1)}` })).toHaveAttribute("title", SASOVA);
    expect(struck[0]).toHaveAttribute("title", L.missingRow);
    // The endpoint's own side holds one row and says nothing about the other.
    expect(within(sideOf(L.right)).queryByText(SASOVA)).toBeNull();
  });

  it("marks nothing, and says so, when the endpoint answers the page as the space holds it", async () => {
    show(fixtureSource(IN_SPACE));

    expect(await screen.findByText(L.same)).toBeInTheDocument();
    expect(document.querySelectorAll('[data-mark]')).toHaveLength(0);
  });

  it("asks the endpoint for exactly the ids the space answered", async () => {
    const right = watched(fixtureSource(IN_SPACE));
    show(right.source);

    await waitFor(() => expect(right.asked).toHaveLength(1));
    expect(right.asked[0].ids).toEqual([RADVAN, SASOVA]);
    expect(right.asked[0].type).toBe(TYPE);
  });

  it("leaves the endpoint side alone, with the reason, when the space answers this person nothing", async () => {
    // SP-06: the surface's 404 never says whether the space exists.
    const refused: EntitySource = {
      query: () => Promise.reject(new SourceError(404, "no grant on this space")),
      get: async () => null,
    };
    show(fixtureSource(IN_SPACE), refused);

    expect(await screen.findByText(L.leftRefused)).toBeInTheDocument();
    // The endpoint half is still there, whole.
    await waitFor(() => expect(within(sideOf(L.right)).getByText("12 µg/m³")).toBeInTheDocument());
    expect(screen.queryByText(L.same)).toBeNull();
  });

  it("says what went wrong when the endpoint itself refuses", async () => {
    const refused: EntitySource = {
      query: () => Promise.reject(new SourceError(403, "this endpoint is not yours to read")),
      get: async () => null,
    };
    show(refused);

    expect(await screen.findByText(/this endpoint is not yours to read/)).toBeInTheDocument();
  });

  it("splits the id list so no query string grows past what a gateway accepts", () => {
    const many = Array.from({ length: 200 }, (_, i) => `urn:ngsi-ld:${TYPE}:bb:ovzdusie:station-${i}`);
    const chunks = idChunks(many);
    expect(chunks.length).toBeGreaterThan(1);
    expect(chunks.flat()).toEqual(many);
    for (const chunk of chunks) {
      expect(chunk.map((id) => encodeURIComponent(id)).join("%2C").length).toBeLessThan(4_000);
    }
    // One page of a grid is one read.
    expect(idChunks(many.slice(0, 25))).toHaveLength(1);
  });

  it("stacks the two sides on a narrow screen", async () => {
    show(fixtureSource(IN_SPACE));
    await waitFor(() => expect(screen.getAllByText("12 µg/m³").length).toBe(2));
    // Both sides are children of one CSS grid, so the stacking is one rule and not a second layout.
    const split = document.querySelector(".jc-compare") as HTMLElement;
    expect(split.children).toHaveLength(2);
    // jsdom applies no stylesheet, so the rule itself is read where it lives.
    const css = readFileSync("src/grid/grid.css", "utf8");
    const stacked = css.slice(css.indexOf("@media (width <= 900px)"));
    expect(stacked).toContain(".jc-compare {");
    expect(stacked).toContain("grid-template-columns: 1fr;");
  });
});
