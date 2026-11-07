/**
 * T-3589: reading a big model diagram — find a class, focus on it and the boxes a few lines out
 * (the rest faded), light the shortest path to another class, fold an import into one box, walk
 * the boxes with the arrow keys, and see the whole drawing small in a minimap.
 */
import { act, cleanup, fireEvent, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { foldImports, importBox, neighbourhood, nextBox, shortestPath } from "../src/pages/models/diagramReading";
import { LinkmlGraphView } from "../src/pages/models/LinkmlGraphView";
import type { GraphEdge, GraphNode } from "../src/pages/models/linkml";
import { expectNoRawKeys } from "./checks";
import { expectNoAxeViolations, inEveryLocale, renderPart } from "./page_contract";

/** A chain Region → District → Street → Building, a Lamp on the Street, and a Park joined to nothing. */
const CITY = `name: city
classes:
  Region:
    slots: [districts]
  District:
    slots: [streets]
  Street:
    slots: [buildings, lamps]
  Building:
    slots: [floors]
  Lamp:
    slots: [height]
  Park:
    slots: [area]
slots:
  districts: { range: District, multivalued: true }
  streets: { range: Street, multivalued: true }
  buildings: { range: Building, multivalued: true }
  lamps: { range: Lamp, multivalued: true }
  floors: { range: integer }
  height: { range: float }
  area: { range: float }
`;

const BASE = `name: base
classes:
  Agent:
    slots: [label]
  Organisation:
    is_a: Agent
    slots: []
slots:
  label: { range: string }
`;

const WITH_IMPORT = `name: city
imports: [base]
classes:
  Office:
    slots: [operator]
slots:
  operator: { range: Organisation }
`;

const edge = (from: string, to: string, kind: GraphEdge["kind"] = "range", label?: string): GraphEdge => ({
  from,
  to,
  kind,
  ...(label ? { label } : {}),
});
const node = (name: string, from?: string): GraphNode => ({ name, kind: "class", slots: [], depth: 0, ...(from ? { from } : {}) });

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(() => {
  cleanup();
});

describe("the reading helpers", () => {
  const names = ["A", "B", "C", "D", "E"];
  const chain = [edge("A", "B"), edge("C", "B"), edge("C", "D"), edge("D", "D")];

  it("reaches as many lines out as asked, either direction, and nothing from a class not drawn", () => {
    expect([...neighbourhood(names, chain, "A", 1)].sort()).toEqual(["A", "B"]);
    expect([...neighbourhood(names, chain, "A", 2)].sort()).toEqual(["A", "B", "C"]);
    expect([...neighbourhood(names, chain, "E", 3)]).toEqual(["E"]);
    expect(neighbourhood(names, chain, "Gone", 1).size).toBe(0);
  });

  it("finds the fewest lines between two classes, and says so when none joins them", () => {
    expect(shortestPath(names, chain, "A", "D")).toEqual({ nodes: ["A", "B", "C", "D"], edges: [0, 1, 2] });
    expect(shortestPath(names, chain, "A", "E")).toEqual({ nodes: [], edges: [] });
    expect(shortestPath(names, chain, "A", "A")).toEqual({ nodes: ["A"], edges: [] });
    expect(shortestPath(names, chain, "A", "Gone")).toEqual({ nodes: [], edges: [] });
  });

  it("folds an import into one box, its lines moved there, inner and repeated lines dropped", () => {
    const nodes = [node("Office"), node("Agent", "base"), node("Organisation", "base")];
    const edges = [
      edge("Organisation", "Agent", "is_a"),
      edge("Office", "Organisation", "range", "operator"),
      edge("Office", "Agent", "range", "operator"),
    ];
    const folded = foldImports(nodes, edges, new Set(["base"]));
    expect(folded.nodes.map((n) => [n.name, n.kind, n.slots])).toEqual([
      ["Office", "class", []],
      [importBox("base"), "import", ["Agent", "Organisation"]],
    ]);
    expect(folded.edges.map((e) => `${e.from}→${e.to}`)).toEqual([`Office→${importBox("base")}`]);
    expect(foldImports(nodes, edges, new Set())).toEqual({ nodes, edges });
  });

  it("goes to the nearest box the arrow points at, along before across", () => {
    const box = (name: string, x: number, y: number) => ({ name, x, y, width: 10, height: 10 });
    const boxes = [box("Here", 100, 100), box("Below", 100, 200), box("FarDiagonal", 300, 160), box("Left", 0, 100)];
    expect(nextBox(boxes, "Here", "ArrowDown")?.name).toBe("Below");
    expect(nextBox(boxes, "Here", "ArrowLeft")?.name).toBe("Left");
    expect(nextBox(boxes, "Here", "ArrowRight")?.name).toBe("FarDiagonal");
    expect(nextBox(boxes, "Here", "ArrowUp")).toBeUndefined();
    expect(nextBox(boxes, "Gone", "ArrowUp")).toBeUndefined();
  });
});

/** The class boxes, by name, and whether each is drawn faded. */
function fadedBoxes(): Record<string, boolean> {
  const out: Record<string, boolean> = {};
  for (const box of screen.getAllByRole("button", { name: /^Open .* in the structure view$/ })) {
    const name = /^Open (.*) in the structure view$/.exec(box.getAttribute("aria-label") ?? "")?.[1] ?? "";
    out[name] = box.getAttribute("opacity") !== null;
  }
  return out;
}

describe("reading the drawing", () => {
  it("finds a class, focuses on it and fades what lies further than the lines asked for", async () => {
    const user = userEvent.setup();
    renderPart(<LinkmlGraphView source={CITY} />);

    await user.type(screen.getByRole("combobox", { name: en.models.graph.find }), "stre{Enter}");
    expect(screen.getByRole("status")).toHaveTextContent("Street and 3 boxes joined to it; the rest is faded.");
    expect(fadedBoxes()).toEqual({ Region: true, District: false, Street: false, Building: false, Lamp: false, Park: true });

    await user.selectOptions(screen.getByRole("combobox", { name: en.models.graph.hops }), "2");
    expect(fadedBoxes()).toMatchObject({ Region: false, Park: true });

    await user.click(screen.getByRole("button", { name: en.models.graph.showAll }));
    expect(Object.values(fadedBoxes()).some(Boolean)).toBe(false);
    expect(screen.getByRole("status")).toHaveTextContent("");
  });

  it("focuses a class picked from the list without a key press", () => {
    renderPart(<LinkmlGraphView source={CITY} />);
    fireEvent.change(screen.getByRole("combobox", { name: en.models.graph.find }), { target: { value: "Lamp" } });
    expect(screen.getByRole("status")).toHaveTextContent("Lamp and 1 box joined to it");
  });

  it("lights the shortest path to another class, and says when none joins them", async () => {
    const user = userEvent.setup();
    const { container } = renderPart(<LinkmlGraphView source={CITY} />);
    await user.type(screen.getByRole("combobox", { name: en.models.graph.find }), "Region{Enter}");

    await user.selectOptions(screen.getByRole("combobox", { name: en.models.graph.pathTo }), "Lamp");
    expect(screen.getByRole("status")).toHaveTextContent("3 lines: Region → District → Street → Lamp");
    expect(fadedBoxes()).toEqual({ Region: false, District: false, Street: false, Building: true, Lamp: false, Park: true });
    const bold = [...container.querySelectorAll("g.text-fg-subtle > path")].filter((p) => p.getAttribute("stroke-width") === "2.5");
    expect(bold).toHaveLength(3);

    await user.selectOptions(screen.getByRole("combobox", { name: en.models.graph.pathTo }), "Park");
    expect(screen.getByRole("status")).toHaveTextContent("No line joins Region and Park.");
  });

  it("folds an import into one box and unfolds it from the box", async () => {
    const user = userEvent.setup();
    renderPart(<LinkmlGraphView source={WITH_IMPORT} imports={{ base: BASE }} />);
    expect(screen.getByRole("button", { name: en.models.graph.openClass.replace("{name}", "Agent") })).toBeInTheDocument();

    const fold = screen.getByRole("button", { name: en.models.graph.foldImport.replace("{name}", "base") });
    await user.click(fold);
    expect(fold).toHaveAttribute("aria-pressed", "true");
    expect(screen.queryByRole("button", { name: en.models.graph.openClass.replace("{name}", "Agent") })).toBeNull();
    const box = screen.getByRole("button", { name: "Unfold base: Agent, Organisation" });

    await user.click(box);
    expect(fold).toHaveAttribute("aria-pressed", "false");
    expect(screen.getByRole("button", { name: en.models.graph.openClass.replace("{name}", "Agent") })).toBeInTheDocument();
  });

  it("walks the boxes with the arrow keys", async () => {
    const user = userEvent.setup();
    renderPart(<LinkmlGraphView source={CITY} />);
    const open = (name: string) => screen.getByRole("button", { name: en.models.graph.openClass.replace("{name}", name) });
    act(() => open("Region").focus());
    await user.keyboard("{ArrowDown}");
    expect(open("District")).toHaveFocus();
    await user.keyboard("{ArrowDown}");
    expect(open("Street")).toHaveFocus();
    await user.keyboard("{ArrowUp}");
    expect(open("District")).toHaveFocus();
  });

  it("shows every box in the minimap, faded as in the drawing", async () => {
    const user = userEvent.setup();
    renderPart(<LinkmlGraphView source={CITY} />);
    const minimap = screen.getByTestId("diagram-minimap");
    expect(minimap).toHaveAttribute("aria-hidden", "true");
    expect(minimap.querySelectorAll("rect")).toHaveLength(6);
    await user.type(screen.getByRole("combobox", { name: en.models.graph.find }), "Park{Enter}");
    const faint = [...minimap.querySelectorAll("rect")].filter((rect) => rect.getAttribute("opacity") === "0.45");
    expect(faint).toHaveLength(5);
    // A press on the minimap scrolls the frame; jsdom has no layout, so it only must not throw.
    await user.click(minimap);
  });

  it("is axe-clean and translated with a focus on, in every locale", async () => {
    await inEveryLocale(async () => {
      const { container, unmount } = renderPart(<LinkmlGraphView source={CITY} />);
      fireEvent.change(screen.getByRole("combobox", { name: i18n.t("models.graph.find") }), { target: { value: "Street" } });
      expectNoRawKeys(container);
      expect(screen.getByRole("status").textContent).not.toBe("");
      unmount();
    });
    const { container } = renderPart(<LinkmlGraphView source={CITY} />);
    fireEvent.change(screen.getByRole("combobox", { name: en.models.graph.find }), { target: { value: "Street" } });
    await expectNoAxeViolations(container);
  });
});
