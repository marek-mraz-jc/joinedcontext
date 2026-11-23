/**
 * The toolbar panels' branches the page tests leave open: a nested filter group edited in place,
 * a between condition's upper value, colouring rows by an enum, removing a colour rule, the "All"
 * group chip, the group panel's pick and remove, adding a group through the toolbar, and the
 * access panel of a page with no endpoint.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { queryFromGroup } from "@joinedcontext/sdk";
import type { EntitySource, FilterColumn, FilterGroup } from "@joinedcontext/sdk";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { EMPTY_GROUP, FilterPanel, readGroup } from "../src/components/entities/FilterPanel";
import { GroupPanel, orderedRead } from "../src/components/entities/GridToolbar";
import { GroupCounts, ViewOptions } from "../src/components/entities/ViewOptions";
import type { ViewExtras } from "../src/components/entities/ViewOptions";
import { AccessPanel, useAccess } from "../src/components/entities/AccessPanel";
import { DataToolbar } from "../src/components/entities/DataToolbar";
import type { DataToolbarProps } from "../src/components/entities/DataToolbar";

const L = en.spaces.toolbar;
const S = en.spaces.saved;
const COLUMNS: FilterColumn[] = [
  { key: "name", attr: "name", kind: "text" },
  { key: "seats", attr: "seats", kind: "number" },
];

function wrap(node: React.ReactNode) {
  return render(
    <I18nextProvider i18n={i18n}>
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>{node}</QueryClientProvider>
    </I18nextProvider>,
  );
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

describe("readGroup", () => {
  it("keeps the text values of a saved list and drops what is not text", () => {
    const group = readGroup({ join: "and", conditions: [{ key: "name", filter: { op: "anyOf", value: "", values: ["a", 3, "b"] } }] });
    expect(group).toEqual({ join: "and", conditions: [{ key: "name", filter: { op: "anyOf", value: "", values: ["a", "b"] } }] });
  });
});

function Panel({ seen }: { seen: (next: FilterGroup) => void }) {
  const [value, setValue] = useState<FilterGroup>(EMPTY_GROUP);
  return (
    <FilterPanel
      columns={COLUMNS}
      titleOf={(column) => column.attr ?? column.key}
      value={value}
      onChange={(next) => {
        setValue(next);
        seen(next);
      }}
    />
  );
}

describe("FilterPanel", () => {
  it("edits a condition inside a group, adds one to the group and asks the range's two ends", async () => {
    const user = userEvent.setup();
    const seen = vi.fn();
    wrap(<Panel seen={seen} />);
    await user.click(screen.getByRole("button", { name: L.addGroup }));
    // The group's own condition: text typed in it reaches the whole tree.
    await user.type(screen.getByRole("textbox", { name: L.value }), "jazz");
    expect(queryFromGroup(COLUMNS, seen.mock.lastCall![0] as FilterGroup)).toBe('name~="jazz"');

    // The group's own "Add condition" is the first of the two buttons, the panel's the last.
    await user.click(screen.getAllByRole("button", { name: L.addCondition })[0]!);
    expect(screen.getAllByRole("combobox", { name: L.field })).toHaveLength(2);
    await user.selectOptions(screen.getAllByRole("combobox", { name: L.field })[1]!, "seats");
    await user.selectOptions(screen.getAllByRole("combobox", { name: L.operator })[1]!, "between");
    await user.type(screen.getAllByRole("spinbutton", { name: L.value })[0]!, "2");
    await user.type(screen.getByRole("spinbutton", { name: L.upperValue }), "9");
    expect(queryFromGroup(COLUMNS, seen.mock.lastCall![0] as FilterGroup)).toBe('name~="jazz"|(seats>=2;seats<=9)');
  });
});

const STATUS = [
  { value: "working", title: "Working" },
  { value: "outOfService", title: "Out of service" },
];

function Options({ onChange }: { onChange: (next: ViewExtras) => void }) {
  const [value, setValue] = useState<ViewExtras>({});
  return (
    <ViewOptions
      part="colours"
      attributes={["status"]}
      enums={{ status: STATUS }}
      value={value}
      onChange={(next) => {
        setValue(next);
        onChange(next);
      }}
    />
  );
}

describe("ViewOptions colours", () => {
  it("colours by an enum, one rule per value, and removes a rule", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    wrap(<Options onChange={onChange} />);
    await user.selectOptions(screen.getByLabelText(S.colourBy), "status");
    expect(onChange).toHaveBeenLastCalledWith({
      colour: [
        { when: 'status=="working"', colour: "info" },
        { when: 'status=="outOfService"', colour: "success" },
      ],
    });
    await user.click(screen.getByRole("button", { name: S.removeRule.replace("{n}", "1") }));
    expect(onChange).toHaveBeenLastCalledWith({ colour: [{ when: 'status=="outOfService"', colour: "success" }] });
    expect(screen.getAllByLabelText(S.when)).toHaveLength(1);
  });
});

describe("GroupCounts", () => {
  it("clears the chosen group with the All chip", async () => {
    const user = userEvent.setup();
    const onChoose = vi.fn();
    const source: EntitySource = { query: async () => ({ rows: [], total: 4 }) } as unknown as EntitySource;
    wrap(<GroupCounts scope="p/s" source={source} type="Station" attr="status" options={STATUS} chosen="working" onChoose={onChoose} />);
    expect(await screen.findByRole("button", { name: "Working (4)" })).toHaveAttribute("aria-pressed", "true");
    await user.click(screen.getByRole("button", { name: S.allGroups }));
    expect(onChoose).toHaveBeenCalledWith(null);
  });
});

describe("orderedRead", () => {
  it("reads no row for an aggregation that is not an ordered one", () => {
    expect(orderedRead("count", 5)).toBeNull();
    expect(orderedRead("min", 0)).toBeNull();
  });
});

const FIELDS = [
  { value: "name", label: "Name" },
  { value: "seats", label: "Seats" },
  { value: "city", label: "City" },
];

describe("GroupPanel", () => {
  it("changes the field of one level and removes another", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    wrap(<GroupPanel fields={FIELDS} value={["name", "seats"]} onChange={onChange} />);
    await user.selectOptions(screen.getByRole("combobox", { name: "Group rows by, field 1" }), "city");
    expect(onChange).toHaveBeenLastCalledWith(["city", "seats"]);
    await user.click(screen.getByRole("button", { name: "Remove Name" }));
    expect(onChange).toHaveBeenLastCalledWith(["seats"]);
  });
});

describe("DataToolbar group tool", () => {
  it("adds the first free field as a group level", async () => {
    const user = userEvent.setup();
    const onGroupBy = vi.fn();
    const props: DataToolbarProps = {
      filterColumns: COLUMNS,
      filterTitle: (column) => column.key,
      filters: EMPTY_GROUP,
      onFilters: vi.fn(),
      attributes: ["name", "seats"],
      enums: {},
      extras: {},
      onExtras: vi.fn(),
      fields: FIELDS,
      sortKeys: [],
      onSortKeys: vi.fn(),
      groupBy: [],
      onGroupBy,
      rowSize: "small",
      onRowSize: vi.fn(),
      searchText: "",
      onSearchText: vi.fn(),
      showQuery: false,
      onShowQuery: vi.fn(),
    };
    wrap(<DataToolbar {...props} />);
    await user.click(screen.getByText(L.group, { selector: "summary" }));
    await user.click(screen.getByRole("button", { name: L.groupAdd }));
    expect(onGroupBy).toHaveBeenCalledWith(["name"]);
  });
});

describe("AccessPanel", () => {
  function Bare() {
    const access = useAccess(undefined);
    return (
      <div data-testid="host">
        <AccessPanel slug={undefined} type="Station" access={access} />
      </div>
    );
  }

  it("draws nothing for a page with no endpoint", () => {
    wrap(<Bare />);
    expect(within(screen.getByTestId("host")).queryByRole("heading")).toBeNull();
    expect(screen.getByTestId("host")).toBeEmptyDOMElement();
  });
});
