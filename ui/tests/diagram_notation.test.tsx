/**
 * T-3589: one documented notation for the model diagram — UML's hollow triangle for `is_a` and
 * mixins, ER crow's foot at both ends of a reference and a relationship, an arrow at an enum —
 * a legend drawn with the same marks, and the drawing as SVG and as Mermaid `erDiagram`, the
 * notation of LinkML's own `gen-erdiagram`.
 */
import { cleanup, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { mermaidErDiagram } from "../src/pages/models/diagramExport";
import { LinkmlGraphView } from "../src/pages/models/LinkmlGraphView";
import { graphData, parseModel } from "../src/pages/models/linkml";
import { expectNoRawKeys } from "./checks";
import { expectNoAxeViolations, inEveryLocale, renderPart } from "./page_contract";

const MODEL = `name: city
classes:
  Thing:
    slots: []
  Traceable:
    slots: []
  Building:
    is_a: Thing
    mixins: [Traceable]
    slots: [id, owner, kind, floors]
  Person:
    slots: [id, homes]
  Floor:
    slots: [level]
slots:
  id:
    identifier: true
    range: string
  owner:
    range: Person
    required: true
    inverse: homes
    annotations: { ngsi_ld_kind: Relationship }
  homes:
    range: Building
    multivalued: true
    inverse: owner
    annotations: { ngsi_ld_kind: Relationship }
  floors:
    range: Floor
    multivalued: true
    required: true
  level:
    range: integer
  kind:
    range: BuildingKind
enums:
  BuildingKind:
    permissible_values:
      house: {}
      tower: {}
`;

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

/** The marker a path ends with, by the end's name (`triangle`, `many`, …), or null. */
function end(path: Element | null, side: "start" | "end"): string | null {
  const url = path?.getAttribute(`marker-${side}`);
  return url ? (/-([A-Za-z]+)\)$/.exec(url)?.[1] ?? url) : null;
}

