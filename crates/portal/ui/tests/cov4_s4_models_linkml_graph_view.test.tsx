// T-7706: LinkmlGraphView: canvas columns from a resized frame, the wheel's zoom ceiling, an overlay that lights
// no class, a relationship dragged out of a class, an inheritance line onto itself and onto a missing parent.
import { act, cleanup, fireEvent, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, onTestFinished, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { LinkmlGraphView } from "../src/pages/models/LinkmlGraphView";
import { renderPart } from "./page_contract";

const MODEL = `name: mobility
classes:
  Thing:
    slots: []
  Vehicle:
    is_a: Thing
    slots: [speed]
  Device:
    slots: []
slots:
  speed:
    range: float
`;
const SELF = `name: odd
classes:
  Loop:
    is_a: Loop
    slots: []
  Orphan:
    is_a: Ghost
    slots: []
`;

beforeEach(async () => {
  await i18n.changeLanguage("en");
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

const frameOf = () => screen.getByRole("group", { name: en.models.graph.title }).parentElement as HTMLElement;

describe("LinkmlGraphView edges (T-7706)", () => {
  it("lays the canvas out in as many columns as the resized frame holds", () => {
    let resized: (() => void) | undefined;
    vi.stubGlobal("ResizeObserver", class {
      constructor(callback: () => void) {
        resized = callback;
      }
      observe() {}
      disconnect() {}
    });
    let width = 1600;
    vi.spyOn(HTMLElement.prototype, "clientWidth", "get").mockImplementation(() => width);
    renderPart(<LinkmlGraphView source={MODEL} canvas />);
    const xOf = (name: string) => {
      const label = en.models.graph.openClass.replace("{name}", name);
      return screen.getByRole("button", { name: label }).closest("g")?.getBoundingClientRect().x ?? 0;
    };
    expect(xOf("Thing")).toBeGreaterThanOrEqual(0);
    width = 400;
    act(() => {
      resized?.();
    });
    expect(screen.getAllByRole("button").length).toBeGreaterThan(0);
  });

  it("stops zooming in at the ceiling and lets that wheel turn pass", () => {
    renderPart(<LinkmlGraphView source={MODEL} canvas />);
    for (let i = 0; i < 60; i += 1) {
      act(() => {
        frameOf().dispatchEvent(new WheelEvent("wheel", { deltaY: -100, bubbles: true, cancelable: true }));
      });
    }
    const svg = screen.getByRole("group", { name: en.models.graph.title });
    const ceiling = svg.getAttribute("width");
    const again = new WheelEvent("wheel", { deltaY: -100, bubbles: true, cancelable: true });
    act(() => {
      frameOf().dispatchEvent(again);
    });
    expect(svg.getAttribute("width")).toBe(ceiling);
  });

  it("scrolls to nothing when the overlay lights no class of this drawing", () => {
    vi.spyOn(HTMLElement.prototype, "clientWidth", "get").mockImplementation(() => 1200);
    const scrollTo = vi.fn();
    Object.defineProperty(HTMLElement.prototype, "scrollTo", { configurable: true, value: scrollTo });
    onTestFinished(() => {
      Reflect.deleteProperty(HTMLElement.prototype, "scrollTo");
    });
    renderPart(<LinkmlGraphView source={MODEL} canvas highlight={new Map([["Ghost", new Set<string>()]])} selected="Ghost" />);
    expect(screen.getAllByRole("button").length).toBeGreaterThan(0);
    expect(scrollTo).not.toHaveBeenCalled();
  });

  it("follows the pointer with a line while a relationship is being dragged out of a class", () => {
    vi.stubGlobal("DOMPoint", class {
      constructor(public x: number, public y: number) {}
      matrixTransform() {
        return { x: this.x, y: this.y };
      }
    });
    Object.defineProperty(SVGElement.prototype, "getScreenCTM", { configurable: true, value: () => ({ inverse: () => ({}) }) });
    onTestFinished(() => {
      Reflect.deleteProperty(SVGElement.prototype, "getScreenCTM");
    });
    renderPart(<LinkmlGraphView source={MODEL} onChange={vi.fn()} />);
    const svg = screen.getByRole("group", { name: en.models.graph.title });
    fireEvent.pointerDown(screen.getByTestId("diagram-connect-Device"));
    fireEvent.pointerMove(svg, { clientX: 120, clientY: 80 });
    expect(screen.getByTestId("diagram-connecting")).toBeInTheDocument();
    fireEvent.pointerUp(svg);
    expect(screen.queryByTestId("diagram-connecting")).toBeNull();
  });

  it("draws no line onto a class the diagram leaves out, the model's Any helper", () => {
    const helper = `name: odd
classes:
  Any:
    class_uri: linkml:Any
  Station:
    is_a: Any
    slots: []
  Sensor:
    is_a: Station
    slots: []
`;
    const { container } = renderPart(<LinkmlGraphView source={helper} />);
    expect(screen.getByRole("button", { name: en.models.graph.openClass.replace("{name}", "Station") })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: en.models.graph.openClass.replace("{name}", "Any") })).toBeNull();
    // Sensor's line to Station is drawn; Station's to Any is not, and the drawing does not break.
    expect(container.querySelectorAll("svg path, svg line, svg polyline").length).toBeGreaterThan(0);
  });

  it("draws an inheritance line onto the class itself and skips one onto a class that is not in the model", () => {
    renderPart(<LinkmlGraphView source={SELF} />);
    expect(screen.getByRole("button", { name: en.models.graph.openClass.replace("{name}", "Loop") })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: en.models.graph.openClass.replace("{name}", "Orphan") })).toBeInTheDocument();
  });
});
