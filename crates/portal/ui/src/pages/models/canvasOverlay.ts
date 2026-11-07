/**
 * What the model canvas lays over the drawing (T-6094): which classes and attributes each
 * Endpoint serves, which Pipelines write a class, and the area each class is coloured by.
 *
 * An Endpoint serves what the Policies bound to it name (`bindingOf`, the gateway's own rule):
 * each rule's `information[]` names entity types and, optionally, the attributes of them
 * (`propertyNames`, `relationshipNames`); an item without entities names every type, one without
 * names every attribute. The Endpoint's `projection.hiddenAttributes` then takes attributes out.
 * Nothing here asks the API: the lists are the ones the page read for the person, so an Endpoint
 * or Policy the person may not read is never drawn.
 */
import type { Manifest } from "../../api/manifest";
import { bindingOf, policyRules } from "../../components/endpoints/policyBinding";
import { spaceOf } from "../spaces/spaceFacts";
import type { GraphNode } from "./linkml";

/** Per class, the attribute names one Endpoint serves. A class absent from the map is not served. */
export type Served = ReadonlyMap<string, ReadonlySet<string>>;

function strings(value: unknown): string[] {
  return Array.isArray(value) ? value.filter((one): one is string => typeof one === "string" && one !== "") : [];
}

function records(value: unknown): Record<string, unknown>[] {
  return Array.isArray(value)
    ? value.filter((one): one is Record<string, unknown> => typeof one === "object" && one !== null && !Array.isArray(one))
    : [];
}

/**
 * The classes and attributes of the drawn model `endpoint` serves. `nodes` are the drawing's
 * boxes; only classes count, and a rule naming a type the model lacks adds nothing.
 */
export function servedBy(
  endpoint: Manifest,
  project: string,
  policies: Manifest[],
  spaces: Manifest[],
  nodes: readonly GraphNode[],
): Served {
  const classes = new Map(nodes.filter((node) => node.kind === "class").map((node) => [node.name, (node.rows ?? []).map((row) => row.name)]));
  const binding = bindingOf(endpoint, project, spaceOf(endpoint), policies, spaces);
  const hidden = new Set(strings((endpoint.spec.projection as { hiddenAttributes?: unknown } | undefined)?.hiddenAttributes));
  const served = new Map<string, Set<string>>();
  if (binding.kind === "unbound") return served;
  for (const rule of binding.policies.flatMap(policyRules)) {
    for (const item of records(rule.information)) {
      const types = records(item.entities).flatMap((entity) => strings([entity.type]));
      const named = [...strings(item.propertyNames), ...strings(item.relationshipNames)];
      for (const type of types.length > 0 ? types : [...classes.keys()]) {
        const fields = classes.get(type);
        if (!fields) continue;
        const kept = served.get(type) ?? new Set<string>();
        for (const field of named.length > 0 ? fields.filter((one) => named.includes(one) || one === fields[0]) : fields) {
          if (!hidden.has(field)) kept.add(field);
        }
        served.set(type, kept);
      }
    }
  }
  return served;
}

/** How many of the Endpoints serve each class, for the badge on its box. */
export function servingCounts(all: readonly Served[]): Map<string, number> {
  const counts = new Map<string, number>();
  for (const served of all) {
    for (const name of served.keys()) counts.set(name, (counts.get(name) ?? 0) + 1);
  }
  return counts;
}

/** Per class, the Pipelines whose output writes it (`spec.output.type`), sorted by name. */
export function writtenBy(pipelines: Manifest[], nodes: readonly GraphNode[]): Map<string, string[]> {
  const classes = new Set(nodes.filter((node) => node.kind === "class").map((node) => node.name));
  const written = new Map<string, string[]>();
  for (const pipeline of [...pipelines].sort((a, b) => a.metadata.name.localeCompare(b.metadata.name))) {
    const type = (pipeline.spec.output as { type?: unknown } | undefined)?.type;
    if (typeof type === "string" && classes.has(type)) written.set(type, [...(written.get(type) ?? []), pipeline.metadata.name]);
  }
  return written;
}

/** The area of a class with personal data and no `in_subset` of its own. */
export const PERSONAL_AREA = "personal";
/** The area of every other class of the model itself. */
export const MAIN_AREA = "main";

/**
 * The area each box is coloured and tabbed by: the class's first LinkML `in_subset`, else the
 * import it came from, else `personal` when any of its slots is personal data, else `main`.
 * An enum takes the area of the first class that uses it, so it sits on that class's tab.
 */
export function areasOf(nodes: readonly GraphNode[], uses: readonly { from: string; to: string }[], subsets: ReadonlyMap<string, string>): Map<string, string> {
  const areas = new Map<string, string>();
  for (const node of nodes) {
    if (node.kind === "enum") continue;
    areas.set(
      node.name,
      subsets.get(node.name) ?? node.from ?? ((node.rows ?? []).some((row) => row.personal) ? PERSONAL_AREA : MAIN_AREA),
    );
  }
  for (const node of nodes) {
    if (node.kind !== "enum") continue;
    const user = uses.find((edge) => edge.to === node.name && areas.has(edge.from));
    areas.set(node.name, node.from ?? (user ? (areas.get(user.from) ?? MAIN_AREA) : MAIN_AREA));
  }
  return areas;
}

/** Tabs below the canvas: Main first, then every other area in name order; none for one area. */
export function areaTabs(areas: ReadonlyMap<string, string>): string[] {
  const others = [...new Set(areas.values())].filter((area) => area !== MAIN_AREA).sort();
  return others.length === 0 ? [] : [MAIN_AREA, ...others];
}

/** Where a person moved boxes, by class name, as the browser keeps it; anything malformed is dropped. */
export type Positions = Record<string, { x: number; y: number }>;

/** The one browser key every model's layout is kept under, a map of `project/model` to positions. */
export const LAYOUT_STORAGE = "jc-diagram-layout";

function objectOf(value: unknown): Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value) ? (value as Record<string, unknown>) : {};
}

/** The positions kept for one model (`project/model`) in the stored text. */
export function readPositions(text: string | null, model: string): Positions {
  if (!text) return {};
  try {
    return Object.fromEntries(
      Object.entries(objectOf(objectOf(JSON.parse(text))[model])).flatMap(([name, at]) => {
        const point = at as { x?: unknown; y?: unknown } | null;
        return typeof point?.x === "number" && typeof point.y === "number" && Number.isFinite(point.x) && Number.isFinite(point.y)
          ? [[name, { x: point.x, y: point.y }]]
          : [];
      }),
    );
  } catch {
    return {};
  }
}

/** The stored text with one model's positions replaced; an empty layout drops the model. */
export function writePositions(text: string | null, model: string, positions: Positions): string {
  let all: Record<string, unknown> = {};
  try {
    all = objectOf(text ? JSON.parse(text) : {});
  } catch {
    all = {};
  }
  const others = { ...all };
  delete others[model];
  return JSON.stringify(Object.keys(positions).length === 0 ? others : { ...others, [model]: positions });
}
