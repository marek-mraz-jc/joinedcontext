/**
 * How the grid reads values of every kind and data that is missing or partial (T-6304,
 * G-R2GRID2-017..020): relationship targets resolved once per page and opened in the same grid,
 * enum terms, each value kind, absent and null values, several instances, a wrong kind and a
 * target the person may not read.
 */
import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { EntityGrid } from "../src/grid/EntityGrid";
import { parseGridConfig } from "../src/grid/config";
import { fixtureSource } from "../src/grid/source";
import type { EntitySource } from "../src/grid/source";
import { toRichRow } from "../src/grid/model";
import { DEFAULT_LABELS } from "../src/grid/useEntityGrid";
import { enumOptions } from "../src/enums";

const gridOf = (type: string, columns: { attr: string; label: string }[]) =>
  parseGridConfig({ source: { kind: "fixture", name: "t" }, type, columns, pageSize: 50 }).config!;

const prop = (value: unknown, extra: Record<string, unknown> = {}) => ({ type: "Property", value, ...extra });

/** A fixture whose reads are counted, and which never answers the ids in `hidden`. */
function counted(entities: Record<string, unknown>[], hidden: string[] = []) {
  const fixture = fixtureSource(entities);
  const calls = { query: [] as { type: string; ids?: string[] }[], get: 0 };
  const source: EntitySource = {
    ...fixture,
    async query(q, page) {
      calls.query.push({ type: q.type, ids: q.ids });
      const answer = await fixture.query(q, page);
      return { ...answer, rows: answer.rows.filter((row) => !hidden.includes(row.id)) };
    },
    async get(id) {
      calls.get += 1;
      return fixture.get(id);
    },
  };
  return { source, calls };
}

const stations = Array.from({ length: 50 }, (_, i) => ({
  id: `urn:ngsi-ld:Station:s${i}`,
  type: "Station",
  name: prop(`Station ${i}`),
  owner: { type: "Relationship", object: `urn:ngsi-ld:Company:c${i % 10}` },
}));
const companies = Array.from({ length: 10 }, (_, i) => ({
  id: `urn:ngsi-ld:Company:c${i}`,
  type: "Company",
  name: prop(`Company ${i}`),
  revenue: prop(1000 + i),
}));

describe("relationship targets (G-R2GRID2-017)", () => {
  it("reads 10 targets of 50 rows in one query and no get, then names each by its name", async () => {
    const { source, calls } = counted([...stations, ...companies]);
    render(<EntityGrid config={gridOf("Station", [{ attr: "name", label: "Name" }, { attr: "owner", label: "Owner" }])} source={source} />);
    // 50 rows render before the one target read: a loaded CI runner passes the 1 s findBy default (ci 41368860961).
    await screen.findAllByRole("button", { name: `${DEFAULT_LABELS.openTarget}: Company · Company 3` }, { timeout: 5000 });
    const resolves = calls.query.filter((call) => call.ids);
    expect(resolves).toHaveLength(1);
    expect(resolves[0].type).toBe("Company");
    expect(new Set(resolves[0].ids)).toEqual(new Set(companies.map((c) => c.id)));
    expect(calls.get).toBe(0);
  }, 15000);

  it("opens a target in the same grid, and the breadcrumb brings back the first rows", async () => {
    const { source } = counted([...stations, ...companies]);
    const { container } = render(<EntityGrid config={gridOf("Station", [{ attr: "name", label: "Name" }, { attr: "owner", label: "Owner" }])} source={source} />);
    const chips = await screen.findAllByRole("button", { name: `${DEFAULT_LABELS.openTarget}: Company · Company 3` });
    fireEvent.click(chips[0]);
    // The target's own row and its values, read by its id from the same source.
    const trail = await screen.findByRole("navigation", { name: DEFAULT_LABELS.drillTrail });
    expect(within(trail).getByText("Company · c3")).toHaveAttribute("aria-current", "page");
    expect(await screen.findByText(/^1\D?003$/)).toBeInTheDocument();
    const shown = () => [...container.querySelectorAll(".jc-grid")].filter((grid) => !grid.closest("[hidden]"));
    expect(shown()).toHaveLength(1);
    expect(within(shown()[0] as HTMLElement).queryByText("Station 3")).toBeNull();
    // Only the opened target: no other company of the type.
    expect(within(shown()[0] as HTMLElement).queryByText("Company 4")).toBeNull();
    fireEvent.click(within(trail).getByRole("button", { name: `← ${DEFAULT_LABELS.drillBack}` }));
    await waitFor(() => expect(screen.queryByRole("navigation", { name: DEFAULT_LABELS.drillTrail })).toBeNull());
    expect(within(shown()[0] as HTMLElement).getByText("Station 3")).toBeInTheDocument();
  });

  it("shows a target the person may not read as its id alone: no link, none of its values", async () => {
    const secret = { id: "urn:ngsi-ld:Company:hidden", type: "Company", name: prop("TOPSECRET Ltd"), revenue: prop(424242) };
    const row = { id: "urn:ngsi-ld:Station:x", type: "Station", name: prop("X"), owner: { type: "Relationship", object: secret.id } };
    const { source, calls } = counted([row, secret], [secret.id]);
    const { container } = render(<EntityGrid config={gridOf("Station", [{ attr: "name", label: "Name" }, { attr: "owner", label: "Owner" }])} source={source} />);
    await screen.findByText("X");
    await waitFor(() => expect(calls.query.some((call) => call.ids)).toBe(true));
    const chip = await screen.findByTitle(secret.id);
    expect(chip.tagName).toBe("SPAN");
    expect(chip).toHaveTextContent("hidden");
    expect(screen.queryByRole("button", { name: /Company · hidden/ })).toBeNull();
    expect(container.textContent).not.toContain("TOPSECRET");
    expect(container.textContent).not.toContain("424242");
  });

  it("reads a ListRelationship as one chip per target", () => {
    const row = toRichRow({
      id: "urn:ngsi-ld:Station:l",
      type: "Station",
      members: { type: "ListRelationship", objectList: [{ object: "urn:ngsi-ld:Company:c1" }, { object: "urn:ngsi-ld:Company:c2" }] },
    });
    expect(row.cells.members).toMatchObject({ kind: "relationship", object: ["urn:ngsi-ld:Company:c1", "urn:ngsi-ld:Company:c2"] });
  });
});

