/**
 * T-6094: the model as a full-screen canvas. The overlay maps an Endpoint's bound Policies to the
 * classes and attributes it serves, the layout is the same for the same model, and the page
 * shows a picked class with its fields, Endpoints and Pipelines, behind propose rights for Edit.
 */
import { useState } from "react";
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import en from "../src/locales/en.json";
import type { Manifest } from "../src/api/manifest";
import {
  MAIN_AREA,
  PERSONAL_AREA,
  areaTabs,
  areasOf,
  LAYOUT_STORAGE,
  readPositions,
  servedBy,
  servingCounts,
  writePositions,
  writtenBy,
} from "../src/pages/models/canvasOverlay";
import { fitRow, LinkmlGraphView, place } from "../src/pages/models/LinkmlGraphView";
import { fieldsOf, layoutKey } from "../src/pages/models/ModelCanvas";
import { graphData, parseModel } from "../src/pages/models/linkml";
import { jsonResponse, list, renderRoute } from "./pageHarness";

const manifest = (kind: string, name: string, spec: Record<string, unknown>, extra: Record<string, unknown> = {}) =>
  ({ apiVersion: "joinedcontext.com/v1alpha1", kind, metadata: { name, namespace: "helsinki" }, spec, ...extra }) as Manifest;

const BIKES = `id: https://hel.fi/models/bikes
name: bikes
default_range: string
classes:
  Place:
    slots: [name]
  BikeHireDockingStation:
    is_a: Place
    slots: [capacity, freeBikes, contactPoint, operator]
  Operator:
    slots: [name]
  Rider:
    in_subset: [accounts]
    slots: [email, rides]
slots:
  name: { range: string, required: true, description: The station's name }
  capacity: { range: integer }
  freeBikes: { range: integer, multivalued: true }
  contactPoint: { range: string }
  operator: { range: Operator }
  email: { range: string, annotations: { personalData: true } }
  rides: { range: integer }
`;

const SPACES = [manifest("ContextSpace", "helsinki", {})];
const POLICIES = [
  manifest("Policy", "public-bikes", {
    contextSpaceRef: { kind: "ContextSpace", name: "helsinki" },
    assignee: { kind: "role", id: "public" },
    information: [{ entities: [{ type: "BikeHireDockingStation" }] }],
  }),
  manifest("Policy", "ops", {
    contextSpaceRef: "helsinki",
    rules: [{ assignee: { kind: "role", id: "ops" }, information: [{ entities: [{ type: "Operator" }, { type: "Nope" }], propertyNames: ["name"] }] }],
  }),
  manifest("Policy", "elsewhere", { contextSpaceRef: "other", information: [{ entities: [{ type: "Rider" }] }] }),
];
const ENDPOINTS = [
  manifest("Endpoint", "helsinki-bikes", {
    contextSpaceRef: "helsinki",
    slug: "a",
    policyRef: "urn:ngsi-ld:Policy:hel.fi:helsinki-helsinki:public-bikes",
    projection: { hiddenAttributes: ["contactPoint"] },
  }),
  manifest("Endpoint", "helsinki-ops", { contextSpaceRef: "helsinki", slug: "b", policyRef: "urn:ngsi-ld:Policy:hel.fi:helsinki-helsinki:ops" }),
  manifest("Endpoint", "helsinki-all", { contextSpaceRef: "helsinki", slug: "c" }),
  manifest("Endpoint", "helsinki-broken", { contextSpaceRef: "helsinki", slug: "d", policyRef: "urn:ngsi-ld:Policy:hel.fi:helsinki-helsinki:gone" }),
];
const PIPELINES = [
  manifest("Pipeline", "gbfs", { output: { type: "BikeHireDockingStation" } }),
  manifest("Pipeline", "other", { output: { type: "Elsewhere" } }),
];

const graph = graphData(parseModel(BIKES));

