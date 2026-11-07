/**
 * The cell inspector (T-5528): what the person may do with one attribute and why, drawn from the
 * Endpoint's own decisions. The grid opens an editor only where the decisions say write.
 */
import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { CellInspector, DEFAULT_INSPECTOR_LABELS } from "../src/grid/CellInspector";
import { EntityGrid } from "../src/grid/EntityGrid";
import { parseGridConfig } from "../src/grid/config";
import { accessOf, endpointSource, fixtureSource } from "../src/grid/source";
import type { AttributeAccess } from "../src/grid/source";

const L = DEFAULT_INSPECTOR_LABELS;
const css = readFileSync(join(__dirname, "../src/grid/grid.css"), "utf8");

const entities: Record<string, unknown>[] = [
  {
    id: "urn:ngsi-ld:AirQualityObserved:1",
    type: "AirQualityObserved",
    name: { type: "Property", value: "Kallio" },
    no2: { type: "Property", value: 21 },
    pm10: { type: "Property", value: 8 },
  },
];

const table: AttributeAccess[] = [
  { name: "name", read: true, write: true, delete: false, reason: "grant" },
  { name: "no2", read: true, write: false, delete: false, reason: "prohibited" },
  { name: "pm10", read: true, write: false, delete: false, reason: "not_in_grant" },
];

function config() {
  const parsed = parseGridConfig({
    source: { kind: "fixture", name: "test" },
    type: "AirQualityObserved",
    columns: [{ attr: "name" }, { attr: "no2" }, { attr: "pm10" }],
    mode: "edit",
    editableAttrs: ["name", "no2", "pm10"],
    pageSize: 10,
  });
  return parsed.config!;
}

function source(access?: () => Promise<AttributeAccess[]>) {
  const inner = fixtureSource(entities);
  return { query: inner.query, get: inner.get, patch: async () => undefined, ...(access ? { access } : {}) };
}

const chips = () => within(screen.getByRole("list", { name: L.title })).getAllByRole("listitem").map((chip) => [chip.className, chip.textContent]);

describe("CellInspector", () => {
  it.each([
    ["grant", L.grant],
    ["not_in_grant", L.notInGrant],
    ["no_grant", L.noGrant],
    ["formula", L.formula],
    ["prohibited", L.prohibited],
  ] as const)("says %s in words", (reason, words) => {
    render(<CellInspector attribute="no2" label="NO2" access={{ name: "no2", read: true, write: false, delete: false, reason }} onClose={() => undefined} />);
    expect(screen.getByRole("status")).toHaveTextContent(words);
  });

  it("colours each allowed action and locks each refused one", () => {
    render(<CellInspector attribute="no2" label="NO2" access={{ name: "no2", read: true, write: true, delete: false, reason: "grant" }} onClose={() => undefined} />);
    expect(chips()).toEqual([
      ["jc-grid-chip jc-grid-chip--read", `${L.read}: ${L.allowed}`],
      ["jc-grid-chip jc-grid-chip--write", `${L.write}: ${L.allowed}`],
      ["jc-grid-chip jc-grid-chip--none", `${L.delete}: ${L.notAllowed}`],
    ]);
  });

  it("names the policy for a reader of it, says it is hidden for anyone else", async () => {
    const explain = vi.fn().mockResolvedValue({ policy: "no2-frozen", policyTitle: "Lab only", policyHidden: false });
    const access: AttributeAccess = { name: "no2", read: true, write: false, delete: false, reason: "prohibited" };
    const { unmount } = render(<CellInspector attribute="no2" label="NO2" access={access} explain={explain} onClose={() => undefined} />);
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("Explicitly not allowed by policy “Lab only”."));
    expect(explain).toHaveBeenCalledWith("no2", "write");
    unmount();
    const hidden = vi.fn().mockResolvedValue({ policy: null, policyHidden: true });
    render(<CellInspector attribute="no2" label="NO2" access={access} explain={hidden} onClose={() => undefined} />);
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent(L.prohibitedHidden));
  });

  it("asks nothing about a policy where none refused", () => {
    const explain = vi.fn();
    render(<CellInspector attribute="pm10" label="PM10" access={table[2]} explain={explain} askAccessHref="/projects/p/access-requests" onClose={() => undefined} />);
    expect(explain).not.toHaveBeenCalled();
    expect(screen.getByRole("link", { name: L.askAccess })).toHaveAttribute("href", "/projects/p/access-requests");
  });

  it("says when the decisions could not be read", () => {
    render(<CellInspector attribute="no2" label="NO2" access={undefined} failed="HTTP 502" onClose={() => undefined} />);
    expect(screen.getByRole("alert")).toHaveTextContent("Your access could not be read (HTTP 502). Reload the page to try again.");
  });

  it("is a side panel from 1024 px and a bottom sheet below", () => {
    expect(css).toMatch(/\.jc-grid-inspector \{[^}]*position: fixed;[^}]*right: 0;/);
    expect(css).toMatch(/@media \(max-width: 1023px\) \{\s*\.jc-grid-inspector \{[^}]*bottom: 0;[^}]*width: 100vw;/);
  });
});

