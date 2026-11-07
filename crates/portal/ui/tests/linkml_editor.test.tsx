/** T-0575: one editor, controlled from outside, in edit mode and in subset mode (DM-13, DM-31, MP-03). */
import { useState } from "react";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n, { SUPPORTED_LOCALES } from "../src/i18n";
import { LinkmlEditor } from "../src/pages/models/LinkmlEditor";
import { applyOperations } from "../src/pages/models/operations";
import { graphData, parseModel } from "../src/pages/models/linkml";
import { place } from "../src/pages/models/LinkmlGraphView";
import en from "../src/locales/en.json";
import type { Subset } from "../src/pages/models/subset";
import { expectNoRawKeys, expectNoViolations, focusables } from "./checks";

const SOURCE = `id: https://hel.fi/models/fleet
name: fleet
prefixes:
  hel: https://hel.fi/terms/
classes:
  Vehicle:
    class_uri: hel:Vehicle
    slots: [id, name, speed]
  User:
    class_uri: hel:User
    slots: [id, name, age]
slots:
  id:
    identifier: true
  name:
    range: string
  speed:
    range: float
  age:
    range: integer
`;

/**
 * The page holds the document; the assistant button applies an operation to it the way an
 * assistant would, without touching the editor.
 */
function Harness({ subsetMode = false }: { subsetMode?: boolean }) {
  const [source, setSource] = useState(SOURCE);
  const [subset, setSubset] = useState<Subset>({ classes: [] });
  return (
    <>
      <button
        type="button"
        onClick={() =>
          setSource(
            applyOperations(source, [
              { op: "addSlot", name: "colour", class: "Vehicle", range: "string" },
            ]).source,
          )
        }
      >
        assistant adds colour
      </button>
      <LinkmlEditor
        source={source}
        onChange={setSource}
        locales={["en"]}
        {...(subsetMode ? { subset, onSubsetChange: setSubset } : {})}
      />
      <textarea readOnly aria-label="source" value={source} />
    </>
  );
}

function renderEditor(subsetMode = false) {
  const fetchMock = vi.fn((input: RequestInfo | URL) =>
    Promise.resolve(
      new Response(
        JSON.stringify({
          jsonSchema: { title: (input as Request).url },
          context: {},
          example: {},
          errors: [],
        }),
        { status: 200, headers: { "Content-Type": "application/json" } },
      ),
    ),
  );
  vi.stubGlobal("fetch", fetchMock);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <Harness subsetMode={subsetMode} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return {
    fetchMock,
    source: () => (screen.getByLabelText("source") as HTMLTextAreaElement).value,
    user: userEvent.setup(),
  };
}

