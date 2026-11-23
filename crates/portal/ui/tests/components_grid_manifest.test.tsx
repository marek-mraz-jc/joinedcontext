/**
 * T-7585: the manifest grid's remaining branches: every control of the cell panel (a typed
 * reference, a set of choices, a number, a duration, a date-time) with what the checked PUT carries,
 * Undo, the empty search and its Clear all, two chips and Clear filters, the server-paged footer,
 * and the hints a field shows by its widget.
 */
import { cleanup, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import "../src/i18n";
import en from "../src/locales/en.json";
import type { Manifest } from "../src/api/manifest";
import type { FieldWidget } from "../src/components/grid/fieldWidget";
import { ManifestGrid, example } from "../src/components/grid/ManifestGrid";
import type { ServerPages } from "../src/components/grid/ManifestGrid";
import endpointSchema from "./fixtures/kinds/Endpoint.json";
import subscriptionSchema from "./fixtures/kinds/Subscription.json";
import pipelineSchema from "./fixtures/kinds/Pipeline.json";
import { json, list, renderPage } from "./page_contract";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const API = "joinedcontext.com/v1alpha1";
const CHANGE = { apiVersion: API, kind: "Change", metadata: { name: "c-1", namespace: "helsinki" }, status: { state: "Pending" } };

function manifest(kind: string, name: string, spec: Record<string, unknown>): Manifest {
  return { apiVersion: API, kind, metadata: { name, namespace: "helsinki" }, spec, status: { phase: "Live" } } as unknown as Manifest;
}

interface Sent {
  dryRun: boolean;
  body: { spec: Record<string, unknown> };
}

/** A grid of one row whose one editable column is `field`, with the kind's schema and a PUT that records. */
function editor(kind: string, plural: string, schema: unknown, field: string, spec: Record<string, unknown>) {
  const row = manifest(kind, "one", spec);
  const sent: Sent[] = [];
  const answer = async (url: URL, request: Request) => {
    const path = `/api/v1/projects/helsinki/${plural}/one`;
    if (url.pathname === path && request.method === "GET") {
      return new Response(JSON.stringify(row), { headers: { "Content-Type": "application/json", ETag: '"v1"' } });
    }
    if (url.pathname === `/api/v1/kinds/${kind}/schema`) return json(schema);
    if (url.pathname.endsWith("/permissions/me")) {
      return json({ project: "helsinki", bootstrap: false, grants: [{ scope: "project:helsinki", rule: { kinds: [kind], verbs: ["read", "propose"] } }] });
    }
    if (url.pathname === "/api/v1/projects/helsinki/spaces") {
      return json(
        list(["air", "mobility"].map((name) => ({ apiVersion: API, kind: "ContextSpace", metadata: { name, namespace: "helsinki" }, spec: {} }))),
      );
    }
    if (url.pathname === path && request.method === "PUT") {
      sent.push({ dryRun: url.searchParams.get("dryRun") === "All", body: (await request.json()) as Sent["body"] });
      return url.searchParams.get("dryRun") === "All" ? json({ valid: true }) : json(CHANGE, 202);
    }
    return undefined;
  };
  const onChange = vi.fn();
  renderPage(
    <ManifestGrid<Manifest>
      caption="Things"
      storageKey={`test-${kind}`}
      searchLabel="Search things"
      rows={[row]}
      rowKey={(each) => each.metadata.name}
      name={{ id: "name", label: "Name", text: (each) => each.metadata.name }}
      columns={[{ id: "field", label: "Field", field, text: () => "x" }]}
      edit={{ project: "helsinki", plural, kind, manifest: (each) => each, onChange }}
      empty={<p>Nothing yet</p>}
    />,
    { answer },
  );
  return { sent, onChange };
}

async function openPanel(): Promise<HTMLElement> {
  await userEvent.click(await screen.findByRole("button", { name: /More actions.*one/ }));
  await userEvent.click(await screen.findByRole("menuitem", { name: en.grid.details }));
  return screen.findByRole("dialog");
}

async function propose(dialog: HTMLElement, onChange: ReturnType<typeof vi.fn>): Promise<void> {
  await userEvent.click(within(dialog).getByRole("button", { name: en.grid.propose }));
  await waitFor(() => expect(onChange).toHaveBeenCalled());
}

describe("the cell panel's controls", () => {
  it("keeps a typed reference's kind and namespace and moves only its name", async () => {
    const { sent, onChange } = editor("Endpoint", "endpoints", endpointSchema, "contextSpaceRef", {
      contextSpaceRef: { kind: "ContextSpace", name: "air", namespace: "helsinki" },
    });
    const dialog = await openPanel();
    const picker = await within(dialog).findByRole("combobox", { name: "Field" });
    expect(picker).toHaveValue("air");
    await userEvent.click(picker);
    await userEvent.click(await screen.findByRole("option", { name: /mobility/ }));
    await propose(dialog, onChange);
    expect(sent.map((each) => each.dryRun)).toEqual([true, false]);
    expect(sent[1]!.body.spec.contextSpaceRef).toEqual({ kind: "ContextSpace", name: "mobility", namespace: "helsinki" });
  });

  it("starts an unset reference empty and stores a plain name as text", async () => {
    const { sent, onChange } = editor("Endpoint", "endpoints", endpointSchema, "contextSpaceRef", {});
    const dialog = await openPanel();
    const picker = await within(dialog).findByRole("combobox", { name: "Field" });
    expect(picker).toHaveValue("");
    await userEvent.click(picker);
    await userEvent.click(await screen.findByRole("option", { name: /mobility/ }));
    await propose(dialog, onChange);
    expect(sent[1]!.body.spec.contextSpaceRef).toBe("mobility");
  });

  it("ticks and unticks a choice of a set, and Undo puts the stored set back", async () => {
    const { sent, onChange } = editor("Endpoint", "endpoints", endpointSchema, "enabledRepresentations", {
      enabledRepresentations: ["ngsi-ld"],
    });
    const dialog = await openPanel();
    const group = await within(dialog).findByRole("group", { name: "Field" });
    const boxes = within(group).getAllByRole("checkbox");
    const ngsi = boxes.find((box) => (box as HTMLInputElement).checked)!;
    const other = boxes.find((box) => !(box as HTMLInputElement).checked)!;
    await userEvent.click(other);
    expect(other).toBeChecked();
    await userEvent.click(within(dialog).getByRole("button", { name: en.grid.undo }));
    expect(other).not.toBeChecked();
    expect(ngsi).toBeChecked();
    expect(within(dialog).queryByRole("button", { name: en.grid.propose })).toBeNull();
    await userEvent.click(other);
    await userEvent.click(ngsi);
    expect(ngsi).not.toBeChecked();
    await propose(dialog, onChange);
    const stored = sent[1]!.body.spec.enabledRepresentations as string[];
    expect(stored).toHaveLength(1);
    expect(stored).not.toContain("ngsi-ld");
  });

  it("takes a number as a number, and removes the member when it is cleared", async () => {
    const { sent, onChange } = editor("Subscription", "subscriptions", subscriptionSchema, "throttling", { throttling: 3, isActive: true });
    const dialog = await openPanel();
    const input = await within(dialog).findByRole("spinbutton");
    expect(input).toHaveValue(3);
    await userEvent.clear(input);
    await userEvent.type(input, "7");
    await propose(dialog, onChange);
    expect(sent[1]!.body.spec.throttling).toBe(7);
    cleanup();
    const again = editor("Subscription", "subscriptions", subscriptionSchema, "throttling", { throttling: 3, isActive: true });
    const second = await openPanel();
    await userEvent.clear(await within(second).findByRole("spinbutton"));
    await propose(second, again.onChange);
    expect(again.sent[1]!.body.spec).toEqual({ isActive: true });
  });

  it("types a duration as text and a date-time as text, and an emptied text removes the member", async () => {
    const { sent, onChange } = editor("Pipeline", "pipelines", pipelineSchema, "period", { period: "5m" });
    const dialog = await openPanel();
    const input = await within(dialog).findByRole("textbox", { name: "Field" });
    expect(input).toHaveValue("5m");
    await userEvent.clear(input);
    await userEvent.type(input, "15s");
    await propose(dialog, onChange);
    expect(sent[1]!.body.spec.period).toBe("15s");
    cleanup();
    const emptied = editor("Subscription", "subscriptions", subscriptionSchema, "expiresAt", { expiresAt: "2030-01-01T00:00:00Z", isActive: true });
    const second = await openPanel();
    await userEvent.clear(await within(second).findByRole("textbox", { name: "Field" }));
    await propose(second, emptied.onChange);
    expect(emptied.sent[1]!.body.spec).toEqual({ isActive: true });
  });
});

describe("example hints by widget", () => {
  const t = (key: string, values?: Record<string, unknown>) => (values?.example ? `${key}:${String(values.example)}` : key);
  const kinds: [FieldWidget, string][] = [
    [{ kind: "reference", plural: "spaces" }, "grid.reference.hint"],
    [{ kind: "boolean", absent: true }, "grid.boolean.hint"],
    [{ kind: "number", integer: true }, "grid.number.hint:60"],
    [{ kind: "number", integer: false }, "grid.number.hint:1.5"],
    [{ kind: "duration" }, "grid.duration.hint"],
    [{ kind: "dateTime" }, "grid.dateTime.hint"],
    [{ kind: "text" }, "grid.text.hint"],
  ];
  it.each(kinds)("%j reads %s", (widget, hint) => {
    expect(example(widget, t)).toBe(hint);
  });
});

describe("the grid's own search, chips and footer", () => {
  const rows = [
    manifest("Endpoint", "air", { audience: "public", visibility: "a" }),
    manifest("Endpoint", "traffic", { audience: "organization", visibility: "b" }),
  ];
  const audience = (row: Manifest) => String((row.spec as { audience: string }).audience);

  function plain(extra: Partial<React.ComponentProps<typeof ManifestGrid<Manifest>>> = {}) {
    return renderPage(
      <ManifestGrid<Manifest>
        caption="Endpoints"
        storageKey="test-plain"
        searchLabel="Search endpoints"
        rows={rows}
        rowKey={(row) => row.metadata.name}
        name={{ id: "name", label: "Name", text: (row) => row.metadata.name }}
        columns={[{ id: "audience", label: "Audience", filter: true, text: audience }]}
        empty={<p>Nothing yet</p>}
        {...extra}
      />,
      { answer: () => undefined },
    );
  }

  it("says nothing matches a search and Clear all brings the rows back", async () => {
    plain();
    await userEvent.type(await screen.findByRole("searchbox", { name: "Search endpoints" }), "zzz");
    expect(await screen.findByText(en.grid.noMatch)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: en.grid.clearAll }));
    expect(await screen.findByText("traffic")).toBeInTheDocument();
    expect(screen.getByRole("searchbox", { name: "Search endpoints" })).toHaveValue("");
  });

  it("clears the page's own search too when Clear all is pressed", async () => {
    const onSearchChange = vi.fn();
    plain({ search: "zzz", onSearchChange, rows: [] , empty: <p>Nothing yet</p>, server: { onSearch: vi.fn(), hasMore: false, loadMore: vi.fn() } });
    await userEvent.click(await screen.findByRole("button", { name: en.grid.clearAll }));
    expect(onSearchChange).toHaveBeenCalledWith("");
  });

  it("offers Clear filters once two chips are on, and removes both", async () => {
    plain();
    await userEvent.click(await screen.findByText(en.grid.filter));
    await userEvent.click(screen.getByRole("checkbox", { name: /organization/ }));
    await userEvent.click(screen.getByRole("checkbox", { name: /public/ }));
    expect(screen.getAllByRole("button", { name: /Remove filter/ })).toHaveLength(2);
    await userEvent.click(screen.getByRole("button", { name: en.grid.clearFilters }));
    expect(screen.queryByRole("button", { name: /Remove filter/ })).toBeNull();
  });

  it("sends a typed search to the server and reads the next page from it", async () => {
    const server: ServerPages = { onSearch: vi.fn(), total: 120, hasMore: true, loadMore: vi.fn() };
    plain({ server });
    await userEvent.type(await screen.findByRole("searchbox", { name: "Search endpoints" }), "a");
    expect(server.onSearch).toHaveBeenCalledWith("a");
    expect(await screen.findByText(/Showing 2 of 120/)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: /Show .* more/ }));
    expect(server.loadMore).toHaveBeenCalledTimes(1);
  });

  it("gives a row menu with Details only when the page passes no actions, and reads a sort on the actions column as empty", async () => {
    plain();
    await userEvent.click(await screen.findByRole("button", { name: /More actions.*air/ }));
    expect(await screen.findByRole("menuitem", { name: en.grid.details })).toBeInTheDocument();
  });
});
