// covers: src/pages/spaces/SpaceInside.tsx
/**
 * T-7678: what the space page does with each answer of the parts it hands its data to: the grid,
 * the toolbar, the dialogs and the live feed. Those parts have tests of their own; here each one
 * is a stand-in that keeps the props the page gave it, so a test calls what the part would call
 * (a hidden column, a sort, a live change, an import) and reads what the page does next: what the
 * grid is handed, what the address says, what the server is asked.
 */
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { RouterProvider, createRootRoute, createRouter } from "@tanstack/react-router";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { SourceError, toRichRow } from "@joinedcontext/sdk";
import type { FilterColumn, RichRow } from "@joinedcontext/sdk";
import type { ReactElement, ReactNode } from "react";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { classNamesOf, readable, Samples, SpaceInside } from "../src/pages/spaces/SpaceInside";

/** What a stand-in keeps of its props: read and called as loosely as the part itself would. */
type Props = { readonly [key: string]: Props } & ((...args: unknown[]) => Props);

const seen = vi.hoisted(() => ({
  grid: null as Props | null,
  toolbar: null as Props | null,
  viewBar: null as Props | null,
  importDialog: null as Props | null,
  form: null as Props | null,
  addField: null as Props | null,
  trash: null as Props | null,
  live: null as ((change: unknown) => void) | null,
}));

vi.mock("../src/components/entities/PortalEntityGrid", async (actual) => ({
  ...(await actual<object>()),
  PortalEntityGrid: (props: Props) => {
    seen.grid = props;
    return <div data-testid="grid">{props.empty as unknown as ReactNode}</div>;
  },
}));
vi.mock("../src/components/entities/DataToolbar", () => ({
  DataToolbar: (props: Props) => {
    seen.toolbar = props;
    return null;
  },
}));
vi.mock("../src/components/entities/ViewBar", () => ({
  ViewBar: (props: Props) => {
    seen.viewBar = props;
    return null;
  },
}));
vi.mock("../src/pages/spaces/ImportRows", async (actual) => ({
  ...(await actual<object>()),
  ImportRowsDialog: (props: Props) => {
    seen.importDialog = props;
    return props.open ? <p>import open</p> : null;
  },
}));
vi.mock("../src/pages/spaces/FormView", () => ({
  FormView: (props: Props) => {
    seen.form = props;
    return null;
  },
}));
vi.mock("../src/components/entities/AddFieldDialog", () => ({
  AddFieldDialog: (props: Props) => {
    seen.addField = props;
    return props.open ? <p>add field open</p> : null;
  },
}));
vi.mock("../src/components/AlertDialog", () => ({
  AlertDialog: (props: Props) => (
    <div role="dialog" aria-label={`alerts ${props.scope} ${props.target}`}>
      <button type="button" onClick={props.onClose}>
        close alerts
      </button>
    </div>
  ),
}));
vi.mock("../src/pages/spaces/DataViews", async (actual) => ({
  ...(await actual<object>()),
  TrashPanel: (props: Props) => {
    seen.trash = props;
    return null;
  },
  useLive: (_project: string, _space: string, _type: string, onChange: (change: unknown) => void) => {
    seen.live = onChange;
  },
}));

const SLUG = "k7m2qz4tv6xh3n5jb2ryd3wcfa";
const TYPE = "AirQualityObserved";
const ID1 = "urn:ngsi-ld:AirQualityObserved:banskabystrica.sk:ovzdusie:1";
const ID2 = "urn:ngsi-ld:AirQualityObserved:banskabystrica.sk:ovzdusie:2";

const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });

const SPACE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ContextSpace",
  metadata: { name: "ovzdusie", namespace: "banskabystrica", title: { en: "Air quality" } },
  spec: { dataModelRef: "bb-air" },
  status: { phase: "Live" },
};

const MODEL = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "DataModel",
  metadata: { name: "bb-air", namespace: "banskabystrica" },
  spec: { classes: [TYPE, "AirQualityStation"], artifacts: { jsonSchema: "./bb-air.schema.json" } },
};

const SOURCE = [
  "id: https://banskabystrica.sk/models/air",
  "name: bb-air",
  "enums:",
  "  AirStatus:",
  "    permissible_values:",
  "      good: {}",
  "      bad: {}",
  "classes:",
  `  ${TYPE}:`,
  "    slots: [name, pm10, status, station]",
  "  AirQualityStation:",
  "    slots: [name, observations]",
  "slots:",
  "  name: { range: string }",
  "  pm10: { range: float }",
  "  status: { range: AirStatus }",
  "  station:",
  "    range: AirQualityStation",
  "    inverse: observations",
  "    annotations: { ngsi_ld_kind: Relationship }",
  "  observations:",
  "    range: AirQualityObserved",
  "    multivalued: true",
  "    inverse: station",
  "    annotations: { ngsi_ld_kind: Relationship }",
  "",
].join("\n");

