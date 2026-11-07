/**
 * The Sort and Group panels and the ordered footer (T-5188): up to three sort keys and three group
 * fields picked from what the person's answer carries, each group header counted by the endpoint,
 * and min, max, median, earliest and latest read off the endpoint's own order of the whole set.
 */
import { describe, expect, it } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import type { EntitySource, GridQuery, RichRow } from "@joinedcontext/sdk";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import {
  aggregationsFor,
  FooterAggregate,
  GroupCount,
  GroupPanel,
  groupValueTerm,
  orderedRead,
  SortPanel,
} from "../src/components/entities/GridToolbar";
import type { Aggregation, SortKey } from "../src/components/entities/GridToolbar";
import { viewerWriteFindings } from "./pageChecks";

const L = en.spaces.toolbar;
const FIELDS = [
  { value: "name", label: "Name" },
  { value: "city", label: "City" },
  { value: "seats", label: "Seats" },
  { value: "startDate", label: "Start" },
];

function wrap(node: React.ReactNode) {
  return render(
    <I18nextProvider i18n={i18n}>
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <main>{node}</main>
      </QueryClientProvider>
    </I18nextProvider>,
  );
}

function Sorts({ seen }: { seen: SortKey[][] }) {
  const [value, setValue] = useState<SortKey[]>([]);
  return (
    <SortPanel
      fields={FIELDS}
      value={value}
      onChange={(next) => {
        seen.push(next);
        setValue(next);
      }}
    />
  );
}

function Groups({ seen }: { seen: string[][] }) {
  const [value, setValue] = useState<string[]>([]);
  return (
    <GroupPanel
      fields={FIELDS.slice(0, 3)}
      value={value}
      onChange={(next) => {
        seen.push(next);
        setValue(next);
      }}
    />
  );
}

describe("SortPanel", () => {
  it("says what to do when empty, adds up to three keys, each field once, and turns a direction round", async () => {
    const user = userEvent.setup();
    const seen: SortKey[][] = [];
    wrap(<Sorts seen={seen} />);
    expect(screen.getByText(L.sortEmpty)).toBeInTheDocument();
    expect(screen.getByText(L.sortHint)).toBeInTheDocument();
    // A key only orders the rows in hand: the walker does not read it as a viewer's write.
    expect(viewerWriteFindings(false)).toEqual([]);
    const add = screen.getByRole("button", { name: L.sortAdd });
    await user.click(add);
    await user.click(add);
    await user.click(add);
    expect(seen.at(-1)).toEqual([
      { attr: "name", dir: "asc" },
      { attr: "city", dir: "asc" },
      { attr: "seats", dir: "asc" },
    ]);
    expect(screen.getByRole("button", { name: L.sortAdd })).toHaveAttribute("aria-disabled", "true");
    // A key offers only the fields no other key holds.
    const second = screen.getByRole("combobox", { name: "Sort rows by, field 2" }) as HTMLSelectElement;
    expect([...second.options].map((option) => option.value)).toEqual(["city", "startDate"]);
    await user.selectOptions(second, "startDate");
    await user.selectOptions(screen.getByRole("combobox", { name: "Direction of sort key 2" }), "desc");
    expect(seen.at(-1)?.[1]).toEqual({ attr: "startDate", dir: "desc" });
    await user.click(screen.getByRole("button", { name: "Remove Name" }));
    expect(seen.at(-1)?.map((key) => key.attr)).toEqual(["startDate", "seats"]);
  });
});

describe("GroupPanel", () => {
  it("adds fields outermost first and stops when no field is left", async () => {
    const user = userEvent.setup();
    const seen: string[][] = [];
    wrap(<Groups seen={seen} />);
    expect(screen.getByText(L.groupEmpty)).toBeInTheDocument();
    // A group field only arranges the rows in hand, as a sort key does.
    expect(viewerWriteFindings(false)).toEqual([]);
    for (let n = 0; n < 3; n += 1) await user.click(screen.getByRole("button", { name: L.groupAdd }));
    expect(seen.at(-1)).toEqual(["name", "city", "seats"]);
    expect(screen.getByRole("button", { name: L.groupAdd })).toHaveAttribute("aria-disabled", "true");
  });
});