describe("the notation", () => {
  it("draws a hollow triangle at the parent, crow's feet at a reference's ends, an arrow at an enum", () => {
    const { container } = renderPart(<LinkmlGraphView source={MODEL} />);
    const lines = [...container.querySelectorAll("g.text-fg-subtle > path")];
    const shapes = lines.map((path) => [path.getAttribute("stroke-dasharray"), end(path, "start"), end(path, "end")]);
    expect(shapes).toEqual(
      expect.arrayContaining([
        [null, null, "triangle"], // Building is_a Thing
        ["6 4", null, "triangle"], // Building mixes in Traceable
        // Building.floors: a Building holds at least one Floor; nothing limits how many Buildings name a Floor.
        ["2 3", "many", "oneOrMore"],
        ["1 4", null, "arrow"], // Building.kind picks from BuildingKind
      ]),
    );
  });

  it("ends a relationship with the crow's feet of both of its ends", () => {
    renderPart(<LinkmlGraphView source={MODEL} />);
    const relationship = screen.getByRole("button", { name: /Open the relationship/ });
    const line = relationship.querySelectorAll("path")[1];
    // owner (required, single) on Building, homes (multivalued) on Person: any number of
    // Buildings per Person at the Building end, exactly one Person per Building at the Person end.
    expect([end(line, "start"), end(line, "end")]).toEqual(["many", "one"]);
  });

  it("keeps marker ids apart when two drawings share a page", () => {
    const { container } = renderPart(
      <>
        <LinkmlGraphView source={MODEL} />
        <LinkmlGraphView source={MODEL} />
      </>,
    );
    const ids = [...container.querySelectorAll("marker")].map((marker) => marker.id);
    expect(ids).toHaveLength(12);
    expect(new Set(ids).size).toBe(12);
  });

  // T-6090 (owner walk): two paragraphs of notation and a strip of marks sat above the drawing and
  // pushed it below the fold. They live in one Notation popover in the diagram's toolbar now.
  it("keeps the notation in a closed popover of the toolbar, so the drawing comes first", async () => {
    const user = userEvent.setup();
    renderPart(<LinkmlGraphView source={MODEL} />);
    const tools = screen.getByRole("toolbar", { name: en.models.graph.tools });
    expect(within(tools).getByRole("combobox", { name: en.models.graph.find })).toHaveAttribute(
      "placeholder",
      en.models.graph.findHint,
    );
    expect(within(tools).getByRole("button", { name: en.models.graph.fit })).toBeInTheDocument();
    const summary = within(tools).getByText(en.models.graph.notation.button, { selector: "summary" });
    const popover = summary.closest("details") as HTMLDetailsElement;
    expect(popover.open).toBe(false);
    expect(popover).toContainElement(screen.getByText(en.models.graph.legend));
    await user.click(summary);
    expect(popover.open).toBe(true);
    expect(within(popover).getByRole("figure", { name: en.models.graph.notation.title })).toBeVisible();
    // Nothing of the notation precedes the drawing outside the toolbar.
    const drawing = screen.getByRole("group", { name: en.models.graph.title });
    expect(tools.compareDocumentPosition(drawing) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it("explains every mark in a legend drawn with the same marks, in every locale", async () => {
    await inEveryLocale(async () => {
      const { container, unmount } = renderPart(<LinkmlGraphView source={MODEL} />);
      const legend = screen.getByRole("figure", { name: i18n.t("models.graph.notation.title") });
      const items = within(legend).getAllByRole("listitem");
      expect(items.map((item) => item.textContent)).toEqual(
        ["isA", "mixin", "one", "zeroOrOne", "oneOrMore", "many", "enum"].map((key) => i18n.t(`models.graph.notation.${key}`)),
      );
      const marks = items.map((item) => end(item.querySelector("line"), "end"));
      expect(marks).toEqual(["triangle", "triangle", "one", "zeroOrOne", "oneOrMore", "many", "arrow"]);
      expectNoRawKeys(container);
      unmount();
    });
  });

  it("is axe-clean with the legend and the export buttons", async () => {
    const { container } = renderPart(<LinkmlGraphView source={MODEL} />);
    await expectNoAxeViolations(container);
  });
});

describe("the drawing as files", () => {
  it("writes the model as a Mermaid erDiagram with crow's foot ends", () => {
    const { nodes, edges } = graphData(parseModel(MODEL));
    expect(mermaidErDiagram(nodes, edges)).toBe(
      [
        "erDiagram",
        "  Thing {",
        "    URN id PK",
        "  }",
        "  Traceable {",
        "    URN id PK",
        "  }",
        "  Building {",
        "    string id PK",
        "    Person owner FK",
        "    BuildingKind kind",
        "    Floor[] floors FK",
        "  }",
        "  Person {",
        "    string id PK",
        "    Building[] homes FK",
        "  }",
        "  Floor {",
        "    URN id PK",
        "    integer level",
        "  }",
        "  %% Building is_a Thing",
        "  %% Building mixin Traceable",
        "  Building }o--|{ Floor : \"floors\"",
        "  Building }o--|| Person : \"owner / homes\"",
        "",
      ].join("\n"),
    );
  });

  it("never lets a name close a block or start a statement in Mermaid", () => {
    const hostile = `name: x
classes:
  "A } B\\n%%":
    slots: [s]
slots:
  s:
    range: '"<img src=x onerror=alert(1)>'
`;
    const { nodes, edges } = graphData(parseModel(hostile));
    const text = mermaidErDiagram(nodes, edges);
    expect(text.split("\n").filter((line) => line.trim() === "}")).toHaveLength(1);
    expect(text).not.toMatch(/[<>"]/);
    expect(text).toContain("A___B___ {");
  });

  it("downloads the Mermaid text and the drawing as a standalone SVG", async () => {
    const user = userEvent.setup();
    const files: Blob[] = [];
    const names: string[] = [];
    vi.stubGlobal("URL", Object.assign(URL, { createObjectURL: (blob: Blob) => (files.push(blob), "blob:x"), revokeObjectURL: () => {} }));
    vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(function (this: HTMLAnchorElement) {
      names.push(this.download);
    });
    renderPart(<LinkmlGraphView source={MODEL} />);

    // One Download menu holds both files (T-6090).
    await user.click(screen.getByRole("button", { name: en.models.graph.download }));
    await user.click(await screen.findByRole("menuitem", { name: en.models.graph.exportMermaid }));
    await user.click(screen.getByRole("button", { name: en.models.graph.download }));
    await user.click(await screen.findByRole("menuitem", { name: en.models.graph.exportSvg }));

    expect(names).toEqual(["city.mmd", "city.svg"]);
    expect(await files[0].text()).toMatch(/^erDiagram\n/);
    const svg = await files[1].text();
    expect(files[1].type).toBe("image/svg+xml");
    expect(svg).toMatch(/^<\?xml version="1.0" encoding="UTF-8"\?>\n<svg[^>]* xmlns="http:\/\/www.w3.org\/2000\/svg"/);
    expect(svg).toContain(">Building<");
    expect(svg).not.toMatch(/ (class|tabindex|role)="/);
  });
});