describe("the grid's inspector", () => {
  it("opens no editor where the decisions refuse the write", async () => {
    render(<EntityGrid config={config()} source={source(async () => table)} />);
    await waitFor(() => expect(screen.getByDisplayValue("Kallio")).toBeInTheDocument());
    // name is written; no2 and pm10 are values, not inputs.
    expect(screen.queryByDisplayValue("21")).toBeNull();
    expect(screen.queryByDisplayValue("8")).toBeNull();
  });

  it("opens no editor at all until the decisions are read", async () => {
    render(<EntityGrid config={config()} source={source(() => new Promise(() => undefined))} />);
    await waitFor(() => expect(screen.getByText("Kallio")).toBeInTheDocument());
    expect(screen.queryByRole("textbox")).toBeNull();
  });

  it("opens from the cell's info button, and Escape gives the grid back", async () => {
    render(<EntityGrid config={config()} source={source(async () => table)} />);
    const cell = (await screen.findByText("21")).closest('[role="gridcell"]') as HTMLElement;
    fireEvent.click(cell.querySelector(".jc-grid-info") as HTMLElement);
    const panel = screen.getByRole("dialog", { name: `${L.title}: no2` });
    expect(within(panel).getByRole("status")).toHaveTextContent(L.prohibited);
    fireEvent.keyDown(panel, { key: "Escape" });
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(document.activeElement).toBe(screen.getByRole("grid"));
  });

  it("closes with the host's own word for close", async () => {
    render(<EntityGrid config={config()} source={source(async () => table)} labels={{ close: "Sulje" }} />);
    const cell = (await screen.findByText("21")).closest('[role="gridcell"]') as HTMLElement;
    fireEvent.click(cell.querySelector(".jc-grid-info") as HTMLElement);
    const panel = screen.getByRole("dialog", { name: `${L.title}: no2` });
    fireEvent.click(within(panel).getByRole("button", { name: "Sulje" }));
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("opens with i on the active cell and comes back to that cell", async () => {
    render(<EntityGrid config={config()} source={source(async () => table)} />);
    const cell = (await screen.findByText("8")).closest('[role="gridcell"]') as HTMLElement;
    fireEvent.mouseDown(cell);
    const grid = screen.getByRole("grid");
    grid.focus();
    fireEvent.keyDown(grid, { key: "i" });
    const panel = screen.getByRole("dialog", { name: `${L.title}: pm10` });
    expect(document.activeElement).toBe(within(panel).getByRole("heading"));
    fireEvent.keyDown(panel, { key: "Escape" });
    expect(document.activeElement).toBe(grid);
    expect(grid.getAttribute("aria-activedescendant")).toBe(cell.id);
  });

  it("opens from the cell's i button, editable cells included", async () => {
    render(<EntityGrid config={config()} source={source(async () => table)} />);
    await screen.findByDisplayValue("Kallio");
    fireEvent.click(screen.getAllByRole("button", { name: `${L.title}: name` })[0]);
    expect(within(screen.getByRole("dialog")).getByRole("status")).toHaveTextContent(L.grant);
  });

  it("opens the attribute's own view on a click of its value (owner 2026-11-01)", async () => {
    render(<EntityGrid config={config()} source={source(async () => table)} />);
    const cell = (await screen.findByText("21")).closest('[role="gridcell"]') as HTMLElement;
    fireEvent.click(cell);
    expect(await screen.findByRole("tab", { name: "no2", selected: true })).toBeInTheDocument();
    expect(screen.queryByRole("dialog", { name: `${L.title}: no2` })).toBeNull();
  });

  it("treats an attribute the decisions leave out as refused", async () => {
    render(<EntityGrid config={config()} source={source(async () => table.slice(0, 1))} />);
    const cell = (await screen.findByText("21")).closest('[role="gridcell"]') as HTMLElement;
    fireEvent.click(cell.querySelector(".jc-grid-info") as HTMLElement);
    expect(chips().every(([className]) => className?.endsWith("--none"))).toBe(true);
  });
});

describe("access/attributes", () => {
  it("is asked of the Endpoint with the person's session, never computed", async () => {
    const transport = vi.fn().mockResolvedValue({ status: 200, body: { type: "T", attributes: table } });
    const rows = await endpointSource("air-quality", transport).access!("AirQualityObserved");
    expect(transport).toHaveBeenCalledWith({ method: "GET", path: "/api/endpoint/air-quality/access/attributes?type=AirQualityObserved" });
    expect(rows).toEqual(table);
  });

  it("denies by default what an answer does not say", () => {
    expect(accessOf({ attributes: [{ name: "a", read: "yes", write: 1, reason: "made-up" }, { read: true }] })).toEqual([
      { name: "a", read: false, write: false, delete: false, reason: "no_grant" },
    ]);
    expect(() => accessOf({})).toThrow("not a list of attributes");
  });
});