describe("the Endpoints overlay", () => {
  it("serves what the bound Policy names, less the hidden attributes", () => {
    const served = servedBy(ENDPOINTS[0], "helsinki", POLICIES, SPACES, graph.nodes);
    expect([...served.keys()]).toEqual(["BikeHireDockingStation"]);
    expect([...(served.get("BikeHireDockingStation") ?? [])].sort()).toEqual(["capacity", "freeBikes", "id", "operator"]);
  });

  it("keeps a rule's attribute list and the key, and drops a type the model lacks", () => {
    const served = servedBy(ENDPOINTS[1], "helsinki", POLICIES, SPACES, graph.nodes);
    expect([...served.keys()]).toEqual(["Operator"]);
    expect([...(served.get("Operator") ?? [])]).toEqual(["id", "name"]);
  });

  it("serves every Policy of its own space when it names none, and nothing when its reference binds none", () => {
    const all = servedBy(ENDPOINTS[2], "helsinki", POLICIES, SPACES, graph.nodes);
    expect([...all.keys()].sort()).toEqual(["BikeHireDockingStation", "Operator"]);
    expect(servedBy(ENDPOINTS[3], "helsinki", POLICIES, SPACES, graph.nodes).size).toBe(0);
  });

  it("counts the Endpoints per class and lists the Pipelines that write one", () => {
    const counts = servingCounts(ENDPOINTS.map((endpoint) => servedBy(endpoint, "helsinki", POLICIES, SPACES, graph.nodes)));
    expect(Object.fromEntries(counts)).toEqual({ BikeHireDockingStation: 2, Operator: 2 });
    expect(Object.fromEntries(writtenBy(PIPELINES, graph.nodes))).toEqual({ BikeHireDockingStation: ["gbfs"] });
  });
});

describe("areas, layout and positions", () => {
  it("puts a class in its in_subset, a personal one in personal data, the rest in main", () => {
    const areas = areasOf(graph.nodes, graph.edges, new Map([["Rider", "accounts"]]));
    expect(areas.get("Rider")).toBe("accounts");
    expect(areas.get("Place")).toBe(MAIN_AREA);
    expect(areaTabs(areas)).toEqual([MAIN_AREA, "accounts"]);
    const unsorted = areasOf(graph.nodes, graph.edges, new Map());
    expect(unsorted.get("Rider")).toBe(PERSONAL_AREA);
    expect(areaTabs(new Map([["A", MAIN_AREA]]))).toEqual([]);
  });

  it("lays the same model out the same way every time", () => {
    const first = place(graph.nodes, graph.edges);
    const again = place(graphData(parseModel(BIKES)).nodes, graphData(parseModel(BIKES)).edges);
    expect(again.placed.map(({ name, x, y }) => ({ name, x, y }))).toEqual(first.placed.map(({ name, x, y }) => ({ name, x, y })));
  });

  it("reads kept positions per model and drops anything malformed", () => {
    expect(readPositions('{"p/m":{"A":{"x":1,"y":2},"B":{"x":"1","y":2},"C":null}}', "p/m")).toEqual({ A: { x: 1, y: 2 } });
    expect(readPositions('{"p/m":{"A":{"x":1,"y":2}}}', "p/other")).toEqual({});
    expect(readPositions("not json", "p/m")).toEqual({});
    expect(readPositions("[1]", "p/m")).toEqual({});
    expect(readPositions(null, "p/m")).toEqual({});
    const kept = writePositions('{"p/a":{"A":{"x":1,"y":1}}}', "p/b", { B: { x: 2, y: 2 } });
    expect(JSON.parse(kept)).toEqual({ "p/a": { A: { x: 1, y: 1 } }, "p/b": { B: { x: 2, y: 2 } } });
    expect(JSON.parse(writePositions(kept, "p/a", {}))).toEqual({ "p/b": { B: { x: 2, y: 2 } } });
    expect(JSON.parse(writePositions("garbage", "p/a", {}))).toEqual({});
  });

  it("lists a class's own fields first and its inherited ones after, marked", () => {
    const model = parseModel(BIKES);
    const station = model.classes.find((klass) => klass.name === "BikeHireDockingStation");
    if (!station) throw new Error("no station");
    expect(fieldsOf(model, station).map(({ slot, inherited }) => `${slot.name}${inherited ? `<${inherited}` : ""}`)).toEqual([
      "capacity",
      "freeBikes",
      "contactPoint",
      "operator",
      "name<Place",
    ]);
  });
});

const LISTS: Record<string, Manifest[]> = {
  datamodels: [manifest("DataModel", "bikes", { contextSpaceRef: "helsinki", linkml: BIKES, classes: ["BikeHireDockingStation"] })],
  spaces: SPACES,
  endpoints: ENDPOINTS,
  policies: POLICIES,
  pipelines: PIPELINES,
};
function answer(path: string): Response | undefined {
  const plural = /^\/api\/v1\/projects\/helsinki\/([a-z]+)$/.exec(path)?.[1];
  return plural ? jsonResponse(list(LISTS[plural] ?? [])) : undefined;
}
const READER = { project: "helsinki", bootstrap: false, grants: [{ rule: { kinds: ["*"], verbs: ["read"] } }] };