const ENDPOINT = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Endpoint",
  metadata: { name: "public-air", namespace: "banskabystrica", title: { en: "Open air data" } },
  spec: { contextSpaceRef: "ovzdusie", slug: SLUG, audience: "public", enabledRepresentations: ["ngsi-ld", "csv"] },
  status: { phase: "Live" },
};

const POLICY = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Policy",
  metadata: { name: "public-air-quality", namespace: "banskabystrica", title: { en: "Anyone reads the air" } },
  spec: {
    contextSpaceRef: "ovzdusie",
    assignee: { kind: "role", id: "public" },
    operations: ["queryEntity", "deleteEntity"],
    information: [{ entities: [{ type: TYPE }] }],
  },
};

/** A grant that lets the person write pm10 and delete rows of the type. */
const STEWARD = {
  permissions: [{ resource: { type: TYPE }, actions: ["queryEntity", "updateAttrs", "deleteEntity"], attributes: "*" }],
};
const STEWARD_WRITES = {
  type: TYPE,
  attributes: [
    { name: "name", read: true, write: false, delete: false, reason: "not_in_grant" },
    { name: "pm10", read: true, write: true, delete: true, reason: "grant" },
  ],
};

type Answer = { status: number; body: unknown } | "hang";
type Seed = Partial<Record<"space" | "models" | "source" | "endpoints" | "policies" | "views" | "access" | "attributes" | "probe", Answer>>;

const json = (body: unknown, status = 200) =>
  typeof body === "string"
    ? new Response(body, { status, headers: { "Content-Type": "text/yaml" } })
    : new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });

function show(seed: Seed = {}, tab = "") {
  window.history.replaceState({}, "", tab ? `/?tab=${tab}` : "/");
  const given: Record<string, Answer> = {
    space: { status: 200, body: SPACE },
    models: { status: 200, body: list([MODEL]) },
    source: { status: 200, body: SOURCE },
    endpoints: { status: 200, body: list([ENDPOINT]) },
    policies: { status: 200, body: list([POLICY]) },
    views: { status: 200, body: { items: [] } },
    access: { status: 200, body: { permissions: [] } },
    attributes: { status: 200, body: { type: TYPE, attributes: [] } },
    probe: { status: 200, body: [] },
    ...seed,
  };
  const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = new URL(input instanceof Request ? input.url : String(input), window.location.origin);
    const path = url.pathname;
    const method = input instanceof Request ? input.method : (init?.method ?? "GET");
    const key = path === `/api/endpoint/${SLUG}/access/attributes`
      ? "attributes"
      : path === `/api/endpoint/${SLUG}/access`
        ? "access"
        : path.startsWith("/cs/ovzdusie/ngsi-ld/v1/entities") && method === "GET"
          ? "probe"
          : path.endsWith("/datamodels/bb-air/source")
            ? "source"
            : path.endsWith("/spaces/ovzdusie/views")
              ? "views"
              : path.endsWith("/spaces/ovzdusie")
                ? "space"
                : path.endsWith("/datamodels")
                  ? "models"
                  : path.endsWith("/endpoints")
                    ? "endpoints"
                    : path.endsWith("/policies")
                      ? "policies"
                      : "other";
    const answer = given[key];
    if (answer === "hang") return new Promise<Response>(() => undefined);
    if (answer === undefined) return json(path.startsWith("/cs/") ? {} : list([]), path.startsWith("/cs/") ? 201 : 200);
    return json(answer.body, answer.status);
  });
  vi.stubGlobal("fetch", fetchMock);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const router = createRouter({
    routeTree: createRootRoute({ component: () => <SpaceInside project="banskabystrica" name="ovzdusie" /> }),
  });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <RouterProvider router={router} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return { fetchMock, client };
}

/** The grid's props, once it is drawn. */
async function grid(): Promise<Props> {
  await waitFor(() => expect(seen.grid?.config).toBeTruthy());
  return seen.grid!;
}

const rows = (...entities: Record<string, unknown>[]): RichRow[] => entities.map((each) => toRichRow(each));
const ROW1 = { id: ID1, type: TYPE, name: { type: "Property", value: "Námestie SNP" }, pm10: { type: "Property", value: 12 } };
const ROW2 = { id: ID2, type: TYPE, name: { type: "Property", value: "Radvaň" }, pm10: { type: "Property", value: 9 } };

const askedQ = (fetchMock: ReturnType<typeof show>["fetchMock"]) =>
  fetchMock.mock.calls
    .map(([input]) => new URL(input instanceof Request ? input.url : String(input), window.location.origin))
    .filter((url) => url.pathname.startsWith("/cs/ovzdusie/"));

