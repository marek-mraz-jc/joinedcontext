/**
 * T-6303: the grid's one toolbar, shared by the space's data tab and an Endpoint's Data section.
 * The row draws what the host holds and reports each change; both pages import this one component.
 */
import { readFileSync } from "node:fs";
import { act, render, renderHook, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { FilterColumn } from "@joinedcontext/sdk";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { DataToolbar, sortKeysOf, useDataToolbarState } from "../src/components/entities/DataToolbar";
import { schemaState } from "../src/components/entities/EndpointDataView";
import { ApiError } from "../src/api/client";
import type { DataToolbarProps } from "../src/components/entities/DataToolbar";
import { expectNoAxeViolations, renderPart } from "./page_contract";

const COLUMNS: FilterColumn[] = [{ key: "startDate", attr: "startDate", kind: "date" }];

function props(own: Partial<DataToolbarProps> = {}): DataToolbarProps {
  return {
    filterColumns: COLUMNS,
    filterTitle: (column) => (column.attr === "startDate" ? "Start" : column.key),
    filters: { join: "and", conditions: [{ key: "startDate", filter: { op: "gte", value: "2026-11-01" } }] },
    onFilters: vi.fn(),
    attributes: ["name", "startDate"],
    enums: {},
    extras: { hidden: ["startDate"] },
    onExtras: vi.fn(),
    fields: [
      { value: "name", label: "Name" },
      { value: "startDate", label: "Start" },
    ],
    sortKeys: [],
    onSortKeys: vi.fn(),
    groupBy: [],
    onGroupBy: vi.fn(),
    rowSize: "small",
    onRowSize: vi.fn(),
    searchText: "",
    onSearchText: vi.fn(),
    showQuery: false,
    onShowQuery: vi.fn(),
    ...own,
  };
}

describe("the grid's one toolbar (T-6303)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("draws the space's tools, each with the count it holds", () => {
    renderPart(<DataToolbar {...props()} />);
    const t = en.spaces.toolbar;
    for (const label of [t.filter, t.hide, t.sort, t.group, t.colour, t.rowSize]) {
      expect(screen.getByText(label, { selector: "summary" })).toBeInTheDocument();
    }
    expect(screen.getByText(t.filter, { selector: "summary" })).toHaveTextContent(`${t.filter}1`);
    expect(screen.getByText(t.hide, { selector: "summary" })).toHaveTextContent(`${t.hide}1`);
    expect(screen.getByRole("searchbox", { name: t.search })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: t.query })).toHaveAttribute("aria-pressed", "false");
  });

  it("names each field as its column does and reports a field shown again", async () => {
    const user = userEvent.setup();
    const onExtras = vi.fn();
    renderPart(<DataToolbar {...props({ onExtras })} />);
    await user.click(screen.getByText(en.spaces.toolbar.hide, { selector: "summary" }));
    const start = screen.getByRole("checkbox", { name: "Start" });
    expect(start).not.toBeChecked();
    expect(screen.getByRole("checkbox", { name: "Name" })).toBeChecked();
    await user.click(start);
    expect(onExtras).toHaveBeenLastCalledWith({ hidden: [] });
  });

  it("reports the search and the query switch", async () => {
    const user = userEvent.setup();
    const onSearchText = vi.fn();
    const onShowQuery = vi.fn();
    renderPart(<DataToolbar {...props({ onSearchText, onShowQuery })} />);
    await user.type(screen.getByRole("searchbox", { name: en.spaces.toolbar.search }), "k");
    expect(onSearchText).toHaveBeenLastCalledWith("k");
    await user.click(screen.getByRole("button", { name: en.spaces.toolbar.query }));
    expect(onShowQuery).toHaveBeenLastCalledWith(true);
  });

  it("shows the filter's condition in the panel, its field titled", async () => {
    const user = userEvent.setup();
    renderPart(<DataToolbar {...props()} />);
    await user.click(screen.getByText(en.spaces.toolbar.filter, { selector: "summary" }));
    const field = screen.getByRole("combobox", { name: en.spaces.toolbar.field });
    expect(field).toHaveValue("startDate");
    expect(within(field).getByRole("option", { name: "Start" })).toBeInTheDocument();
    expect(screen.getByRole("combobox", { name: en.spaces.toolbar.operator })).toHaveValue("gte");
  });

  describe("on a phone", () => {
    const stubbed = window.matchMedia;
    beforeEach(() => {
      window.matchMedia = ((query: string) => ({ matches: query.startsWith("(max-width"), addEventListener() {}, removeEventListener() {} })) as unknown as typeof window.matchMedia;
    });
    afterEach(() => {
      window.matchMedia = stubbed;
    });

    it("folds every tool into one menu that counts what is set, and keeps the search outside it", async () => {
      const user = userEvent.setup();
      const onShowQuery = vi.fn();
      const { container } = renderPart(<DataToolbar {...props({ onShowQuery })} />);
      const tools = screen.getByText(en.spaces.toolbar.tools, { selector: "summary" });
      expect(tools).toHaveTextContent(`${en.spaces.toolbar.tools}2`);
      expect(screen.getByRole("searchbox", { name: en.spaces.toolbar.search })).toBeVisible();
      expect(container.querySelector("details[open]")).toBeNull();
      await user.click(tools);
      await user.click(screen.getByText(en.spaces.toolbar.sort, { selector: "summary" }));
      expect(screen.getByText(en.spaces.toolbar.sort, { selector: "summary" }).closest("details")).toHaveAttribute("open");
      await user.click(screen.getByRole("button", { name: en.spaces.toolbar.query }));
      expect(onShowQuery).toHaveBeenCalledWith(true);
      await expectNoAxeViolations(container);
    });

    it("closes the menu from its own button and gives focus back", async () => {
      const user = userEvent.setup();
      renderPart(<DataToolbar {...props()} />);
      const tools = screen.getByText(en.spaces.toolbar.tools, { selector: "summary" });
      await user.click(tools);
      await user.click(screen.getByRole("button", { name: en.app.close }));
      expect(tools.closest("details")).not.toHaveAttribute("open");
      expect(tools).toHaveFocus();
    });

    it("closes the open tool on Escape and leaves the menu open", async () => {
      const user = userEvent.setup();
      renderPart(<DataToolbar {...props()} />);
      const tools = screen.getByText(en.spaces.toolbar.tools, { selector: "summary" });
      await user.click(tools);
      const hide = screen.getByText(en.spaces.toolbar.hide, { selector: "summary" });
      await user.click(hide);
      await user.keyboard("{Escape}");
      expect(hide.closest("details")).not.toHaveAttribute("open");
      expect(tools.closest("details")).toHaveAttribute("open");
    });
  });

  it("has no axe violation", async () => {
    const { container } = renderPart(<DataToolbar {...props()} />);
    await expectNoAxeViolations(container);
  });

  it("is the one toolbar both pages import", () => {
    for (const file of ["src/pages/spaces/SpaceInside.tsx", "src/components/entities/EndpointDataView.tsx"]) {
      const code = readFileSync(file, "utf8");
      expect(code).toMatch(/import \{ DataToolbar\b[^}]*\} from "[./]+\/(components\/entities\/)?DataToolbar";/);
      expect(code).not.toMatch(/<FilterPanel\b|<SortPanel\b|<GroupPanel\b/);
    }
  });
});