describe("values by kind (G-R2GRID2-018, -019)", () => {
  const entity = {
    id: "urn:ngsi-ld:Place:p",
    type: "Place",
    name: { type: "LanguageProperty", languageMap: { fi: "Kauppatori" } },
    data: { type: "JsonProperty", json: { note: "<script>alert(1)</script>", deep: { a: { b: { c: { d: 1 } } } } } },
    category: { type: "VocabProperty", vocab: "https://vocab.example/terms#market" },
    status: prop("open"),
  };
  const columns = [
    { attr: "name", label: "Name" },
    { attr: "data", label: "Data" },
    { attr: "category", label: "Category" },
    { attr: "status", label: "Status" },
  ];

  it("says the language a text falls back to, and renders a JsonProperty as text", async () => {
    const { container } = render(<EntityGrid config={gridOf("Place", columns)} source={fixtureSource([entity])} locale="en" />);
    const lang = await screen.findByText("fi", { selector: ".jc-cell-lang" });
    expect(lang.previousElementSibling).toHaveAttribute("lang", "fi");
    expect(container.querySelector("script")).toBeNull();
    expect(container.textContent).toContain("<script>alert(1)</script>");
    // Deep values end in "…" in the cell; the row detail holds the whole value.
    expect(container.textContent).toContain("…");
  });

  it("links a vocabulary IRI, and an enum value to its term through termHref", async () => {
    const options = enumOptions({ enum: ["open", "closed"], "x-enum-titles": { open: "Open now" }, "x-enum-meanings": { open: "https://schema.example/Open" } })!;
    expect(options[0].meaning).toBe("https://schema.example/Open");
    const termHref = vi.fn((meaning: string) => `${meaning}?from=grid`);
    render(<EntityGrid config={gridOf("Place", columns)} source={fixtureSource([entity])} enums={{ status: options }} termHref={termHref} />);
    const term = await screen.findByRole("link", { name: "Open now" });
    expect(term).toHaveAttribute("href", "https://schema.example/Open?from=grid");
    expect(screen.getByRole("link", { name: "https://vocab.example/terms#market" })).toHaveAttribute("rel", "noopener noreferrer");
  });

  it("never links a term address that is not http(s)", async () => {
    const options = [{ value: "open", title: "Open now", meaning: "x:Open" }];
    render(<EntityGrid config={gridOf("Place", columns)} source={fixtureSource([entity])} enums={{ status: options }} termHref={() => "javascript:alert(1)"} />);
    await screen.findByText("Open now");
    expect(screen.queryByRole("link", { name: "Open now" })).toBeNull();
  });

  it("shows a value outside the enum raw, said so, with no link", async () => {
    const zzz = { ...entity, status: prop("zzz") };
    render(<EntityGrid config={gridOf("Place", columns)} source={fixtureSource([zzz])} enums={{ status: [{ value: "open", title: "Open now", meaning: "https://schema.example/Open" }] }} termHref={(m) => m} />);
    expect(await screen.findByText(`zzz (${DEFAULT_LABELS.notInList})`)).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: /zzz/ })).toBeNull();
  });
});

