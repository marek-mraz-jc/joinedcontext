/**
 * The grid as a view of a generated application (T-1440; SDK-30, UI-71).
 *
 * One configuration object for both places the grid appears: `parseGridConfig` validates it here
 * and in the Portal, so a person writing `spec.json` by hand reads the findings the editor shows,
 * at the path of their own file.
 */
import { render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import example from "../spec.example.json";
import { parseSpec } from "../src/spec";
import type { Spec } from "../src/spec";
import { GridView } from "../src/views/GridView";

const SOURCES = [{ name: "s", type: "T", attrs: ["a", "b"] }];
const spec = (views: unknown[]) => ({ title: "x", sources: SOURCES, views });

describe("a grid view of a spec", () => {
  it("parses with nothing but its kind: the app's endpoint and the source's type are enough", () => {
    const parsed = parseSpec(spec([{ kind: "grid" }]));
    expect(parsed.errors).toEqual([]);
    expect(parsed.spec?.views[0].kind).toBe("grid");
  });

  it("carries the grid's own configuration", () => {
    const parsed = parseSpec(
      spec([
        {
          kind: "grid",
          title: "Every station",
          grid: {
            columns: [{ attr: "a", format: "number" }, { attr: "b" }],
            pageSize: 25,
            mode: "edit",
            editableAttrs: ["a"],
            history: { enabled: true },
            density: "compact",
          },
        },
      ]),
    );
    expect(parsed.errors).toEqual([]);
  });

  it("reports a bad grid config under the view's own path", () => {
    const parsed = parseSpec(
      spec([{ kind: "grid", grid: { pageSize: 5001, columns: [{ attr: "" }], nope: true } }]),
    );
    expect(parsed.spec).toBeNull();
    expect(parsed.errors).toEqual([
      "views[0].grid.nope: unknown property",
      "views[0].grid.columns.0.attr: must be a non-empty string",
      "views[0].grid.pageSize: must be an integer between 1 and 5000",
    ]);
  });

  it("refuses a grid that names its own source or type: the app decides both", () => {
    const parsed = parseSpec(
      spec([{ kind: "grid", grid: { source: { kind: "endpoint", slug: "another" }, type: "Other" } }]),
    );
    expect(parsed.errors).toEqual([
      "views[0].grid.source: is the view's own source; leave it out",
      "views[0].grid.type: is the view's own source; leave it out",
    ]);
  });

  it("refuses a grid that names a second source to compare against (T-1440)", () => {
    // The explorer offers the comparison because a person chose both sides on screen; a spec that
    // wrote it would read another endpoint from inside the app, and jc_core's GridConfig has no
    // such field, so the manifest would be refused by the platform anyway.
    const parsed = parseSpec(
      spec([{ kind: "grid", grid: { compareWith: { kind: "endpoint", slug: "another" } } } as never]),
    );
    expect(parsed.errors).toEqual([
      "views[0].grid.compareWith: is the view's own source; leave it out",
    ]);
  });

  it("still parses a spec written before the grid existed", () => {
    const parsed = parseSpec(spec([{ kind: "table", columns: ["a"] }]));
    expect(parsed.errors).toEqual([]);
  });

  it("is in the example the bundle ships", () => {
    const parsed = parseSpec(example);
    expect(parsed.errors).toEqual([]);
    expect((parsed.spec as Spec).views.some((view) => view.kind === "grid")).toBe(true);
  });
});

describe("the grid view rendered", () => {
  const rows = [
    { id: "urn:ngsi-ld:T:one", type: "T", a: { type: "Property", value: 5, unitCode: "C62" } },
    { id: "urn:ngsi-ld:T:two", type: "T", a: { type: "Property", value: 7, unitCode: "C62" } },
  ];

  it("shows the entities of a fixture without fetching anything", async () => {
    render(<GridView slug="demo" type="T" config={{ columns: [{ attr: "a" }] }} inline={rows} />);

    expect(await screen.findByRole("grid")).toBeInTheDocument();
    await waitFor(() => {
      expect(screen.getByText("5")).toBeInTheDocument();
    });
    expect(screen.getByText("7")).toBeInTheDocument();
  });

  it("says which field is wrong instead of drawing an empty table", async () => {
    // `pageSize` past the ceiling: what a hand-written spec that never met the parser carries.
    render(
      <GridView
        slug="demo"
        type="T"
        config={{ pageSize: 9000 } as never}
        inline={rows}
      />,
    );
    expect(await screen.findByText(/pageSize/)).toBeInTheDocument();
    expect(screen.queryByRole("grid")).toBeNull();
  });
});