describe("the toolbar's own state (T-6303)", () => {
  const CONDITION = { key: "startDate", filter: { op: "gte" as const, value: "2026-11-01" } };

  it("asks the filters and the search, the search 400 ms after the last key", () => {
    vi.useFakeTimers();
    try {
      const { result } = renderHook(() => useDataToolbarState());
      expect(result.current.query).toBeUndefined();
      act(() => result.current.onFilterColumns([{ key: "name", attr: "name", kind: "text" }, ...COLUMNS]));
      act(() => result.current.props.onFilters({ join: "and", conditions: [CONDITION] }));
      const filtered = result.current.q;
      expect(filtered).toContain("startDate");
      act(() => result.current.props.onSearchText("park"));
      expect(result.current.q).toBe(filtered);
      act(() => vi.advanceTimersByTime(400));
      expect(result.current.q).toContain("park");
      expect(result.current.query).toEqual({ q: result.current.q });
    } finally {
      vi.useRealTimers();
    }
  });

  it("opens a default filter once, then leaves the panel to the reader", () => {
    const preset = { join: "and" as const, conditions: [CONDITION] };
    const { result, rerender } = renderHook(({ adopt }) => {
      const tools = useDataToolbarState();
      tools.adoptDefault(adopt);
      return tools;
    }, { initialProps: { adopt: preset } });
    expect(result.current.group).toEqual(preset);
    act(() => result.current.props.onFilters({ join: "and", conditions: [] }));
    rerender({ adopt: preset });
    expect(result.current.group.conditions).toEqual([]);
  });

  it("hides beside what is hidden now, and a reset empties every tool", () => {
    const { result } = renderHook(() => useDataToolbarState());
    act(() => result.current.hide(["name"], "startDate"));
    act(() => result.current.props.onGroupBy(["name"]));
    act(() => result.current.setThenBy([{ attr: "name", dir: "asc" }]));
    expect(result.current.extras.hidden).toEqual(["name", "startDate"]);
    act(() => result.current.reset());
    expect(result.current.extras).toEqual({});
    expect(result.current.groupBy).toEqual([]);
    expect(result.current.thenBy).toEqual([]);
  });

  it("tones the rows a colour rule matches and no other", () => {
    const { result } = renderHook(() => useDataToolbarState());
    act(() => result.current.props.onExtras({ colour: [{ when: "pm10>40", colour: "danger" }, { when: " ", colour: "ok" }] }));
    const row = (pm10: number) => ({ id: "urn:ngsi-ld:A:1", type: "A", cells: { pm10: { kind: "property", value: pm10 } }, raw: {} }) as never;
    expect(result.current.rowTone(row(50))?.tone).toBe("danger");
    expect(result.current.rowTone(row(10))).toBeUndefined();
  });

  it("puts the header's sort first in the Sort panel, once", () => {
    const thenBy = [{ attr: "name", dir: "asc" as const }, { attr: "startDate", dir: "desc" as const }];
    expect(sortKeysOf(null, thenBy)).toBe(thenBy);
    expect(sortKeysOf({ attr: "startDate", dir: "asc" }, thenBy)).toEqual([{ attr: "startDate", dir: "asc" }, { attr: "name", dir: "asc" }]);
  });
});

describe("the Data section's wait (T-6303)", () => {
  const done = { error: null, isPending: false };
  const t = (key: string) => key;

  it("names a failed schema read, waits for every read, and says when no type is left", () => {
    render(<>{schemaState({ index: { error: new ApiError(403, "no policy grants this read"), isPending: false }, schema: done, widget: done }, false, t)}</>);
    expect(screen.getByText("no policy grants this read")).toBeInTheDocument();
    expect(schemaState({ index: done, schema: done, widget: { error: null, isPending: true } }, true, t)?.props.role).toBe("status");
    expect(schemaState({ index: done, schema: done, widget: done }, false, t)?.props.children).toBe("endpoints.data.noSchema");
  });

  it("opens the grid when the widget defaults fail: they are optional", () => {
    expect(schemaState({ index: done, schema: done, widget: { error: new Error("404"), isPending: false } }, true, t)).toBeNull();
  });
});