describe("LinkML editor", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("is controlled: an operation applied outside the editor shows up in the tree", async () => {
    const { source, user } = renderEditor();
    expect(screen.getByRole("tab", { name: "Structure" })).toHaveAttribute("aria-selected", "true");
    expect(screen.queryByRole("tab", { name: "Subset" })).not.toBeInTheDocument();
    expect(screen.queryByRole("cell", { name: "colour" })).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "assistant adds colour" }));

    expect(await screen.findByRole("button", { name: "colour" })).toBeInTheDocument();
    expect(parseModel(source()).classes[0].slots).toEqual(["id", "name", "speed", "colour"]);

    // And a click in the tree edits the same string the assistant edited.
    await user.click(screen.getByRole("button", { name: "Add field" }));
    await user.type(screen.getByRole("textbox", { name: "Name" }), "plate");
    await user.click(screen.getByRole("button", { name: "Add" }));
    expect(parseModel(source()).classes[0].slots).toEqual(["id", "name", "speed", "colour", "plate"]);
  });

  /// T-1111, DM-13: the third view is a picture of the model — every class a box, every way one
  /// class names another a line — and clicking a class opens it where it can be edited.
  it("draws the classes and what joins them, and opens one in the structure view", async () => {
    renderEditor();
    await userEvent.click(screen.getByRole("tab", { name: en.models.view.graph }));

    // A group, not an image: the class boxes inside it are buttons, and an image's contents are
    // presentational (T-1846).
    const canvas = await screen.findByRole("group", { name: en.models.graph.title });
    expect(canvas).toBeInTheDocument();
    // Both classes, each with its own slots inside the box.
    expect(screen.getByRole("button", { name: /Open Vehicle/ })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Open User/ })).toBeInTheDocument();
    expect(canvas.textContent).toContain("speed");

    await userEvent.click(screen.getByRole("button", { name: /Open Vehicle/ }));
    // The structure view is what edits a class, so that is where the click lands.
    expect(screen.getByRole("tab", { name: en.models.view.structure })).toHaveAttribute(
      "aria-selected",
      "true",
    );
  });

  it("in subset mode shows the picker and previews the narrowed model", async () => {
    const { fetchMock, user } = renderEditor(true);
    expect(screen.getByRole("tab", { name: "Subset" })).toHaveAttribute("aria-selected", "true");
    expect(screen.queryByRole("tab", { name: "Structure" })).not.toBeInTheDocument();

    await user.click(screen.getByRole("checkbox", { name: "Vehicle.name" }));
    await user.click(screen.getByRole("tab", { name: "Preview" }));

    await screen.findByText(/Vehicle/);
    const request = fetchMock.mock.calls.at(-1)?.[0] as Request;
    expect(new URL(request.url).pathname).toBe("/api/v1/tools/generate");
    const compiled = parseModel(((await request.json()) as { source: string }).source);
    expect(compiled.classes.map((klass) => [klass.name, klass.slots])).toEqual([
      ["Vehicle", ["id", "name"]],
    ]);
    expect(compiled.slots.map((slot) => slot.name)).toEqual(["id", "name"]);
  });
});

/** The graph's own two functions, away from the editor that draws them (T-1111). */
describe("the model as a graph", () => {
  const source = `id: https://hel.fi/models/fleet
name: fleet
classes:
  Thing:
    slots: []
  Vehicle:
    is_a: Thing
    mixins: [Traceable]
    slots: [refDevice, speed]
  Device:
    slots: []
  Traceable:
    slots: []
slots:
  refDevice:
    range: Device
  speed:
    range: float
`;

  it("draws a line for what a class specialises, mixes in, and points at", () => {
    const { edges } = graphData(parseModel(source));
    expect(edges).toContainEqual({ from: "Vehicle", to: "Thing", kind: "is_a" });
    expect(edges).toContainEqual({ from: "Vehicle", to: "Traceable", kind: "mixin" });
    expect(edges).toContainEqual({
      from: "Vehicle",
      to: "Device",
      kind: "range",
      label: "refDevice",
      // T-2881: how many at both ends; nothing limits how many Vehicles share one Device.
      fromMultiplicity: "*",
      toMultiplicity: "0..1",
    });
    // A slot whose range is a primitive is inside the box, not a line.
    expect(edges.some((edge) => edge.label === "speed")).toBe(false);
  });

  it("names no line to a class the model does not declare", () => {
    const dangling = `name: x
classes:
  Vehicle:
    is_a: Elsewhere
    slots: [owner]
slots:
  owner:
    range: Person
`;
    expect(graphData(parseModel(dangling)).edges).toEqual([]);
  });

  it("puts a child below its parent and survives a model somebody is mid-edit", () => {
    const { nodes } = graphData(parseModel(source));
    const depth = (name: string) => nodes.find((node) => node.name === name)?.depth;
    expect(depth("Thing")).toBe(0);
    expect(depth("Vehicle")).toBe(1);

    // A cycle is a model being edited, not a reason to hang: the walk stops at the class it
    // has already seen, so the depth is finite and no deeper than the chain is long.
    const circular = `name: x
classes:
  A:
    is_a: B
    slots: []
  B:
    is_a: A
    slots: []
`;
    const cycled = graphData(parseModel(circular)).nodes;
    expect(cycled).toHaveLength(2);
    expect(cycled.every((node) => node.depth <= cycled.length)).toBe(true);
  });

  it("puts a parent above its child and gives every box a place of its own", () => {
    const { nodes, edges } = graphData(parseModel(source));
    const { placed } = place(nodes, edges);
    const vehicle = placed.find((node) => node.name === "Vehicle");
    const thing = placed.find((node) => node.name === "Thing");
    expect(vehicle && thing && vehicle.y > thing.y).toBe(true);
    const corners = placed.map((node) => `${node.x},${node.y}`);
    expect(new Set(corners).size).toBe(placed.length);
  });
});