describe("the space page's data, wired to its parts (T-7678)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.localStorage.clear();
    for (const key of Object.keys(seen) as (keyof typeof seen)[]) seen[key] = null;
  });
  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it("keeps a hidden column and a changed layout in this browser, and hands them back to the grid", async () => {
    show();
    const first = await grid();
    act(() => first.onHide("pm10"));
    await waitFor(() => expect(seen.grid!.hidden).toEqual(["pm10"]));
    act(() => seen.grid!.onHide("pm10"));
    expect(seen.grid!.hidden).toEqual(["pm10"]);
    act(() => seen.grid!.onLayout({ order: ["pm10", "name"], width: { pm10: 120 }, labels: { pm10: "Dust" } }));
    await waitFor(() => expect(seen.grid!.layout).toEqual({ order: ["pm10", "name"], width: { pm10: 120 }, labels: { pm10: "Dust" } }));
    // A partial change keeps what it does not name.
    act(() => seen.grid!.onLayout({ width: { pm10: 200 } }));
    await waitFor(() => expect(seen.grid!.layout).toEqual({ order: ["pm10", "name"], width: { pm10: 200 }, labels: { pm10: "Dust" } }));
    const kept = Object.entries(window.localStorage).find(([key]) => key.includes(TYPE));
    expect(kept?.[1]).toContain("Dust");
  });

  it("puts the opened row's tab and History attribute into the address, and drops them for Fields", async () => {
    show();
    const first = await grid();
    act(() => first.onDetailPlace({ tab: "history", attr: "pm10" }));
    await waitFor(() => expect(new URLSearchParams(window.location.search).get("attr")).toBe("pm10"));
    expect(new URLSearchParams(window.location.search).get("tab")).toBe("history");
    act(() => seen.grid!.onDetailPlace({ tab: "comments", attr: "pm10" }));
    await waitFor(() => expect(new URLSearchParams(window.location.search).get("tab")).toBe("comments"));
    expect(new URLSearchParams(window.location.search).get("attr")).toBeNull();
    act(() => seen.grid!.onDetailPlace({ tab: "fields", attr: "" }));
    await waitFor(() => expect(new URLSearchParams(window.location.search).get("tab")).toBeNull());
  });

  it("puts the page size into the address, and reads the next page size from it", async () => {
    show();
    const first = await grid();
    expect(first.config.pageSize).toBe(100);
    act(() => first.onPageSize(500));
    await waitFor(() => expect(new URLSearchParams(window.location.search).get("rows")).toBe("500"));
    await waitFor(() => expect(seen.grid!.config.pageSize).toBe(500));
  });

  it("steers the order once the Sort panel sets it, the header's key first, and saves it with a view", async () => {
    show();
    await grid();
    await waitFor(() => expect(seen.toolbar).not.toBeNull());
    expect(seen.grid!.view).toBeUndefined();
    act(() => seen.toolbar!.onSortKeys([{ attr: "pm10", dir: "desc" }, { attr: "name", dir: "asc" }]));
    await waitFor(() => expect(seen.grid!.view).toEqual({ filterText: null, sort: { attr: "pm10", dir: "desc" } }));
    expect(seen.grid!.thenBy).toEqual([{ attr: "name", dir: "asc" }]);
    expect(seen.toolbar!.sortKeys).toEqual([{ attr: "pm10", dir: "desc" }, { attr: "name", dir: "asc" }]);
    await waitFor(() => expect(seen.viewBar).not.toBeNull());
    expect(seen.viewBar!.current.sort).toEqual([
      { attr: "pm10", desc: true },
      { attr: "name", desc: false },
    ]);
    // The grid reporting the same order keeps the page's state as it is; a new one replaces it.
    const before = seen.grid!.view;
    act(() => seen.grid!.onGridState({ filterText: null, sort: { attr: "pm10", dir: "desc" } }));
    expect(seen.grid!.view).toBe(before);
    act(() => seen.grid!.onGridState({ filterText: "pm10>3", sort: null }));
    await waitFor(() => expect(seen.grid!.view).toEqual({ filterText: "pm10>3", sort: null }));
    // Emptied, the panel leaves no order.
    act(() => seen.toolbar!.onSortKeys([]));
    await waitFor(() => expect(seen.grid!.view).toEqual({ filterText: "pm10>3", sort: null }));
  });

  it("groups by an enum field, counts each group and asks a footer's aggregate", async () => {
    show();
    await grid();
    await waitFor(() => expect(seen.toolbar).not.toBeNull());
    act(() => seen.toolbar!.onGroupBy(["status"]));
    await waitFor(() => expect(seen.grid!.groupBy).toEqual(["status"]));
    const header = seen.grid!.groupHeader({ path: [{ attr: "status", value: "good" }] }) as unknown as unknown as ReactElement<Props>;
    expect(header.props.type).toBe(TYPE);
    expect(header.props.path).toEqual([{ attr: "status", value: "good" }]);
    await waitFor(() => expect(seen.viewBar!.current.group).toBe("status"));
    expect(seen.viewBar!.current.settings.groupBy).toEqual(["status"]);

    // A footer for a field, none for a meta column.
    expect(seen.grid!.footer({ attr: "pm10", label: "PM10", meta: "createdAt" })).toBeNull();
    const footer = seen.grid!.footer({ attr: "pm10", label: "PM10" }) as unknown as unknown as ReactElement<Props>;
    expect(footer.props.value).toBe("none");
    act(() => footer.props.onChange("max"));
    await waitFor(() => expect(seen.viewBar!.current.settings.aggregations).toEqual({ pm10: "max" }));
  });

  it("titles a filter column with the model's title and the meta field's name", async () => {
    show();
    await grid();
    await waitFor(() => expect(seen.toolbar).not.toBeNull());
    expect(seen.toolbar!.filterTitle({ key: "pm10", attr: "pm10", kind: "number" })).toBe("PM10");
    expect(seen.toolbar!.filterTitle({ key: "createdAt", attr: null, meta: "createdAt", kind: "date" })).toBe(
      `Created at · ${en.entityGrid.createdAt}`,
    );
  });

  it("says a search matched nothing with a way back, and a filter that matched nothing with its own", async () => {
    const { fetchMock } = show();
    await grid();
    const columns: FilterColumn[] = [
      { key: "name", attr: "name", kind: "text" },
      { key: "pm10", attr: "pm10", kind: "number" },
    ];
    act(() => seen.grid!.onFilterColumns(columns));
    // The same columns again keep the page as it is.
    act(() => seen.grid!.onFilterColumns([...columns]));
    act(() => seen.toolbar!.onSearchText("Radvaň"));
    const searched = await screen.findByText(/matched no row|no row/i, {}, { timeout: 2000 });
    expect(searched).toBeInTheDocument();
    expect(seen.grid!.query.q).toContain("name");
    await userEvent.click(within(screen.getByTestId("grid")).getByRole("button", { name: en.spaces.inside.clearSearch }));
    await waitFor(() => expect(seen.grid!.query).toBeUndefined());

    act(() => seen.toolbar!.onFilters({ join: "and", conditions: [{ key: "pm10", filter: { op: "gt", value: "40" } }] }));
    await waitFor(() => expect(seen.grid!.query).toEqual({ q: "pm10>40" }));
    await userEvent.click(within(screen.getByTestId("grid")).getByRole("button", { name: en.spaces.inside.clearFilters }));
    await waitFor(() => expect(seen.grid!.query).toBeUndefined());
    expect(askedQ(fetchMock).length).toBeGreaterThan(0);
  });

  it("names the relations the rows carry, and marks the rows a live change touched for a few seconds", async () => {
    show();
    const first = await grid();
    expect(first.relations).toEqual({});
    act(() =>
      first.onRows(
        rows(
          { ...ROW1, station: { type: "Relationship", object: "urn:ngsi-ld:AirQualityStation:1" } },
          ROW2,
        ),
      ),
    );
    await waitFor(() => expect(Object.keys(seen.grid!.relations)).toEqual(["station"]));
    expect(seen.grid!.rowTone(rows(ROW1)[0])).toBeUndefined();

    vi.useFakeTimers({ shouldAdvanceTime: true });
    const key = seen.grid!.reloadKey;
    act(() => seen.live!({ type: TYPE, at: new Date("2026-11-23T10:00:00Z") }));
    await waitFor(() => expect(seen.grid!.reloadKey).toBe(Number(key) + 1));
    // The read after the change brings pm10 13: that row flashes, the other does not.
    act(() => seen.grid!.onRows(rows({ ...ROW1, pm10: { type: "Property", value: 13 } }, ROW2)));
    await waitFor(() => expect(seen.grid!.rowTone(rows(ROW1)[0])).toEqual({ tone: "info", label: en.spaces.toolbar.changedLive }));
    expect(seen.grid!.rowTone(rows(ROW2)[0])).toBeUndefined();
    act(() => vi.advanceTimersByTime(4000));
    await waitFor(() => expect(seen.grid!.rowTone(rows(ROW1)[0])).toBeUndefined());
  });

  it("reads again on a live change before any row arrived, with nothing to compare", async () => {
    show();
    const first = await grid();
    act(() => seen.live!({ type: TYPE, at: new Date("2026-11-23T10:00:00Z") }));
    await waitFor(() => expect(seen.grid!.reloadKey).toBe(Number(first.reloadKey) + 1));
    act(() => seen.grid!.onRows(rows(ROW1)));
    expect(seen.grid!.rowTone(rows(ROW1)[0])).toBeUndefined();
  });

  it("writes a correction through the granting endpoint, and refuses one no endpoint grants", async () => {
    const { fetchMock } = show();
    const first = await grid();
    // Nobody has turned on Edit: no grant is asked, so no endpoint writes.
    await expect(first.source.patch(ID1, { pm10: { type: "Property", value: 14 } }, "sensor fixed")).rejects.toEqual(
      new SourceError(403, en.spaces.views.readOnly),
    );
    expect(fetchMock.mock.calls.some(([, init]) => init?.method === "PATCH")).toBe(false);
  });

  it("selects rows, deletes them with the grant's right and sets a field through the writer", async () => {
    const { fetchMock } = show({ access: { status: 200, body: STEWARD }, attributes: { status: 200, body: STEWARD_WRITES } });
    const first = await grid();
    act(() => first.onRows(rows(ROW1, ROW2)));
    act(() => seen.grid!.onSelectedChange([ID1]));
    const bar = await screen.findByRole("region", { name: en.spaces.toolbar.selection });
    await waitFor(() => expect(within(bar).getByRole("button", { name: en.spaces.toolbar.setField })).toBeEnabled());
    expect(within(bar).getByRole("button", { name: en.spaces.toolbar.deleteSelected })).toBeEnabled();
    // The opened row offers the values the person may write, once the grant is in.
    await waitFor(() => expect((seen.grid!.detailTabs(rows(ROW1)[0]) as unknown as { id: string }[]).map((each) => each.id)).toEqual(["values", "comments"]));
    // The writer's patch goes to the endpoint that grants it.
    await seen.grid!.source.patch(ID1, { pm10: { type: "Property", value: 14 } }, "sensor fixed");
    const patched = fetchMock.mock.calls.find(([, init]) => init?.method === "PATCH");
    expect(String(patched?.[0])).toContain(`/api/endpoint/${SLUG}/ngsi-ld/v1/entities/`);
    // Set a field: the dialog's done reads the rows again.
    const key = seen.grid!.reloadKey;
    await userEvent.click(within(bar).getByRole("button", { name: en.spaces.toolbar.setField }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.selectOptions(within(dialog).getByLabelText(en.spaces.toolbar.setFieldLabel), "pm10");
    await userEvent.type(within(dialog).getByLabelText(en.spaces.toolbar.setValue), "20");
    await userEvent.type(within(dialog).getByLabelText(en.spaces.toolbar.setReason), "recalibrated");
    await userEvent.click(within(dialog).getByRole("button", { name: /Set/ }));
    await waitFor(() => expect(seen.grid!.reloadKey).toBe(Number(key) + 1));
    await userEvent.keyboard("{Escape}");
    // Clear drops the selection and the bar with it.
    await userEvent.click(within(bar).getByRole("button", { name: en.spaces.toolbar.clearSelection }));
    await waitFor(() => expect(screen.queryByRole("region", { name: en.spaces.toolbar.selection })).toBeNull());
    expect(seen.grid!.selected).toEqual([]);
  });

  it("keeps the rows a delete refused checked, and reads the type again", async () => {
    const { fetchMock } = show({ access: { status: 200, body: STEWARD }, attributes: { status: 200, body: STEWARD_WRITES } });
    const first = await grid();
    act(() => first.onRows(rows(ROW1, ROW2)));
    act(() => seen.grid!.onSelectedChange([ID1, ID2]));
    const bar = await screen.findByRole("region", { name: en.spaces.toolbar.selection });
    await waitFor(() => expect(within(bar).getByRole("button", { name: en.spaces.toolbar.deleteSelected })).toBeEnabled());
    fetchMock.mockImplementation(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = new URL(input instanceof Request ? input.url : String(input), window.location.origin);
      const method = input instanceof Request ? input.method : (init?.method ?? "GET");
      if (method === "DELETE" && url.pathname.includes(encodeURIComponent(ID2))) {
        return json({ title: "Forbidden", detail: "the station keeps its history" }, 403);
      }
      if (method === "DELETE") return new Response(null, { status: 204 });
      if (url.pathname.includes("trash")) return json({ id: 9, urn: ID1, type: TYPE, entity: {}, deletedAt: "", expiresAt: "" }, 201);
      return json(list([]));
    });
    const gridKey = String(seen.grid!.reloadKey);
    await userEvent.click(within(bar).getByRole("button", { name: en.spaces.toolbar.deleteSelected }));
    const confirm = await screen.findByRole("alertdialog");
    await userEvent.type(within(confirm).getByTestId("spaces-delete-reason"), "duplicates");
    await userEvent.click(within(confirm).getByTestId("confirm-accept"));
    await waitFor(() => expect(seen.grid!.selected).toEqual([ID2]));
    expect(gridKey).toBeDefined();
  });

  it("opens the type's form, import and alerts from the toolbar, and reads the rows again after each write", async () => {
    const { client } = show();
    await grid();
    await userEvent.click(screen.getByText(en.spaces.toolbar.more));
    await userEvent.click(await screen.findByRole("button", { name: en.spaces.import.open }));
    expect(await screen.findByText("import open")).toBeInTheDocument();
    const invalidate = vi.spyOn(client, "invalidateQueries");
    const before = seen.grid!.key;
    act(() => seen.importDialog!.onImported());
    expect(invalidate).toHaveBeenCalledWith({ queryKey: ["space-view-rows", "ovzdusie"] });
    await userEvent.click(screen.getByRole("button", { name: en.spaces.inside.addRow }));
    await waitFor(() => expect(seen.form).not.toBeNull());
    act(() => seen.form!.onCreated());
    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(screen.getByTestId("grid")).toBeInTheDocument());
    expect(before).toBeUndefined();

    await userEvent.click(screen.getByText(en.spaces.toolbar.more));
    await userEvent.click(await screen.findByRole("button", { name: en.alerts.forType.replace("{type}", TYPE) }));
    const alerts = await screen.findByRole("dialog", { name: `alerts type ovzdusie/${TYPE}` });
    await userEvent.click(within(alerts).getByRole("button", { name: "close alerts" }));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: `alerts type ovzdusie/${TYPE}` })).toBeNull());
  });

  it("lists each field the dialog proposed as a Change, and starts a new list when it opens again", async () => {
    show();
    await grid();
    await userEvent.click(screen.getByText(en.spaces.toolbar.more));
    await userEvent.click(await screen.findByRole("button", { name: en.spaces.fields.add }));
    expect(await screen.findByText("add field open")).toBeInTheDocument();
    const change = (name: string) => ({
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Change",
      metadata: { name, namespace: "banskabystrica" },
      spec: {},
      status: { phase: "Proposed" },
    });
    act(() => seen.addField!.onProposed(change("chg-0000001a")));
    act(() => seen.addField!.onProposed(change("chg-0000001b")));
    expect(await screen.findByText(/chg-0000001a/)).toBeInTheDocument();
    expect(screen.getByText(/chg-0000001b/)).toBeInTheDocument();
    act(() => seen.addField!.onOpenChange(false));
    await waitFor(() => expect(screen.queryByText("add field open")).toBeNull());
    expect(screen.getByText(/chg-0000001a/)).toBeInTheDocument();
    await userEvent.click(screen.getByText(en.spaces.toolbar.more));
    await userEvent.click(await screen.findByRole("button", { name: en.spaces.fields.add }));
    await waitFor(() => expect(screen.queryByText(/chg-0000001a/)).toBeNull());
  });

  it("restores a row from the trash through the space surface, and names the refusal", async () => {
    const { fetchMock } = show();
    await grid();
    await waitFor(() => expect(seen.trash).not.toBeNull());
    const entity = { id: ID1, type: TYPE };
    await seen.trash!.restore(entity);
    const posted = fetchMock.mock.calls.find(([, init]) => init?.method === "POST");
    expect(String(posted?.[0])).toBe("/cs/ovzdusie/ngsi-ld/v1/entities");
    fetchMock.mockImplementation(async () => json({ title: "Conflict", detail: "the row exists again" }, 409));
    await expect(seen.trash!.restore(entity)).rejects.toThrow("the row exists again");
    fetchMock.mockImplementation(async () => json({ title: "Forbidden" }, 403));
    await expect(seen.trash!.restore(entity)).rejects.toThrow("Forbidden");
    fetchMock.mockImplementation(async () => new Response(null, { status: 500 }));
    await expect(seen.trash!.restore(entity)).rejects.toThrow("HTTP 500");
  });

  it("asks the space surface again from the failure's Retry", async () => {
    const { fetchMock } = show({ probe: { status: 503, body: { title: "Service Unavailable" } } });
    const retry = await screen.findByRole("button", { name: en.app.error.retry });
    const asked = askedQ(fetchMock).length;
    await userEvent.click(retry);
    await waitFor(() => expect(askedQ(fetchMock).length).toBeGreaterThan(asked));
  });

  it("applies a saved view's order, groups and aggregates, and goes back to every row", async () => {
    const view = {
      id: "7f1c2a9e-4b1d-4a57-9a0e-2f6d1c3b8e11",
      type: TYPE,
      kind: "grid",
      mode: "personal",
      title: "Dusty first",
      owner: "jana.kovacova",
      config: {
        q: "pm10>20",
        sort: [{ attr: "pm10", desc: true }, { attr: "name" }],
        settings: { groupBy: ["status", 7], aggregations: { pm10: "median", name: "nonsense" }, rowSize: "large" },
      },
      version: 1,
      createdAt: "2026-10-06T19:00:00Z",
      updatedAt: "2026-10-06T19:00:00Z",
    };
    show({ views: { status: 200, body: { items: [view] } } });
    const types = await screen.findByRole("navigation", { name: en.spaces.inside.types });
    await userEvent.click(await within(types).findByRole("button", { name: "Dusty first" }));
    await waitFor(() => expect(seen.grid?.thenBy).toEqual([{ attr: "name", dir: "asc" }]));
    expect(seen.grid!.groupBy).toEqual(["status"]);
    expect(seen.grid!.rowSize).toBe("large");
    expect(seen.grid!.view.sort).toEqual({ attr: "pm10", dir: "desc" });
    await waitFor(() => expect(seen.viewBar?.current.settings.aggregations).toEqual({ pm10: "median" }));
    await userEvent.click(within(types).getByRole("button", { name: en.spaces.inside.allRows }));
    await waitFor(() => expect(seen.grid!.groupBy).toEqual([]));
    expect(seen.grid!.thenBy).toEqual([]);
    // The phone's picker opens another type.
    const picker = await waitFor(() => {
      const found = document.getElementById("space-type-picker");
      expect(found).not.toBeNull();
      return found as HTMLSelectElement;
    });
    await userEvent.selectOptions(picker, "AirQualityStation");
    await waitFor(() => expect(new URLSearchParams(window.location.search).get("type")).toBe("AirQualityStation"));
  });

  it("keeps the type when the grid's Type column picks the one already open", async () => {
    show();
    const first = await grid();
    act(() => first.typeChoice.onChange(TYPE));
    expect(new URLSearchParams(window.location.search).get("type")).toBeNull();
    act(() => seen.grid!.typeChoice.onChange("AirQualityStation"));
    await waitFor(() => expect(new URLSearchParams(window.location.search).get("type")).toBe("AirQualityStation"));
  });

  it("shares only the model's fields in the API view", async () => {
    show();
    await grid();
    await userEvent.click(screen.getByRole("tab", { name: en.spaces.views.kind.api }));
    await waitFor(() => expect(new URLSearchParams(window.location.search).get("view")).toBe("api"));
  });
});