afterEach(() => {
  vi.unstubAllGlobals();
  window.localStorage.clear();
});

describe("the canvas page", () => {
  it("opens without the shell, lights what an Endpoint serves and shows a picked class", async () => {
    await renderRoute({ path: "/projects/helsinki/models/bikes/diagram", answer });
    expect(await screen.findByRole("heading", { level: 1, name: "Diagram of bikes" })).toBeInTheDocument();
    expect(screen.queryByRole("navigation", { name: /main/i })).toBeNull();
    await screen.findByTestId("diagram-badge-BikeHireDockingStation");
    expect(screen.getByTestId("diagram-badge-BikeHireDockingStation")).toHaveTextContent("2");
    expect(screen.queryByTestId("diagram-badge-Rider")).toBeNull();

    await userEvent.selectOptions(screen.getByLabelText(en.models.canvas.overlay), "endpoint:helsinki-bikes");
    expect(await screen.findByText("1 class of 4 is reached; the rest is faded.")).toBeInTheDocument();
    const station = screen.getByRole("button", { name: en.models.graph.openClass.replace("{name}", "BikeHireDockingStation") });
    expect(station).not.toHaveAttribute("opacity");
    expect(screen.getByRole("button", { name: en.models.graph.openClass.replace("{name}", "Operator") })).toHaveAttribute("opacity", "0.45");
    expect(station.querySelectorAll('[data-served="false"]')).toHaveLength(1);

    await userEvent.click(station);
    const panel = within(screen.getByRole("complementary"));
    const fields = panel.getByRole("table", { name: "Fields of BikeHireDockingStation" });
    expect(within(fields).getAllByRole("row")).toHaveLength(6);
    expect(within(fields).getByText("from Place")).toBeInTheDocument();
    expect(panel.getByRole("link", { name: "helsinki-bikes" })).toHaveAttribute("href", "/projects/helsinki/endpoints/helsinki-bikes");
    expect(panel.getByRole("link", { name: "gbfs" })).toHaveAttribute("href", "/projects/helsinki/pipelines/gbfs");
    expect(panel.getByRole("link", { name: en.models.canvas.edit })).toHaveAttribute("href", "/projects/helsinki/models/bikes?class=BikeHireDockingStation");
  });

  it("gives a reader no Edit link, and keeps a moved box in this browser until reset", async () => {
    window.localStorage.setItem(LAYOUT_STORAGE, JSON.stringify({ [layoutKey("helsinki", "bikes")]: { Operator: { x: 900, y: 40 } } }));
    await renderRoute({ path: "/projects/helsinki/models/bikes/diagram", answer, permissions: READER });
    const operator = await screen.findByRole("button", { name: en.models.graph.openClass.replace("{name}", "Operator") });
    expect(operator.querySelector("rect")).toHaveAttribute("x", "900");
    operator.focus();
    await userEvent.keyboard("{Shift>}{ArrowRight}{/Shift}");
    await waitFor(() =>
      expect(JSON.parse(window.localStorage.getItem(LAYOUT_STORAGE) ?? "{}")).toEqual({ "helsinki/bikes": { Operator: { x: 920, y: 40 } } }),
    );
    await userEvent.click(operator);
    const panel = within(screen.getByRole("complementary"));
    expect(panel.queryByRole("link", { name: en.models.canvas.edit })).toBeNull();
    expect(panel.getByRole("button", { name: new RegExp(en.models.canvas.edit) })).toHaveAttribute("aria-disabled", "true");
    await userEvent.click(screen.getByRole("button", { name: en.models.canvas.reset }));
    expect(JSON.parse(window.localStorage.getItem(LAYOUT_STORAGE) ?? "")).toEqual({});
  });

  it("splits a model with areas into tabs and draws only the picked area", async () => {
    await renderRoute({ path: "/projects/helsinki/models/bikes/diagram", answer });
    const tabs = await screen.findByRole("tablist", { name: en.models.canvas.areas });
    expect(within(tabs).getAllByRole("tab").map((tab) => tab.textContent)).toEqual([en.models.canvas.areaMain, "accounts"]);
    expect(screen.queryByRole("button", { name: en.models.graph.openClass.replace("{name}", "Rider") })).toBeNull();
    await userEvent.click(within(tabs).getByRole("tab", { name: "accounts" }));
    expect(await screen.findByRole("button", { name: en.models.graph.openClass.replace("{name}", "Rider") })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: en.models.graph.openClass.replace("{name}", "Place") })).toBeNull();
  });
});