describe("group counts", () => {
  it("states a group's value as a q term, an absent one as the attribute missing", () => {
    expect(groupValueTerm("status", "open")).toBe('status=="open"');
    expect(groupValueTerm("seats", 4)).toBe("seats==4");
    expect(groupValueTerm("free", false)).toBe("free==false");
    expect(groupValueTerm("status", undefined)).toBe("!status");
    expect(groupValueTerm("address", { street: "x" })).toBeNull();
  });

  it("counts a header at the endpoint over the view's query and every enclosing group", async () => {
    const asked: (string | undefined)[] = [];
    const source = {
      async query(q: GridQuery) {
        asked.push(q.q);
        return { rows: [], total: 7 };
      },
    } as unknown as EntitySource;
    wrap(<GroupCount scope="helsinki/events" source={source} type="Event" q="seats>2" path={[{ attr: "status", value: "open" }, { attr: "city", value: undefined }]} />);
    expect(await screen.findByText("7 rows")).toBeInTheDocument();
    expect(asked).toEqual(['seats>2;status=="open";!city']);
  });
});

function orderedSource(filled: number, values: number[]): { source: EntitySource; asked: { q?: string; orderBy?: string; offset: number; limit: number }[] } {
  const asked: { q?: string; orderBy?: string; offset: number; limit: number }[] = [];
  const source = {
    orders: true,
    async query(q: GridQuery, page: { offset: number; limit: number }) {
      asked.push({ q: q.q, orderBy: q.orderBy, offset: page.offset, limit: page.limit });
      if (!q.orderBy) return { rows: [], total: filled };
      const sorted = [...values].sort((a, b) => (q.orderBy?.endsWith(";desc") ? b - a : a - b));
      const rows: RichRow[] = sorted.slice(page.offset, page.offset + page.limit).map((value, at) => ({
        id: `urn:ngsi-ld:Event:${at}`,
        type: "Event",
        raw: {},
        cells: { seats: { kind: "property", value } },
      }));
      return { rows, total: filled };
    },
  } as unknown as EntitySource;
  return { source, asked };
}

function Footer({ source, start }: { source: EntitySource; start: Aggregation }) {
  const [value, setValue] = useState<Aggregation>(start);
  return <FooterAggregate scope="helsinki/events" source={source} type="Event" attr="seats" label="Seats" kind="number" q={'category=="music"'} value={value} onChange={setValue} />;
}

describe("ordered footer", () => {
  it("offers min, max and median for a number and earliest and latest for a date, only where the source orders", () => {
    expect(aggregationsFor("number", true)).toEqual(expect.arrayContaining(["min", "max", "median"]));
    expect(aggregationsFor("number", true)).not.toContain("earliest");
    expect(aggregationsFor("date", true)).toEqual(expect.arrayContaining(["earliest", "latest"]));
    expect(aggregationsFor("number", false)).not.toContain("min");
    expect(aggregationsFor("text", true)).not.toContain("min");
  });

  it("reads the first row of the order, or the middle ones for a median", () => {
    expect(orderedRead("min", 5)).toEqual({ desc: false, offset: 0, limit: 1 });
    expect(orderedRead("latest", 5)).toEqual({ desc: true, offset: 0, limit: 1 });
    expect(orderedRead("median", 5)).toEqual({ desc: false, offset: 2, limit: 1 });
    expect(orderedRead("median", 4)).toEqual({ desc: false, offset: 1, limit: 2 });
    expect(orderedRead("max", 0)).toBeNull();
  });

  it("asks the endpoint for the order among the rows that hold the attribute", async () => {
    const user = userEvent.setup();
    const { source, asked } = orderedSource(4, [9, 2, 7, 4]);
    wrap(<Footer source={source} start="max" />);
    expect(await screen.findByText("9")).toBeInTheDocument();
    expect(asked.at(-1)).toEqual({ q: 'category=="music";seats', orderBy: "seats;desc", offset: 0, limit: 1 });
    await user.selectOptions(screen.getByRole("combobox", { name: "Footer of Seats" }), "median");
    // 2 4 7 9: the mean of the middle two.
    expect(await screen.findByText("5.5")).toBeInTheDocument();
    expect(asked.at(-1)).toEqual({ q: 'category=="music";seats', orderBy: "seats", offset: 1, limit: 2 });
    await user.selectOptions(screen.getByRole("combobox", { name: "Footer of Seats" }), "min");
    expect(await screen.findByText("2")).toBeInTheDocument();
  });

  it("says nothing was counted where no row holds the attribute", async () => {
    const { source, asked } = orderedSource(0, []);
    wrap(<Footer source={source} start="min" />);
    await waitFor(() => expect(asked.length).toBe(1));
    expect(asked.every((each) => each.orderBy === undefined)).toBe(true);
  });
});