describe("the rest of the space page (T-7678)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => vi.unstubAllGlobals());

  it("opens and closes the space's alerts", async () => {
    show();
    await userEvent.click(await screen.findByRole("button", { name: en.alerts.forSpace }));
    const alerts = await screen.findByRole("dialog", { name: "alerts space ovzdusie" });
    await userEvent.click(within(alerts).getByRole("button", { name: "close alerts" }));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "alerts space ovzdusie" })).toBeNull());
  });

  it("asks the model's source again when the types could not be read", async () => {
    const { fetchMock } = show({
      models: { status: 200, body: list([{ ...MODEL, spec: { artifacts: MODEL.spec.artifacts } }]) },
      source: { status: 500, body: { title: "Internal Server Error", detail: "git is down" } },
    });
    const retry = await screen.findByRole("button", { name: en.app.error.retry });
    const sourceReads = () => fetchMock.mock.calls.filter(([input]) => String(input instanceof Request ? input.url : input).endsWith("/source")).length;
    const before = sourceReads();
    await userEvent.click(retry);
    await waitFor(() => expect(sourceReads()).toBeGreaterThan(before));
  });

  it("asks the model's source again from the model tab", async () => {
    const { fetchMock } = show({ source: { status: 503, body: { title: "Service Unavailable", detail: "git is down" } } }, "model");
    const retries = await screen.findAllByRole("button", { name: en.app.error.retry });
    const before = fetchMock.mock.calls.length;
    await userEvent.click(retries[retries.length - 1]);
    await waitFor(() => expect(fetchMock.mock.calls.length).toBeGreaterThan(before));
  });

  it("finds an endpoint by its title, audience, representation or catalogue", async () => {
    show({}, "endpoints");
    await screen.findByRole("link", { name: "Open air data" });
    const search = screen.getByLabelText(en.dataGrid.searchRows);
    for (const text of ["Open air", "public", "csv", en.spaces.inside.notPublished]) {
      await userEvent.clear(search);
      await userEvent.type(search, text);
      await waitFor(() => expect(screen.getByRole("link", { name: "Open air data" })).toBeInTheDocument());
    }
    await userEvent.clear(search);
    await userEvent.type(search, "zzzz");
    await waitFor(() => expect(screen.queryByRole("link", { name: "Open air data" })).toBeNull());
  });

  it("finds a policy by its title, assignee, operation or type", async () => {
    show({}, "policies");
    await screen.findAllByText("Anyone reads the air");
    const search = screen.getByLabelText(en.dataGrid.searchRows);
    for (const text of ["Anyone", "public", en.spaces.inside.operationKind.delete, TYPE]) {
      await userEvent.clear(search);
      await userEvent.type(search, text);
      await waitFor(() => expect(screen.getAllByText("Anyone reads the air").length).toBeGreaterThan(0));
    }
    await userEvent.clear(search);
    await userEvent.type(search, "zzzz");
    await waitFor(() => expect(screen.queryByText("Anyone reads the air")).toBeNull());
  });

  it("says a policy that names no type covers every type", async () => {
    show({ policies: { status: 200, body: list([{ ...POLICY, spec: { ...POLICY.spec, information: [] } }]) } }, "policies");
    const search = await screen.findByLabelText(en.dataGrid.searchRows);
    await userEvent.type(search, en.spaces.inside.everyType);
    await waitFor(() => expect(screen.getAllByText("Anyone reads the air").length).toBeGreaterThan(0));
  });
});