describe("the canvas as a part", () => {
  it("draws the model with its Endpoint badges and says what to pick when nothing is", async () => {
    const { ModelCanvas } = await import("../src/pages/models/ModelCanvas");
    const { renderPart } = await import("./page_contract");
    const { createRootRoute, createRouter, RouterProvider } = await import("@tanstack/react-router");
    const root = createRootRoute({ component: () => <ModelCanvas project="helsinki" name="bikes" /> });
    renderPart(<RouterProvider router={createRouter({ routeTree: root })} />, {
      answer: (url) =>
        url.pathname.endsWith("/permissions/me") ? jsonResponse({ project: "helsinki", bootstrap: false, grants: [] }) : answer(url.pathname),
    });
    expect(await screen.findByTestId("diagram-badge-Operator")).toHaveTextContent("2");
    expect(screen.getByText(en.models.canvas.pickHint)).toBeInTheDocument();
    expect(screen.getByLabelText(en.models.canvas.overlay)).toHaveAccessibleDescription(en.models.canvas.overlayHint);
  });

  const renderCanvas = async () => {
    const { ModelCanvas } = await import("../src/pages/models/ModelCanvas");
    const { renderPart } = await import("./page_contract");
    const { createRootRoute, createRouter, RouterProvider } = await import("@tanstack/react-router");
    const root = createRootRoute({ component: () => <ModelCanvas project="helsinki" name="bikes" /> });
    renderPart(<RouterProvider router={createRouter({ routeTree: root })} />, {
      answer: (url) =>
        url.pathname.endsWith("/permissions/me") ? jsonResponse({ project: "helsinki", bootstrap: false, grants: [] }) : answer(url.pathname),
    });
    await screen.findByTestId("diagram-badge-Operator");
  };

  it("keeps the picker's how-to behind a ? beside its label, not as a paragraph under it (T-6164)", async () => {
    await renderCanvas();
    const tip = screen.getByRole("tooltip", { hidden: true });
    expect(tip).toHaveTextContent(en.models.canvas.overlayHint);
    expect(tip).toHaveClass("sr-only");
    const help = screen.getByRole("button", { name: en.models.canvas.overlayHelp });
    expect(help).toHaveAttribute("aria-expanded", "false");
    await userEvent.click(help);
    expect(help).toHaveAttribute("aria-expanded", "true");
    expect(tip).not.toHaveClass("sr-only");
    await userEvent.keyboard("{Escape}");
    expect(tip).toHaveClass("sr-only");
  });

  it("brings a pressed class's details into view and focus where they sit under the drawing (T-6164)", async () => {
    const scrollIntoView = vi.fn();
    Element.prototype.scrollIntoView = scrollIntoView;
    const media = vi.spyOn(window, "matchMedia").mockImplementation((query: string) => ({ matches: false, media: query }) as MediaQueryList);
    try {
      await renderCanvas();
      await userEvent.click(screen.getByRole("button", { name: en.models.graph.openClass.replace("{name}", "Operator") }));
      const heading = screen.getByRole("heading", { level: 2, name: "Operator" });
      await waitFor(() => expect(heading).toHaveFocus());
      expect(scrollIntoView).toHaveBeenCalled();
    } finally {
      media.mockRestore();
      Reflect.deleteProperty(Element.prototype, "scrollIntoView");
    }
  });

  it("leaves the focus on the drawing where the details sit beside it", async () => {
    const media = vi.spyOn(window, "matchMedia").mockImplementation((query: string) => ({ matches: true, media: query }) as MediaQueryList);
    try {
      await renderCanvas();
      const box = screen.getByRole("button", { name: en.models.graph.openClass.replace("{name}", "Operator") });
      await userEvent.click(box);
      expect(await screen.findByRole("heading", { level: 2, name: "Operator" })).not.toHaveFocus();
    } finally {
      media.mockRestore();
    }
  });
});

