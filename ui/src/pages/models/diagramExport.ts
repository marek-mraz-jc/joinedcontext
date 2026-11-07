/**
 * The model diagram as files (T-3589): Mermaid `erDiagram` text, the notation of LinkML's own
 * `gen-erdiagram`, and the drawing as a standalone SVG with its colours written in.
 */
import type { GraphEdge, GraphNode, Multiplicity } from "./linkml";

/** Crow's foot ends in Mermaid: the left one says how many of the left entity, the right one of the right. */
const LEFT: Record<Multiplicity, string> = { "1": "||", "0..1": "|o", "1..*": "}|", "*": "}o" };
const RIGHT: Record<Multiplicity, string> = { "1": "||", "0..1": "o|", "1..*": "|{", "*": "o{" };

/**
 * A name as Mermaid reads it: letters, digits, `_` and `-`, starting with a letter. A model's
 * names are LinkML identifiers already; anything else (a space, a quote, markup) becomes `_`, so
 * a name can never close a block or start a statement of its own.
 */
function word(name: string): string {
  const safe = name.replace(/[^A-Za-z0-9_-]/g, "_");
  return /^[A-Za-z]/.test(safe) ? safe : `_${safe}`;
}

/** A relationship's label, quoted: no quote, no line break inside. */
function label(text: string): string {
  return `"${text.replace(/["\r\n]/g, " ")}"`;
}

/**
 * The model as a Mermaid `erDiagram`: every class an entity with its fields (`PK` the key, `FK`
 * a reference, `[]` a list), every reference and relationship a line with crow's foot ends.
 * Mermaid has no inheritance and no enums in an ER diagram, so `is_a` and mixins are comments
 * and a field that picks from an enum names the enum as its type, as `gen-erdiagram` does.
 */
export function mermaidErDiagram(nodes: readonly GraphNode[], edges: readonly GraphEdge[]): string {
  const lines = ["erDiagram"];
  for (const node of nodes) {
    if (node.kind !== "class") continue;
    lines.push(`  ${word(node.name)} {`);
    for (const row of node.rows ?? []) {
      const type = `${word(row.type)}${row.multivalued ? "[]" : ""}`;
      lines.push(`    ${type} ${word(row.name)}${row.key ? ` ${row.key.toUpperCase()}` : ""}`);
    }
    lines.push("  }");
  }
  for (const edge of edges) {
    if (edge.kind === "is_a" || edge.kind === "mixin") {
      lines.push(`  %% ${word(edge.from)} ${edge.kind} ${word(edge.to)}`);
    } else if (edge.kind !== "enum") {
      const name = edge.kind === "relationship" ? `${edge.label ?? ""} / ${edge.inverse ?? ""}` : (edge.label ?? "");
      const left = LEFT[edge.fromMultiplicity ?? "*"];
      const right = RIGHT[edge.toMultiplicity ?? "*"];
      lines.push(`  ${word(edge.from)} ${left}--${right} ${word(edge.to)} : ${label(name)}`);
    }
  }
  return `${lines.join("\n")}\n`;
}

/** What a stylesheet gives the drawing, written onto each element so the file looks the same alone. */
const PAINT = ["fill", "stroke", "stroke-width", "stroke-dasharray", "font-family", "font-size", "font-weight", "font-style", "opacity"];

/**
 * The drawing as an SVG file: a copy of `svg` with the computed paint of every element written
 * in (the page's classes do not travel with the file), the namespace set, and its interaction
 * attributes dropped.
 */
export function svgFile(svg: SVGSVGElement): string {
  const copy = svg.cloneNode(true) as SVGSVGElement;
  const originals = [svg, ...svg.querySelectorAll("*")];
  const copies = [copy, ...copy.querySelectorAll("*")];
  originals.forEach((element, index) => {
    const target = copies[index];
    const style = getComputedStyle(element);
    for (const property of PAINT) {
      const value = style.getPropertyValue(property);
      if (value) target.setAttribute(property, value);
    }
    for (const attribute of ["class", "tabindex", "role"]) target.removeAttribute(attribute);
  });
  copy.setAttribute("xmlns", "http://www.w3.org/2000/svg");
  return `<?xml version="1.0" encoding="UTF-8"?>\n${new XMLSerializer().serializeToString(copy)}\n`;
}

/** Hands `text` to the browser as a file called `name`. */
export function download(text: string, name: string, type: string): void {
  const href = URL.createObjectURL(new Blob([text], { type }));
  const link = document.createElement("a");
  link.href = href;
  link.download = name;
  link.click();
  URL.revokeObjectURL(href);
}