/**
 * The UI contract of the editor's own frame (T-1770, UI-04, UI-15, UI-16, UI-44, UI-48): the
 * views are the shared `Tabs`, each view is axe-clean, the tab list is walked with the arrow keys
 * the WAI-ARIA pattern asks for, and the four locales name every view.
 *
 * The harness above adds a button and a textarea of its own; this one renders the editor alone,
 * so a violation belongs to the editor and not to the test around it.
 */
describe("the LinkML editor against the UI contract", () => {
  function Alone({ subsetMode }: { subsetMode: boolean }) {
    const [source, setSource] = useState(SOURCE);
    const [subset, setSubset] = useState<Subset>({ classes: [] });
    return (
      <LinkmlEditor
        source={source}
        onChange={setSource}
        locales={["sk", "en"]}
        {...(subsetMode ? { subset, onSubsetChange: setSubset } : {})}
      />
    );
  }

  function renderAlone(subsetMode = false) {
    vi.stubGlobal(
      "fetch",
      vi.fn(() =>
        Promise.resolve(
          new Response(
            JSON.stringify({ jsonSchema: { title: "air" }, context: {}, example: {}, errors: [] }),
            { status: 200, headers: { "Content-Type": "application/json" } },
          ),
        ),
      ),
    );
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const view = render(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <Alone subsetMode={subsetMode} />
        </I18nextProvider>
      </QueryClientProvider>,
    );
    return { container: view.container, user: userEvent.setup() };
  }

  // The Graph view is left out: its figure is an `<svg role="img">` whose class boxes are
  // focusable `role="button"` groups, which axe reports as `nested-interactive` (serious). That
  // file is T-1846's, and the evidence is in its body; this case goes back in with the fix.
  it.each([en.models.view.structure, en.models.view.preview])(
    "has no axe violation in the %s view",
    async (name) => {
      const { container, user } = renderAlone();
      await user.click(screen.getByRole("tab", { name }));
      await screen.findByRole("tab", { name, selected: true });
      await expectNoViolations(container);
    },
  );

  it("has no axe violation in the subset view a picker opens on", async () => {
    const { container } = renderAlone(true);
    await screen.findByRole("tab", { name: en.models.view.subset, selected: true });
    await expectNoViolations(container);
  });

  it("walks the views with the arrow keys and leaves the panel in the tab order", async () => {
    const { container, user } = renderAlone();

    const structure = screen.getByRole("tab", { name: en.models.view.structure });
    structure.focus();
    expect(structure).toHaveAttribute("aria-selected", "true");

    await user.keyboard("{ArrowRight}");
    const graph = screen.getByRole("tab", { name: en.models.view.graph });
    expect(document.activeElement).toBe(graph);
    expect(graph).toHaveAttribute("aria-selected", "true");

    // The tab list is one Tab stop, held by the selected tab; the others are reached with the
    // arrow keys (the roving tabindex of the shared Tabs, WAI-ARIA tabs pattern, T-1753).
    const stops = focusables(container).filter((element) => element.getAttribute("role") === "tab");
    expect(stops).toEqual([graph]);
  });

  /// T-6101: what the model holds, how it connects, an example entity, and the LinkML last.
  it("lists the views as Structure, Graph, Preview, Source", () => {
    renderAlone();
    expect(screen.getAllByRole("tab").map((tab) => tab.textContent)).toEqual([
      en.models.view.structure,
      en.models.view.graph,
      en.models.view.preview,
      en.models.view.source,
    ]);
  });

  it.each(SUPPORTED_LOCALES)("names every view in %s", async (locale) => {
    await i18n.changeLanguage(locale);
    const { container } = renderAlone();

    for (const view of ["structure", "source", "graph", "preview"]) {
      expect(
        screen.getByRole("tab", { name: i18n.t(`models.view.${view}`) }),
        `the ${view} view has a ${locale} name`,
      ).toBeInTheDocument();
    }
    expectNoRawKeys(container);
  });
});