describe("the space's model, endpoints and policies when there are none (T-7678)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => vi.unstubAllGlobals());

  it("says the models are loading, then offers to create one for a space that has none", async () => {
    show({ models: "hang" }, "model");
    expect((await screen.findAllByRole("status")).some((each) => each.textContent === en.app.loading)).toBe(true);
    vi.unstubAllGlobals();
  });

  it("offers Create and Import for a space with no model", async () => {
    show({ space: { status: 200, body: { ...SPACE, spec: {} } }, models: { status: 200, body: list([]) } }, "model");
    expect(await screen.findByText(en.spaces.inside.modelEmpty)).toBeInTheDocument();
    expect(screen.getByText(en.spaces.inside.modelCreate)).toBeInTheDocument();
  });

  it("says a model whose YAML does not parse cannot be read, never an empty class list", async () => {
    show({ source: { status: 200, body: "classes: [unclosed" } }, "model");
    expect(await screen.findByText(en.spaces.inside.modelUnreadable)).toBeInTheDocument();
  });

  it("says a space has no endpoint and no policy, and a type no endpoint reads has no count", async () => {
    show({ endpoints: { status: 200, body: list([]) }, policies: { status: 200, body: list([]) } }, "endpoints");
    expect(await screen.findByText(en.spaces.inside.noEndpoints)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("tab", { name: en.spaces.inside.policies }));
    expect(await screen.findByText(en.spaces.inside.noPolicies)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("tab", { name: en.spaces.inside.model }));
    const row = await screen.findByRole("row", { name: new RegExp(TYPE) });
    expect(within(row).getAllByText("—").length).toBeGreaterThan(0);
  });
});

