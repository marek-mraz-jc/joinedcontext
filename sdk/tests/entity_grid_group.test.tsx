import { describe, expect, it } from "vitest";
import { render, waitFor } from "@testing-library/react";
import { EntityGrid } from "../src/grid/EntityGrid";
import type { GridGroup } from "../src/grid/EntityGrid";
import { parseGridConfig } from "../src/grid/config";
import { fixtureSource, orderRows } from "../src/grid/source";
import { orderKeys } from "../src/grid/useEntityGrid";
import { toRichRow } from "../src/grid/model";

// T-5188: a sort of up to three keys and rows grouped by up to three fields, each group under a header.
const station = (n: number, status: string | null, city: string, name: string): Record<string, unknown> => ({
  id: `urn:ngsi-ld:Station:${n}`,
  type: "Station",
  ...(status === null ? {} : { status: { type: "Property", value: status } }),
  city: { type: "Property", value: city },
  name: { type: "Property", value: name },
});
const stations = [
  station(1, "open", "Kosice", "B"),
  station(2, "closed", "Zilina", "A"),
  station(3, "open", "Kosice", "A"),
  station(4, null, "Kosice", "C"),
  station(5, "open", "Zilina", "D"),
];
const config = parseGridConfig({
  source: { kind: "fixture", name: "test" },
  type: "Station",
  columns: [
    { attr: "name", label: "Name" },
    { attr: "status", label: "Status" },
    { attr: "city", label: "City" },
  ],
  pageSize: 10,
}).config!;

describe("order keys", () => {
  it("puts the group keys first, takes the sort's direction for a grouped attribute and names each attribute once", () => {
    expect(orderKeys(["status", "city"], { attr: "city", dir: "desc" }, [{ attr: "name", dir: "asc" }])).toEqual([
      { attr: "status", dir: "asc" },
      { attr: "city", dir: "desc" },
      { attr: "name", dir: "asc" },
    ]);
    expect(orderKeys([], null, [])).toEqual([]);
  });

  it("orders a fixture by several keys, an absent value last", () => {
    const rows = stations.map((entity) => toRichRow(entity));
    expect(orderRows(rows, "status,name;desc").map((row) => row.id.split(":").at(-1))).toEqual(["2", "5", "1", "3", "4"]);
  });
});

describe("grouped grid", () => {
  it("asks the source for the groups then the sort, and draws a header where a group starts", async () => {
    const asked: (string | undefined)[] = [];
    const fixture = fixtureSource(stations);
    const source = {
      ...fixture,
      query: async (q: Parameters<typeof fixture.query>[0], page: { offset: number; limit: number }) => {
        asked.push(q.orderBy);
        return fixture.query(q, page);
      },
    };
    const groups: GridGroup[] = [];
    const { container } = render(
      <EntityGrid
        config={config}
        source={source}
        state={{ sort: { attr: "name", dir: "asc" } }}
        thenBy={[{ attr: "city", dir: "desc" }]}
        groupBy={["status", "city"]}
        groupHeader={(group) => {
          groups.push(group);
          return <span>{`n=${group.path.length}`}</span>;
        }}
      />,
    );
    await waitFor(() => expect(asked.at(-1)).toBe("status,city;desc,name"));
    await waitFor(() => expect(container.querySelectorAll(".jc-grid-group").length).toBeGreaterThan(0));
    const headers = [...container.querySelectorAll(".jc-grid-group")].map((row) => [row.getAttribute("data-level"), row.textContent]);
    // closed: Zilina; open: Zilina, Kosice; no status: Kosice — an outer header, then its inner ones.
    expect(headers).toEqual([
      ["0", "Status closedn=1"],
      ["1", "City Zilinan=2"],
      ["0", "Status openn=1"],
      ["1", "City Zilinan=2"],
      ["1", "City Kosicen=2"],
      ["0", "Status (no value)n=1"],
      ["1", "City Kosicen=2"],
    ]);
    expect(groups.find((group) => group.level === 0 && group.path[0].value === undefined)?.attr).toBe("status");
    expect(groups.find((group) => group.level === 1 && group.path[1].value === "Kosice")?.path[0]).toEqual({ attr: "status", value: "open" });
  });

  it("draws no header without groups", async () => {
    const { container, findAllByRole } = render(<EntityGrid config={config} source={fixtureSource(stations)} />);
    await findAllByRole("gridcell");
    expect(container.querySelectorAll(".jc-grid-group")).toHaveLength(0);
  });
});
