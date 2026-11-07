/**
 * T-6115: the one grid of a project's manifests. The widgets come from the kind's jc-core JSON
 * Schema (fixtures: joinedcontext-platform schemas/kinds at a454c775), search, chips and sort
 * narrow and order the rows, and a cell edit is a checked proposal the server decides: its 403
 * shows on the field it was for.
 */
import { cleanup, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import "../src/i18n";
import en from "../src/locales/en.json";
import type { Manifest } from "../src/api/manifest";
import { fieldWidget, hintText, specValue, withSpecValue } from "../src/components/grid/fieldWidget";
import { GridFooter, GridRow, GridToolbar, ManifestGrid, example, valueCounts, visibleRows } from "../src/components/grid/ManifestGrid";
import type { GridColumn } from "../src/components/grid/ManifestGrid";
import endpointSchema from "./fixtures/kinds/Endpoint.json";
import subscriptionSchema from "./fixtures/kinds/Subscription.json";
import pipelineSchema from "./fixtures/kinds/Pipeline.json";
import dataModelSchema from "./fixtures/kinds/DataModel.json";
import { json, list, problem, renderPage } from "./page_contract";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

/** The row's Details: the first item of its one menu (owner 2026-10-31). */
async function openDetails(name: string): Promise<void> {
  await userEvent.click(await screen.findByRole("button", { name: new RegExp(`More actions.*${name}`) }));
  await userEvent.click(await screen.findByRole("menuitem", { name: en.grid.details }));
}

describe("fieldWidget reads the control off the kind's schema", () => {
  it("gives Endpoint an audience dropdown, a representation set, a space picker and an MCP switch", () => {
    const audience = fieldWidget("Endpoint", endpointSchema, "audience");
    expect(audience?.widget).toMatchObject({ kind: "enum" });
    expect(audience?.widget.kind === "enum" && audience.widget.options.map((o) => o.value)).toEqual(
      expect.arrayContaining(["public", "organization", "project-list"]),
    );
    const representations = fieldWidget("Endpoint", endpointSchema, "enabledRepresentations")?.widget;
    expect(representations?.kind).toBe("multiEnum");
    expect(representations?.kind === "multiEnum" && representations.options.map((o) => o.value)).toContain("ngsi-ld");
    expect(fieldWidget("Endpoint", endpointSchema, "contextSpaceRef")?.widget).toEqual({ kind: "reference", plural: "spaces" });
    expect(fieldWidget("Endpoint", endpointSchema, "mcp")?.widget).toEqual({ kind: "boolean", absent: true });
  });

  it("gives Subscription a switch, a number and a date-time, and Pipeline a class dropdown and a duration", () => {
    expect(fieldWidget("Subscription", subscriptionSchema, "isActive")?.widget).toEqual({ kind: "boolean", absent: true });
    expect(fieldWidget("Subscription", subscriptionSchema, "throttling")?.widget).toMatchObject({ kind: "number", integer: true });
    expect(fieldWidget("Subscription", subscriptionSchema, "expiresAt")?.widget).toEqual({ kind: "dateTime" });
    expect(fieldWidget("Pipeline", pipelineSchema, "class")?.widget.kind).toBe("enum");
    expect(fieldWidget("Pipeline", pipelineSchema, "period")?.widget).toEqual({ kind: "duration" });
    expect(fieldWidget("Pipeline", pipelineSchema, "enabled")?.widget).toEqual({ kind: "boolean", absent: true });
    expect(fieldWidget("DataModel", dataModelSchema, "openWorld")?.widget).toMatchObject({ kind: "boolean" });
    expect(fieldWidget("DataModel", dataModelSchema, "lifecycle")?.widget.kind).toBe("enum");
  });

  it("turns a schema sentence into a hint without ids or code ticks", () => {
    expect(hintText("Audience access scope (EP-14). More.")).toBe("Audience access scope.");
    expect(hintText("`false` turns it off (EP-24, T-2901); see [`X::y`].")).toBe("Off turns it off; see X::y.");
    expect(hintText(undefined)).toBeUndefined();
  });

  it("hints name exactly the values a choice offers, and a switch's sentence reads as the switch", () => {
    const t = (key: string, values?: Record<string, unknown>) => `${key} ${JSON.stringify(values ?? {})}`;
    const audience = fieldWidget("Endpoint", endpointSchema, "audience");
    if (audience?.widget.kind !== "enum") throw new Error("audience is no enum");
    const offered = audience.widget.options.map((option) => option.value);
    expect(example(audience.widget, t)).toBe(`grid.choose.hint ${JSON.stringify({ options: offered.join(", ") })}`);
    // A column's own labels name the choices as the table shows them.
    expect(example(audience.widget, t, (value) => value.toUpperCase())).toBe(`grid.choose.hint ${JSON.stringify({ options: offered.map((value) => value.toUpperCase()).join(", ") })}`);
    const representations = fieldWidget("Endpoint", endpointSchema, "enabledRepresentations");
    expect(example(representations!.widget, t)).toMatch(/^grid\.multi\.hint /);
    expect(en.grid.multi.hint).toMatch(/^Tick any/);
    const mcp = fieldWidget("Endpoint", endpointSchema, "mcp");
    expect(hintText(mcp?.description)).not.toMatch(/^`?(true|false)\b/);
  });

  it("refuses what one cell cannot hold and a field the schema lacks", () => {
    expect(fieldWidget("Endpoint", endpointSchema, "roles")).toBeNull();
    expect(fieldWidget("Endpoint", endpointSchema, "nope")).toBeNull();
    expect(fieldWidget("Endpoint", null, "audience")).toBeNull();
  });

  it("writes one spec member and keeps every other", () => {
    const manifest = { spec: { audience: "public", notification: { format: "normalized", uri: "x" } } };
    expect(specValue(manifest, "notification.format")).toBe("normalized");
    expect(withSpecValue(manifest, "notification.format", "keyValues").spec).toEqual({ audience: "public", notification: { format: "keyValues", uri: "x" } });
    expect(withSpecValue(manifest, "audience", undefined).spec).toEqual({ notification: manifest.spec.notification });
    expect(manifest.spec.audience).toBe("public");
  });
});

type Row = { name: string; audience: string; calls: number };
const columns: GridColumn<Row>[] = [
  { id: "audience", label: "Audience", filter: true, text: (row) => row.audience },
  { id: "calls", label: "Calls", numeric: true, text: (row) => String(row.calls) },
];
const name: GridColumn<Row> = { id: "name", label: "Name", text: (row) => row.name };
const rows: Row[] = [
  { name: "air-quality", audience: "public", calls: 9 },
  { name: "parking", audience: "organization", calls: 120 },
  { name: "bikes", audience: "public", calls: 30 },
];

describe("visibleRows", () => {
  it("searches every column, keeps the chips' values and sorts numbers as numbers", () => {
    const all = [name, ...columns];
    expect(visibleRows(rows, all, "PARK", {}, null).map((r) => r.name)).toEqual(["parking"]);
    expect(visibleRows(rows, all, "", { audience: ["public"] }, null).map((r) => r.name)).toEqual(["air-quality", "bikes"]);
    expect(visibleRows(rows, all, "", {}, { column: "calls", desc: true }).map((r) => r.name)).toEqual(["parking", "bikes", "air-quality"]);
    expect(visibleRows(rows, all, "", {}, { column: "name", desc: false }).map((r) => r.name)).toEqual(["air-quality", "bikes", "parking"]);
  });
});

const endpoint = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Endpoint",
  metadata: { name: "air-quality", namespace: "helsinki" },
  spec: { contextSpaceRef: "air", audience: "organization", slug: "airqualityhelsinkiaaaaaaaaaa" },
  status: { phase: "Live" },
} as unknown as Manifest;

function grid(onChange = vi.fn()) {
  return (
    <ManifestGrid<Manifest>
      caption="Endpoints"
      storageKey="test-endpoints"
      searchLabel="Search endpoints"
      rows={[endpoint]}
      rowKey={(row) => row.metadata.name}
      query={{ isPending: false, isError: false, error: null, refetch: () => undefined }}
      name={{ id: "name", label: "Name", text: (row) => row.metadata.name }}
      columns={[
        { id: "audience", label: "Audience", field: "audience", filter: true, text: (row) => String((row.spec as { audience?: string }).audience) },
        { id: "mcp", label: "MCP tools", field: "mcp", hidden: true, text: () => "On" },
      ]}
      edit={{ project: "helsinki", plural: "endpoints", kind: "Endpoint", manifest: (row) => row, onChange }}
      empty={<p>Nothing yet</p>}
    />
  );
}

function harness(put: (body: unknown, dryRun: boolean) => Response, mayPropose = true, stored: Manifest = endpoint) {
  const sent: { dryRun: boolean; body: unknown; ifMatch: string | null }[] = [];
  const answer = async (url: URL, request: Request) => {
    if (url.pathname === "/api/v1/projects/helsinki/endpoints/air-quality" && request.method === "GET") {
      return new Response(JSON.stringify(stored), { headers: { "Content-Type": "application/json", ETag: '"v2"' } });
    }
    if (url.pathname === "/api/v1/kinds/Endpoint/schema") return json(endpointSchema);
    if (url.pathname.endsWith("/permissions/me")) {
      return json({ project: "helsinki", bootstrap: false, grants: [{ scope: "project:helsinki", rule: { kinds: ["Endpoint"], verbs: mayPropose ? ["read", "propose"] : ["read"] } }] });
    }
    if (url.pathname === "/api/v1/projects/helsinki/spaces") return json(list([{ apiVersion: "joinedcontext.com/v1alpha1", kind: "ContextSpace", metadata: { name: "air", namespace: "helsinki" }, spec: {} }]));
    if (url.pathname === "/api/v1/projects/helsinki/endpoints/air-quality" && request.method === "PUT") {
      const body = (await request.json()) as unknown;
      const dryRun = url.searchParams.get("dryRun") === "All";
      sent.push({ dryRun, body, ifMatch: request.headers.get("If-Match") });
      return put(body, dryRun);
    }
    return undefined;
  };
  return { sent, answer };
}

describe("valueCounts", () => {
  it("counts each value in one pass over the rows, sorted, without the empty ones", () => {
    const text = vi.fn((row: { phase: string }) => row.phase);
    const rows = ["Live", "Failed", "", "Live", "Pending", "Live"].map((phase) => ({ phase }));
    expect(valueCounts(rows, { id: "phase", label: "Phase", text })).toEqual([
      ["Failed", 1],
      ["Live", 3],
      ["Pending", 1],
    ]);
    // A scan per value read the 10 000 rows of a project once per value, on every render (T-5181).
    expect(text).toHaveBeenCalledTimes(rows.length);
  });
});

describe("ManifestGrid", () => {
  it("hides a column until ticked under Columns, and the panel still shows it", async () => {
    const { answer } = harness(() => json({}));
    renderPage(grid(), { answer });
    const table = await screen.findByRole("table", { name: "Endpoints" });
    expect(within(table).queryByRole("columnheader", { name: /MCP tools/ })).toBeNull();
    await openDetails("air-quality");
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText("MCP tools")).toBeInTheDocument();
  });

  it("filters by a chip and clears it", async () => {
    const { answer } = harness(() => json({}));
    renderPage(grid(), { answer });
    await screen.findByRole("table", { name: "Endpoints" });
    await userEvent.click(screen.getByText(en.grid.filter));
    await userEvent.click(screen.getByRole("checkbox", { name: /organization/ }));
    const chip = screen.getByRole("button", { name: en.grid.removeFilter.replace("{column}", "Audience").replace("{value}", "organization") });
    expect(screen.getByText("air-quality")).toBeInTheDocument();
    await userEvent.click(chip);
    expect(screen.queryByRole("button", { name: /Remove filter/ })).toBeNull();
  });

  it("proposes an audience change through the checked PUT and hands back the Change", async () => {
    const change = { apiVersion: "joinedcontext.com/v1alpha1", kind: "Change", metadata: { name: "c-1", namespace: "helsinki" }, status: { state: "Pending" } };
    const { sent, answer } = harness((_body, dryRun) => json(dryRun ? { valid: true } : change, dryRun ? 200 : 202));
    const onChange = vi.fn();
    renderPage(grid(onChange), { answer });
    await openDetails("air-quality");
    const dialog = await screen.findByRole("dialog");
    const select = await within(dialog).findByRole("combobox", { name: "Audience" });
    expect(select).toHaveAccessibleDescription(/Pick one of project-list, organization, public\./);
    await userEvent.selectOptions(select, "public");
    await userEvent.click(within(dialog).getByRole("button", { name: en.grid.propose }));
    await waitFor(() => expect(onChange).toHaveBeenCalled());
    expect(sent.map((each) => each.dryRun)).toEqual([true, false]);
    const body = sent[1].body as { spec: Record<string, unknown>; status?: unknown };
    expect(body.spec).toEqual({ contextSpaceRef: "air", audience: "public", slug: "airqualityhelsinkiaaaaaaaaaa" });
    expect(body.status).toBeUndefined();
  });

  // F-R2UI2-007: a Change merged after the list was read must survive a cell edit.
  it("edits the manifest as it is now, not the row the list cached, and sends its ETag", async () => {
    const merged = { ...endpoint, spec: { ...(endpoint.spec as object), title: "merged since" } } as unknown as Manifest;
    const change = { apiVersion: "joinedcontext.com/v1alpha1", kind: "Change", metadata: { name: "c-2", namespace: "helsinki" }, status: { state: "Pending" } };
    const { sent, answer } = harness((_body, dryRun) => json(dryRun ? { valid: true } : change, dryRun ? 200 : 202), true, merged);
    const onChange = vi.fn();
    renderPage(grid(onChange), { answer });
    await openDetails("air-quality");
    const dialog = await screen.findByRole("dialog");
    await userEvent.selectOptions(await within(dialog).findByRole("combobox", { name: "Audience" }), "public");
    await userEvent.click(within(dialog).getByRole("button", { name: en.grid.propose }));
    await waitFor(() => expect(onChange).toHaveBeenCalled());
    expect((sent[1].body as { spec: Record<string, unknown> }).spec).toEqual({
      contextSpaceRef: "air",
      audience: "public",
      slug: "airqualityhelsinkiaaaaaaaaaa",
      title: "merged since",
    });
    expect(sent.map((each) => each.ifMatch)).toEqual(['"v2"', '"v2"']);
  });

  it("shows the server's 403 on the field it was for", async () => {
    const { answer } = harness(() => problem(403, "You may not change the audience of Endpoint air-quality."));
    renderPage(grid(), { answer });
    await openDetails("air-quality");
    const dialog = await screen.findByRole("dialog");
    await userEvent.selectOptions(await within(dialog).findByRole("combobox", { name: "Audience" }), "public");
    await userEvent.click(within(dialog).getByRole("button", { name: en.grid.propose }));
    const select = within(dialog).getByRole("combobox", { name: "Audience" });
    await waitFor(() => expect(select).toHaveAccessibleDescription(/may not change the audience/));
    // The refused value cannot be sent again; a new value clears the refusal and the button.
    expect(within(dialog).getByRole("button", { name: new RegExp(en.grid.propose) })).toHaveAttribute("aria-disabled", "true");
    await userEvent.selectOptions(select, "project-list");
    expect(select).not.toHaveAccessibleDescription(/may not change the audience/);
    expect(within(dialog).getByRole("button", { name: en.grid.propose })).not.toHaveAttribute("aria-disabled");
  });

  it("disables the proposal for a person without propose on the kind, with the reason", async () => {
    const { answer } = harness(() => json({}), false);
    renderPage(grid(), { answer });
    await openDetails("air-quality");
    const dialog = await screen.findByRole("dialog");
    await userEvent.selectOptions(await within(dialog).findByRole("combobox", { name: "Audience" }), "public");
    await waitFor(() => expect(within(dialog).getByRole("button", { name: en.grid.propose })).toHaveAttribute("aria-disabled", "true"));
  });
});

describe("the grid's parts", () => {
  const name: GridColumn<Manifest> = { id: "name", label: "Name", text: (row) => row.metadata.name };
  const types: GridColumn<Manifest> = {
    id: "types",
    label: "Types",
    text: () => "A, B",
    cell: () => (
      <>
        <span>AirQualityObserved</span>
        <span>WeatherObserved</span>
      </>
    ),
  };

  it("GridRow keeps a cell of several parts as one value beside its label", async () => {
    const onDetails = vi.fn();
    renderPage(
      <table>
        <tbody>
          <GridRow row={endpoint} name={name} columns={[types]} onDetails={onDetails} />
        </tbody>
      </table>,
      { answer: () => undefined },
    );
    await screen.findByText("AirQualityObserved");
    const cell = document.querySelector('td[data-label="Types"]');
    expect(cell?.children).toHaveLength(1);
    expect(cell?.textContent).toBe("AirQualityObservedWeatherObserved");
  });

  it("GridFooter says how many show and offers the rest", async () => {
    const onMore = vi.fn();
    const { container } = renderPage(<GridFooter shown={50} kept={50} total={50} step={50} onMore={onMore} />, { answer: () => undefined });
    expect(container.querySelector('[role="status"]')).toBeNull();
    cleanup();
    renderPage(<GridFooter shown={50} kept={70} total={90} step={50} onMore={onMore} />, { answer: () => undefined });
    expect(await screen.findByText(/Showing 50 of 70/)).toHaveTextContent("Showing 50 of 70 matching, 90 in total");
    await userEvent.click(screen.getByRole("button", { name: "Show 20 more" }));
    expect(onMore).toHaveBeenCalled();
  });

  it("GridToolbar searches, and removes a chip it shows", async () => {
    const set = vi.fn();
    const setFilters = vi.fn();
    renderPage(
      <GridToolbar<Manifest>
        caption="Endpoints"
        rows={[endpoint]}
        all={[name, types]}
        columns={[types]}
        ownSearch={{ value: "", set, label: "Search endpoints" }}
        filters={{ types: ["A, B"] }}
        setFilters={setFilters}
        shown={["types"]}
        choose={vi.fn()}
        sort={null}
        setSort={vi.fn()}
        sortId="sort"
      />,
      { answer: () => undefined },
    );
    await userEvent.type(await screen.findByRole("searchbox", { name: "Search endpoints" }), "a");
    expect(set).toHaveBeenCalledWith("a");
    await userEvent.click(screen.getByRole("button", { name: "Remove filter Types: A, B" }));
    expect(setFilters).toHaveBeenCalled();
  });
});