describe("missing and partial data (G-R2GRID2-020)", () => {
  const columns = [
    { attr: "name", label: "Name" },
    { attr: "speed", label: "Speed" },
    { attr: "level", label: "Level" },
  ];
  const openDetail = async (name: string) => {
    fireEvent.click(await screen.findByRole("button", { name: `${DEFAULT_LABELS.openRow}: ${name}` }));
    return screen.findByRole("dialog");
  };

  it("reads an absent and a null value the same: empty, heard as not set", async () => {
    const rows = [
      { id: "urn:ngsi-ld:Road:a", type: "Road", name: prop("Absent") },
      { id: "urn:ngsi-ld:Road:b", type: "Road", name: prop("Null"), speed: prop(null) },
      { id: "urn:ngsi-ld:Road:c", type: "Road", name: prop("NgsiNull"), speed: prop("urn:ngsi-ld:null") },
    ];
    expect(toRichRow(rows[1]).cells.speed).toBeUndefined();
    expect(toRichRow(rows[2]).cells.speed).toBeUndefined();
    render(<EntityGrid config={gridOf("Road", columns)} source={fixtureSource(rows)} />);
    await screen.findByText("NgsiNull");
    // Three rows, each with Speed and Level not set: the column of a model attribute no row holds stays.
    expect(screen.getAllByText(`Speed: ${DEFAULT_LABELS.rowNotSet}`)).toHaveLength(3);
    expect(screen.getAllByText(`Level: ${DEFAULT_LABELS.rowNotSet}`)).toHaveLength(3);
    expect(screen.queryByText("null")).toBeNull();
    const detail = await openDetail("Null");
    expect(within(detail).getAllByText(DEFAULT_LABELS.rowNotSet).length).toBeGreaterThan(0);
  });

  it("shows the default instance of three with +2, and lists all three in the detail", async () => {
    const row = {
      id: "urn:ngsi-ld:Road:i",
      type: "Road",
      name: prop("Instances"),
      speed: [prop(30, { datasetId: "urn:ngsi-ld:Dataset:radar" }), prop(40), prop(50, { datasetId: "urn:ngsi-ld:Dataset:camera" })],
    };
    render(<EntityGrid config={gridOf("Road", columns)} source={fixtureSource([row])} />);
    const more = await screen.findByText("+2");
    expect(more).toHaveAttribute("title", "2 more instances: 30 (radar), 50 (camera)");
    expect(more.previousElementSibling).toHaveTextContent("40");
    const detail = await openDetail("Instances");
    const items = within(detail).getAllByRole("listitem").map((item) => item.textContent);
    expect(within(detail).getByTitle("urn:ngsi-ld:Dataset:radar")).toHaveTextContent("radar");
    expect(items).toEqual([
      expect.stringContaining("radar"),
      expect.stringContaining(DEFAULT_LABELS.defaultInstance),
      expect.stringContaining("camera"),
    ]);
  });

  it("shows a value of the wrong kind raw with its reason, never coerced", async () => {
    const row = { id: "urn:ngsi-ld:Road:w", type: "Road", name: prop("Wrong"), speed: prop("n/a") };
    const { container } = render(<EntityGrid config={gridOf("Road", columns)} source={fixtureSource([row])} rules={{ speed: { kind: "number" } }} />);
    const reason = DEFAULT_LABELS.wrongKind.replace("{kind}", DEFAULT_LABELS.kindNumber);
    const cell = await screen.findByTitle(reason);
    // Seen, not only heard or hovered: the reason is the cell's own text, as "(not in the list)" is.
    expect(cell).toHaveTextContent(`n/a (${reason})`);
    expect(cell).toHaveClass("jc-cell--invalid");
    expect(container.querySelector(".jc-cell--number")).toBeNull();
  });

  it("puts an attribute the config does not list after the listed ones, by its own name", async () => {
    const row = { id: "urn:ngsi-ld:Road:e", type: "Road", name: prop("Extra"), speed: prop(5), extraNote: prop("x") };
    const config = parseGridConfig({ source: { kind: "fixture", name: "t" }, type: "Road", pageSize: 50 }).config!;
    render(<EntityGrid config={config} source={fixtureSource([row])} />);
    await screen.findByText("Extra");
    const headers = screen.getAllByRole("columnheader").map((header) => header.textContent ?? "");
    const at = (text: string) => headers.findIndex((header) => header.includes(text));
    expect(headers.slice(0, 3)).toEqual(["ID", "Type", expect.stringContaining("name")]);
    expect(at("speed")).toBeGreaterThan(2);
    expect(at("extraNote")).toBeGreaterThan(at("speed"));
  });
});