describe("the canvas drawing (UI judge fixes)", () => {
  it("draws no minimap while the frame shows the whole drawing, and one once it does not (T-6164)", async () => {
    const { renderPart } = await import("./page_contract");
    const sizes = ["clientWidth", "clientHeight", "scrollWidth", "scrollHeight"] as const;
    const kept = sizes.map((size) => Object.getOwnPropertyDescriptor(HTMLElement.prototype, size));
    let drawing = 100;
    for (const size of sizes)
      Object.defineProperty(HTMLElement.prototype, size, { configurable: true, get: () => (size.startsWith("client") ? 2000 : drawing) });
    try {
      function Zooming() {
        const [, setRenders] = useState(0);
        return (
          <>
            <button onClick={() => setRenders((renders) => renders + 1)}>again</button>
            <LinkmlGraphView canvas source={BIKES} />
          </>
        );
      }
      renderPart(<Zooming />);
      await screen.findByRole("button", { name: en.models.graph.openClass.replace("{name}", "Operator") });
      await waitFor(() => expect(screen.queryByTestId("diagram-minimap")).toBeNull());
      drawing = 4000;
      await userEvent.click(screen.getByRole("button", { name: en.models.graph.zoomIn }));
      expect(await screen.findByTestId("diagram-minimap")).toBeInTheDocument();
    } finally {
      sizes.forEach((size, index) => {
        const descriptor = kept[index];
        if (descriptor) Object.defineProperty(HTMLElement.prototype, size, descriptor);
        else Reflect.deleteProperty(HTMLElement.prototype, size);
      });
    }
  });

  it("lets a short name leave its room to a long type, and cuts both only when both are long", () => {
    expect(fitRow("publisher", "→ Organization 0..1")).toEqual(["publisher", "→ Organization 0..1"]);
    expect(fitRow("distribution", "→ DistributionPoint *")).toEqual(["distribution", "→ DistributionPoint *"]);
    expect(fitRow("aVeryLongSlotNameIndeedHere", "→ AnotherVeryLongClassName 0..1")).toEqual(["aVeryLongSlotNam…", "→ AnotherVeryLon…"]);
  });

  it("brings the first lit class into view once per overlay", async () => {
    const { renderPart } = await import("./page_contract");
    const width = Object.getOwnPropertyDescriptor(HTMLElement.prototype, "clientWidth");
    const height = Object.getOwnPropertyDescriptor(HTMLElement.prototype, "clientHeight");
    const scrollTo = vi.fn();
    Object.defineProperty(HTMLElement.prototype, "clientWidth", { configurable: true, get: () => 300 });
    Object.defineProperty(HTMLElement.prototype, "clientHeight", { configurable: true, get: () => 200 });
    HTMLElement.prototype.scrollTo = scrollTo;
    try {
      const lit = new Map([["Rider", new Set<string>()]]);
      function Overlays() {
        const [highlight, setHighlight] = useState(lit);
        const [, setRenders] = useState(0);
        return (
          <>
            <button onClick={() => setRenders((renders) => renders + 1)}>again</button>
            <button onClick={() => setHighlight(new Map(lit))}>other</button>
            <LinkmlGraphView canvas source={BIKES} highlight={highlight} />
          </>
        );
      }
      renderPart(<Overlays />);
      await waitFor(() => expect(scrollTo).toHaveBeenCalledTimes(1));
      const [{ left, top }] = scrollTo.mock.calls[0] as [{ left: number; top: number }];
      // Rider lies outside the 300 x 200 frame at the start, so the frame scrolls to it.
      expect(left + top).toBeGreaterThan(0);
      // A render for another reason does not pull the frame back.
      await userEvent.click(screen.getByRole("button", { name: "again" }));
      expect(scrollTo).toHaveBeenCalledTimes(1);
      // Another overlay is another reveal.
      await userEvent.click(screen.getByRole("button", { name: "other" }));
      await waitFor(() => expect(scrollTo).toHaveBeenCalledTimes(2));
    } finally {
      if (width) Object.defineProperty(HTMLElement.prototype, "clientWidth", width);
      if (height) Object.defineProperty(HTMLElement.prototype, "clientHeight", height);
      Reflect.deleteProperty(HTMLElement.prototype, "scrollTo");
    }
  });
});

describe("the overlay pick's how-to (T-6188)", () => {
  it("opens on hover and closes on leave, with the text under the id the select reads", async () => {
    const { OverlayHelp } = await import("../src/pages/models/ModelCanvas");
    const { render } = await import("@testing-library/react");
    render(<OverlayHelp id="overlay-hint" label="How the overlay works" hint="Pick an Endpoint" />);
    const help = screen.getByRole("button", { name: "How the overlay works" });
    const tip = screen.getByRole("tooltip", { hidden: true });
    expect(tip).toHaveAttribute("id", "overlay-hint");
    expect(help).toHaveAttribute("aria-describedby", "overlay-hint");
    await userEvent.hover(help);
    expect(tip).not.toHaveClass("sr-only");
    await userEvent.unhover(help);
    expect(tip).toHaveClass("sr-only");
  });
});