describe("a sample as a person reads it (T-7678)", () => {
  it("has no class names without a source", () => {
    expect(classNamesOf(undefined)).toBeNull();
    expect(classNamesOf("classes:\n  A: {}\n")).toEqual(["A"]);
    // A YAML error, and an alias bomb the parser refuses to expand, are both unreadable.
    expect(classNamesOf("classes: [unclosed")).toBeNull();
    expect(classNamesOf(`a: &a [x]\nb: [${Array.from({ length: 120 }, () => "*a").join(", ")}]\n`)).toBeNull();
  });

  it("writes an instant as a date and time, a URN as its own name, and keeps a date that is none", () => {
    expect(readable("seen 2026-09-25T11:40:00Z", "en")).toBe(`seen ${new Intl.DateTimeFormat("en", { dateStyle: "medium", timeStyle: "short" }).format(new Date("2026-09-25T11:40:00Z"))}`);
    expect(readable("at urn:ngsi-ld:Pipeline:hel.fi:feed-7", "en")).toBe("at feed-7");
    expect(readable("2026-13-45T10:00Z", "en")).toBe("2026-13-45T10:00Z");
    expect(readable({ value: 8.897155361050329 }, "en")).toBe("8.9");
  });

  it("copies a sample's full id, and says so only once the browser took it", async () => {
    await i18n.changeLanguage("en");
    const writeText = vi.fn().mockResolvedValueOnce(undefined).mockRejectedValueOnce(new Error("denied"));
    vi.stubGlobal("navigator", { ...navigator, clipboard: { writeText } });
    render(
      <I18nextProvider i18n={i18n}>
        <Samples samples={[{ id: ID1, type: TYPE, pm10: 12 }]} />
      </I18nextProvider>,
    );
    const copy = screen.getByRole("button", { name: en.spaces.inside.copyId.replace("{id}", ID1) });
    await userEvent.click(copy);
    expect(writeText).toHaveBeenCalledWith(ID1);
    await waitFor(() => expect(copy).toHaveAttribute("title", en.endpoints.copied));
    await userEvent.click(copy);
    await waitFor(() => expect(copy).not.toHaveAttribute("title", en.endpoints.copied));
    vi.unstubAllGlobals();
  });
});
