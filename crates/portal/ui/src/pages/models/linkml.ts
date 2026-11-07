/**
 * The LinkML document the editor edits, and the rules it is judged by.
 *
 * The YAML text is the single source of truth: the structured view reads a projection of it
 * and writes back through the YAML document, so a comment or a hand-made ordering survives a
 * click in the tree, and the two views cannot drift (DM-13). Nothing here reaches the network;
 * the authoritative artifacts come from Model Tools (DM-18).
 */
import { parseDocument, type Document } from "yaml";
import { unitOf } from "../../units";

/** The NGSI-LD kinds a slot may declare (DM-05), in the annotation Model Tools reads. */
export const NGSI_LD_KINDS = [
  "Property",
  "GeoProperty",
  "Relationship",
  "LanguageProperty",
  "ListProperty",
  "JsonProperty",
  "VocabProperty",
] as const;

export type NgsiLdKind = (typeof NGSI_LD_KINDS)[number];

/** A slot without the annotation is a plain Property: the common case stays unwritten. */
export const DEFAULT_KIND: NgsiLdKind = "Property";

/**
 * Namespaces an organisation must never mint its own terms under (DM-04, DM-16). Reusing an
 * upstream IRI is how a model says it means the same thing as a standard; minting a new term
 * under someone else's namespace is squatting. The list is Model Tools' own, so the editor
 * refuses exactly what generation would refuse.
 */
export const RESERVED_NAMESPACES = [
  "https://smartdatamodels.org/",
  "https://raw.githubusercontent.com/smart-data-models/",
  "https://github.com/smart-data-models/",
  "https://uri.etsi.org/",
  "http://uri.etsi.org/",
  "https://www.w3.org/",
  "http://www.w3.org/",
  "https://w3id.org/linkml/",
] as const;

/** A slot carrying this annotation cites an upstream term, so a reserved IRI is a citation. */
export const UPSTREAM_ANNOTATION = "upstream_source";

/**
 * The IRI the organisation mints for a new class or slot (T-4829): `{default_prefix}:{name}`,
 * only under a prefix the model declares and no one else owns. Otherwise none, and `diagnose`
 * still asks for one: a guessed namespace would be squatting.
 */
export function mintIri(model: LinkmlModel, name: string): string | undefined {
  const prefix = model.default_prefix;
  if (!prefix || !model.prefixes[prefix] || !/^[A-Za-z_][A-Za-z0-9_-]*$/.test(name)) return undefined;
  const iri = `${prefix}:${name}`;
  return reservedNamespace(iri, model.prefixes) === undefined ? iri : undefined;
}

/** LinkML's `Any`: a slot ranging over it takes any value, so it is no class a reader follows. */
export function isAnyClass(klass: Pick<LinkmlClass, "name" | "class_uri">): boolean {
  return klass.class_uri === "linkml:Any" || klass.class_uri === "https://w3id.org/linkml/Any" || (klass.class_uri === undefined && klass.name === "Any");
}

/**
 * The classes only ever written inside another entity (`inlined: true` on every slot ranging
 * over them, T-4829): value objects such as a calculation period, with no `id` of their own.
 */
export function valueClasses(model: LinkmlModel): Set<string> {
  const names = new Set(model.classes.map((klass) => klass.name));
  const inline = new Set<string>();
  const referenced = new Set<string>();
  for (const klass of model.classes) {
    for (const slot of classSlots(model, klass)) {
      if (slot.range === undefined || !names.has(slot.range)) continue;
      if (isAnyClass(model.classes.find((one) => one.name === slot.range) ?? { name: "" })) continue;
      // A JsonProperty's range is the shape of its value (a Smart Data Model's L1/L2/L3 parts), held inline.
      (slot.inlined === true || slot.kind === "JsonProperty" ? inline : referenced).add(slot.range);
    }
    if (klass.is_a !== undefined) referenced.add(klass.is_a);
    for (const mixin of klass.mixins ?? []) referenced.add(mixin);
  }
  return new Set([...inline].filter((name) => !referenced.has(name)));
}

/** The LinkML ranges the editor offers, and how a dashboard may use each of them (DM-20). */
export const RANGES = [
  "string",
  "integer",
  "float",
  "double",
  "decimal",
  "boolean",
  "date",
  "datetime",
  "uri",
  "uriorcurie",
] as const;

export interface LinkmlUnit {
  ucum_code?: string;
  symbol?: string;
  /** The UN/CEFACT code NGSI-LD puts on the wire, and the QUDT unit beside it (DM-06, DM-59). */
  exact_mappings?: string[];
  /** The dimension, `qudt:hasQuantityKind`: what makes two units comparable at all (DM-59). */
  has_quantity_kind?: string;
}

export interface LinkmlSlot {
  name: string;
  range?: string;
  required?: boolean;
  multivalued?: boolean;
  deprecated?: boolean;
  description?: string;
  title?: Record<string, string>;
  slot_uri?: string;
  unit?: LinkmlUnit;
  kind: NgsiLdKind;
  /** Where an upstream term came from, when this slot cites one (DM-08, DM-16). */
  upstream?: string;
  pattern?: string;
  minimum_value?: number;
  maximum_value?: number;
  /** Where the slot stands in the display order, lower first (DM-98). */
  rank?: number;
  /** `annotations.label`: the slot's label per language, before its title (DM-100). */
  label?: Record<string, string>;
  /** `view_width`: the column's maximum width in characters, 8 to 80 (DM-134). */
  width?: number;
  /** `view_hidden`: the column stays one tick away (DM-134). */
  hidden?: boolean;
  /** `view_role`: what the column is to a row (DM-134). */
  role?: SlotRole;
  /** The profiles this slot belongs to, as LinkML `subsets`: which of them a projection takes. */
  subsets?: string[];
  /** The other end of a relationship, as LinkML `inverse` (DM-64). */
  inverse?: string;
  /** The slot is the class's key, as LinkML `identifier` (T-2881). */
  identifier?: boolean;
  /** Whether the target is written inside the entity; a relationship never is (DM-64). */
  inlined?: boolean;
  /** The delete rule a relationship's source end carries, as written (DM-66). */
  on_delete?: string;
  /** The formula that computes the slot from its entity's other slots, as LinkML writes it (DM-80). */
  equals_expression?: string;
  /** `annotations.personalData: true`: the slot holds data about a person (DM-120). */
  personal?: boolean;
}

export interface LinkmlClass {
  name: string;
  class_uri?: string;
  description?: string;
  title?: Record<string, string>;
  /** `annotations.label`: what a reader calls one of the class, per language (DM-135), e.g. Podujatie. */
  label?: Record<string, string>;
  slots: string[];
  /** The class this one specialises, as LinkML `is_a` (DM-13). */
  is_a?: string;
  /** The classes this one mixes in, as LinkML `mixins`. */
  mixins?: string[];
  /**
   * The slots the class declares inline, as LinkML `attributes`: a Smart Data Model writes most
   * of its fields this way. Kept apart from `slots`, which is the list operations write back.
   */
  attributes?: LinkmlSlot[];
  /** How the class narrows a slot it uses, as LinkML `slot_usage`. */
  slot_usage?: Record<string, SlotUsage>;
  /** The import the class came from, when it is not the model's own. */
  from?: string;
  /**
   * How a table of the class opens (T-5192), from its `view_order_by` (an `orderBy`, `startDate` or
   * `startDate;desc`) and `view_upcoming` (a date slot: rows from today on, the undated last) annotations.
   */
  view?: ClassView;
  /** LinkML `in_subset`: the areas the class belongs to, which the diagram groups by (T-6094). */
  in_subset?: string[];
  /** `annotations.personalData: true` on the class: every slot of it is about a person (DM-120). */
  personal?: boolean;
}

/**
 * How a table of the class is presented (DM-133, DM-134), from its `view_*` annotations as written:
 * the viewer checks each against the slots the Endpoint answers before it uses one.
 */
export interface ClassView {
  orderBy?: string;
  upcoming?: string;
  /** `view_filter`: the default filter, an NGSI-LD `q` that may name `today`, `today+7d`, `today-1d`. */
  filter?: string;
  /** `view_page_length`: rows per page, one of 10, 20, 50, 100. */
  pageLength?: number;
  /** `view_columns`: the slots shown by default, in order. */
  columns?: string[];
}

/** What a column of a slot does in a table (DM-134): the row's title, its time, place, link or image. */
export type SlotRole = "title" | "time" | "place" | "link" | "image";
const SLOT_ROLES = new Set<string>(["title", "time", "place", "link", "image"]);
const PAGE_LENGTHS = new Set([10, 20, 50, 100]);

/** What a class may narrow of a slot it uses (LinkML `slot_usage`); only what it names. */
export type SlotUsage = Partial<Pick<LinkmlSlot, "range" | "required" | "multivalued" | "description" | "label">>;

export interface LinkmlEnumValue {
  name: string;
  meaning?: string;
  description?: string;
  /** What a person reads for the value, per language (UI-86); LinkML's `title`. */
  title?: Record<string, string>;
}

export interface LinkmlEnum {
  name: string;
  permissible_values: LinkmlEnumValue[];
  /** The import the enum came from, when it is not the model's own. */
  from?: string;
}

/** The projection the structured view renders. Never the thing that is saved. */
export interface LinkmlModel {
  id?: string;
  name?: string;
  title?: Record<string, string>;
  prefixes: Record<string, string>;
  default_prefix?: string;
  classes: LinkmlClass[];
  slots: LinkmlSlot[];
  enums: LinkmlEnum[];
  /** The schemas this model imports, as LinkML `imports`. */
  imports?: string[];
  /** The range of a slot that names none, as LinkML `default_range`; `string` when unset. */
  default_range?: string;
}

export interface Diagnostic {
  /** 1-based, so it can be shown next to the line the editor numbers (DM-14). */
  line: number;
  column: number;
  severity: "error" | "warning";
  message: string;
  /** What the message is about, so the structured view can point at the same thing. */
  path?: string;
  /** The relationship rule it breaks, the identifier the server and Model Tools use (DM-68). */
  rule?: RelationshipRule;
}

export const EMPTY_MODEL: LinkmlModel = {
  prefixes: {},
  classes: [],
  slots: [],
  enums: [],
};

/**
 * The parent a type added in the editor takes: `Entity` where the model has it, by importing
 * `ngsi-ld-core` or declaring it, so the type is an NGSI-LD entity type an Endpoint serves (DM-09).
 * Without it Check refused a model built only from "Add type" for having no entity (T-5066).
 */
export function newTypeParent(model: LinkmlModel): { is_a?: string } {
  const hasEntity = (model.imports ?? []).includes("ngsi-ld-core") || model.classes.some((klass) => klass.name === "Entity");
  return hasEntity ? { is_a: "Entity" } : {};
}

/** A new model, as the editor starts one (DM-13). */
export function blankSource(organizationDomain: string, name: string): string {
  const prefix = organizationDomain.split(".")[0] || "org";
  return [
    `id: https://${organizationDomain}/models/${name}`,
    `name: ${name}`,
    "prefixes:",
    `  ${prefix}: https://${organizationDomain}/terms/`,
    "  linkml: https://w3id.org/linkml/",
    `default_prefix: ${prefix}`,
    "imports:",
    "  - linkml:types",
    "  - ngsi-ld-core",
    "classes: {}",
    "slots: {}",
    "enums: {}",
    "",
  ].join("\n");
}

function record(value: unknown): Record<string, unknown> {
  return value && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : {};
}

function text(value: unknown): string | undefined {
  return typeof value === "string" && value.length > 0 ? value : undefined;
}

function languageMap(value: unknown): Record<string, string> | undefined {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return undefined;
  }
  const map: Record<string, string> = {};
  for (const [locale, phrase] of Object.entries(value as Record<string, unknown>)) {
    if (typeof phrase === "string") {
      map[locale] = phrase;
    }
  }
  return Object.keys(map).length > 0 ? map : undefined;
}

/** A boolean annotation, as a plain value or LinkML's `{ tag, value }`. */
function flagged(value: unknown): boolean {
  return value === true || record(value).value === true;
}

function slotOf(name: string, raw: Record<string, unknown>): LinkmlSlot {
  const annotated = record(raw.annotations);
  // An annotation is a plain value or LinkML's `{ tag, value }`, as a published model writes it (T-5192).
  const kind = text(annotated.ngsi_ld_kind) ?? text(record(annotated.ngsi_ld_kind).value);
  const unit = record(raw.unit);
  const mappings = Array.isArray(unit.exact_mappings)
    ? (unit.exact_mappings as unknown[]).filter((m): m is string => typeof m === "string")
    : undefined;
  return {
    name,
    range: text(raw.range),
    required: raw.required === true,
    multivalued: raw.multivalued === true,
    deprecated: raw.deprecated === true,
    description: text(raw.description),
    // LinkML's own `title` is one string: read as English, as an enum value's is (T-4827).
    title: typeof raw.title === "string" && raw.title !== "" ? { en: raw.title } : languageMap(raw.title),
    slot_uri: text(raw.slot_uri),
    unit:
      Object.keys(unit).length > 0
        ? {
            ucum_code: text(unit.ucum_code),
            symbol: text(unit.symbol),
            ...(mappings && mappings.length > 0 ? { exact_mappings: mappings } : {}),
            has_quantity_kind: text(unit.has_quantity_kind),
          }
        : undefined,
    kind: (NGSI_LD_KINDS as readonly string[]).includes(kind ?? "")
      ? (kind as NgsiLdKind)
      : DEFAULT_KIND,
    upstream: text(annotated[UPSTREAM_ANNOTATION]),
    pattern: text(raw.pattern),
    minimum_value: typeof raw.minimum_value === "number" ? raw.minimum_value : undefined,
    subsets: names(raw.subsets),
    maximum_value: typeof raw.maximum_value === "number" ? raw.maximum_value : undefined,
    inverse: text(raw.inverse),
    ...(raw.identifier === true ? { identifier: true } : {}),
    inlined: typeof raw.inlined === "boolean" ? raw.inlined : undefined,
    on_delete: text(annotated.on_delete),
    ...(typeof raw.rank === "number" ? { rank: raw.rank } : {}),
    ...(labelOf(annotated.label) ? { label: labelOf(annotated.label) } : {}),
    ...presentationOf(annotated),
    ...(text(raw.equals_expression) ? { equals_expression: text(raw.equals_expression) } : {}),
    ...(flagged(annotated.personalData) ? { personal: true } : {}),
  };
}

/**
 * `annotations.label` as a language map: LinkML writes an annotation as a plain value or as
 * `{ tag, value }`, and a label per language as a map; a single string is the model's own language.
 */
function labelOf(raw: unknown): Record<string, string> | undefined {
  const value = typeof raw === "object" && raw !== null && "value" in raw ? (raw as { value: unknown }).value : raw;
  if (typeof value === "string" && value.trim().startsWith("{")) {
    // A published model (Model Tools' compiled LinkML) writes the map as JSON text in the value (T-5192).
    try {
      const map = languageMap(JSON.parse(value) as unknown);
      if (map) return map;
    } catch {
      // Not JSON after all: the label is that text.
    }
  }
  if (typeof value === "string") return value.trim() === "" ? undefined : { "": value };
  return languageMap(value);
}

/** A slot's `view_width`, `view_hidden` and `view_role` (DM-134); a value out of range is left out. */
function presentationOf(annotated: Record<string, unknown>): Pick<LinkmlSlot, "width" | "hidden" | "role"> {
  const note = (key: string) => {
    const raw = annotated[key];
    const value = raw && typeof raw === "object" ? record(raw).value : raw;
    return typeof value === "string" || typeof value === "number" || typeof value === "boolean" ? String(value).trim() : undefined;
  };
  const width = Number(note("view_width"));
  const role = note("view_role");
  return {
    ...(Number.isInteger(width) && width >= 8 && width <= 80 ? { width } : {}),
    ...(note("view_hidden") === "true" ? { hidden: true } : {}),
    ...(role && SLOT_ROLES.has(role) ? { role: role as SlotRole } : {}),
  };
}

/** A list of names as the metamodel writes them: a YAML sequence of strings, or nothing. */
function names(value: unknown): string[] | undefined {
  if (!Array.isArray(value)) {
    return undefined;
  }
  const found = (value as unknown[]).filter((one): one is string => typeof one === "string");
  return found.length > 0 ? found : undefined;
}

/** What a `slot_usage` entry narrows: only the fields it writes, so the rest stay the slot's. */
function usageOf(raw: Record<string, unknown>): SlotUsage {
  const usage: SlotUsage = {};
  if (text(raw.range)) usage.range = text(raw.range);
  if (typeof raw.required === "boolean") usage.required = raw.required;
  if (typeof raw.multivalued === "boolean") usage.multivalued = raw.multivalued;
  if (text(raw.description)) usage.description = text(raw.description);
  // A class names its slots in its own words (DM-100): the seeds put each label in `slot_usage`.
  const label = labelOf(record(raw.annotations).label);
  if (label) usage.label = label;
  return usage;
}

/**
 * The last few sources parsed, by their text (T-5598): a space's grid asked the same 96 KB model
 * for its slots, enums, rules, relations and titles, eight YAML parses in one 2 s render on a
 * phone. Every caller reads the model and copies what it changes, so one object serves them all.
 */
const PARSED = new Map<string, LinkmlModel>();
const PARSED_KEPT = 4;

/** The structured projection of a source that parses; `EMPTY_MODEL` for one that does not. */
export function parseModel(source: string): LinkmlModel {
  const kept = PARSED.get(source);
  if (kept !== undefined) return kept;
  const model = parseSource(source);
  if (PARSED.size >= PARSED_KEPT) PARSED.delete(PARSED.keys().next().value as string);
  PARSED.set(source, model);
  return model;
}

function parseSource(source: string): LinkmlModel {
  const document = parseDocument(source);
  if (document.errors.length > 0) {
    return EMPTY_MODEL;
  }
  const root = record(document.toJS({ maxAliasCount: 100 }));

  const prefixes: Record<string, string> = {};
  for (const [prefix, target] of Object.entries(record(root.prefixes))) {
    const expansion =
      typeof target === "string" ? target : text(record(target).prefix_reference);
    if (expansion) {
      prefixes[prefix] = expansion;
    }
  }

  const classes: LinkmlClass[] = Object.entries(record(root.classes)).map(([name, value]) => {
    const raw = record(value);
    const slots = Array.isArray(raw.slots)
      ? (raw.slots as unknown[]).filter((s): s is string => typeof s === "string")
      : [];
    const notes = record(raw.annotations);
    const scalar = (value: unknown) => (typeof value === "number" ? String(value) : text(value));
    const note = (key: string) => scalar(notes[key]) ?? scalar(record(notes[key]).value);
    const length = Number(note("view_page_length"));
    const columns = note("view_columns")?.split(",").map((each) => each.trim()).filter(Boolean);
    const view: ClassView = {
      ...(note("view_order_by") ? { orderBy: note("view_order_by") } : {}),
      ...(note("view_upcoming") ? { upcoming: note("view_upcoming") } : {}),
      ...(note("view_filter") ? { filter: note("view_filter") } : {}),
      ...(PAGE_LENGTHS.has(length) ? { pageLength: length } : {}),
      ...(columns && columns.length > 0 ? { columns } : {}),
    };
    return {
      name,
      ...(Object.keys(view).length > 0 ? { view } : {}),
      class_uri: text(raw.class_uri),
      description: text(raw.description),
      title: languageMap(raw.title),
      ...(labelOf(notes.label) ? { label: labelOf(notes.label) } : {}),
      ...(names(raw.in_subset) ? { in_subset: names(raw.in_subset) } : {}),
      ...(flagged(notes.personalData) ? { personal: true } : {}),
      slots,
      // The hierarchy the model declares (DM-13): read so the editor, the preview and the
      // breaking-change detector see what the YAML says instead of only the flat class list.
      is_a: text(raw.is_a),
      mixins: names(raw.mixins),
      attributes: Object.entries(record(raw.attributes)).map(([slot, value]) => slotOf(slot, record(value))),
      slot_usage: Object.fromEntries(
        Object.entries(record(raw.slot_usage)).map(([slot, value]) => [slot, usageOf(record(value))]),
      ),
    };
  });

  const slots = Object.entries(record(root.slots)).map(([name, value]) =>
    slotOf(name, record(value)),
  );

  const enums: LinkmlEnum[] = Object.entries(record(root.enums)).map(([name, value]) => {
    const raw = record(value);
    const permissible = Object.entries(record(raw.permissible_values)).map(
      ([valueName, entry]) => {
        const details = record(entry);
        const title = text(details.title);
        return {
          name: valueName,
          meaning: text(details.meaning),
          description: text(details.description),
          // A plain string title is the model's one language; a map is read per language.
          // The seeds title a value by `annotations.label` (DM-100), as they do a slot.
          title: title ? { en: title } : (languageMap(details.title) ?? labelOf(record(details.annotations).label)),
        };
      },
    );
    return { name, permissible_values: permissible };
  });

  return {
    id: text(root.id),
    name: text(root.name),
    title: languageMap(root.title),
    prefixes,
    default_prefix: text(root.default_prefix),
    classes,
    slots,
    enums,
    imports: names(root.imports),
    ...(text(root.default_range) ? { default_range: text(root.default_range) } : {}),
  };
}

/** A CURIE resolved against the model's own prefixes; anything else is returned unchanged. */
export function expandIri(iri: string, prefixes: Record<string, string>): string {
  const colon = iri.indexOf(":");
  if (colon <= 0 || iri.startsWith("http://") || iri.startsWith("https://")) {
    return iri;
  }
  const expansion = prefixes[iri.slice(0, colon)];
  return expansion ? `${expansion}${iri.slice(colon + 1)}` : iri;
}

/** The reserved namespace an IRI falls under, or `undefined` where it is the organisation's. */
export function reservedNamespace(
  iri: string,
  prefixes: Record<string, string>,
): string | undefined {
  const expanded = expandIri(iri, prefixes);
  return RESERVED_NAMESPACES.find((namespace) => expanded.startsWith(namespace));
}

/**
 * Every problem the editor can see without compiling: the YAML itself, and the metamodel rules
 * a model has to satisfy before it may be published (DM-04, DM-05, DM-06, DM-14, DM-15, DM-16).
 *
 * `locales` are the organisation's configured languages; a title missing one of them is a
 * warning here and a block at publish time (DM-15).
 */
export function diagnose(
  source: string,
  locales: string[] = [],
  imported: Record<string, LinkmlModel> = {},
): Diagnostic[] {
  const document = parseDocument(source);
  if (document.errors.length > 0) {
    return document.errors.map((error) => ({
      line: error.linePos?.[0]?.line ?? 1,
      column: error.linePos?.[0]?.col ?? 1,
      severity: "error" as const,
      message: error.message,
    }));
  }

  const model = parseModel(source);
  const root = record(document.toJS({ maxAliasCount: 100 }));
  const found: Diagnostic[] = [];
  const at = (path: (string | number)[]): Pick<Diagnostic, "line" | "column"> =>
    positionOf(document, path);

  if (!model.id) {
    found.push({ ...at([]), severity: "error", message: "the model needs an `id`", path: "id" });
  }
  if (!model.name) {
    found.push({
      ...at([]),
      severity: "error",
      message: "the model needs a `name`",
      path: "name",
    });
  }
  if (model.classes.length === 0) {
    found.push({
      ...at([]),
      severity: "warning",
      message: "the model declares no class, so it defines no entity type",
      path: "classes",
    });
  }

  const declared = new Set(model.slots.map((slot) => slot.name));
  // A value structure (a Smart Data Model's L1..L3 parts) is the shape of a JsonProperty's value,
  // never an entity type, so it needs no IRI of its own (owner 2026-11-01, T-6353).
  const structures = valueClasses(model);
  for (const klass of model.classes) {
    if (!klass.class_uri && !structures.has(klass.name)) {
      found.push({
        ...at(["classes", klass.name]),
        severity: "warning",
        message: `class '${klass.name}' has no class_uri, and a model without complete IRI bindings cannot be published`,
        path: `classes.${klass.name}`,
      });
    }
    for (const slot of klass.slots) {
      if (!declared.has(slot)) {
        found.push({
          ...at(["classes", klass.name, "slots"]),
          severity: "error",
          message: `class '${klass.name}' uses slot '${slot}', which the model does not declare`,
          path: `classes.${klass.name}`,
        });
      }
    }
    found.push(
      ...missingLocales(
        klass.title,
        locales,
        `class '${klass.name}'`,
        at(["classes", klass.name]),
        `classes.${klass.name}`,
      ),
    );
  }

  for (const slot of model.slots) {
    const where = at(["slots", slot.name]);
    if (!slot.slot_uri) {
      found.push({
        ...where,
        severity: "warning",
        message: `slot '${slot.name}' has no slot_uri, and a model without complete IRI bindings cannot be published`,
        path: `slots.${slot.name}`,
      });
    } else {
      const reserved = reservedNamespace(slot.slot_uri, model.prefixes);
      if (reserved && !slot.upstream) {
        found.push({
          ...where,
          severity: "error",
          message: `slot '${slot.name}' mints '${slot.slot_uri}' under ${reserved}, which belongs to someone else; use the organisation's own prefix, or cite the upstream term with an ${UPSTREAM_ANNOTATION} annotation`,
          path: `slots.${slot.name}`,
        });
      }
    }
    if (!slot.range) {
      found.push({
        ...where,
        severity: "warning",
        message: `slot '${slot.name}' has no range, so it falls back to the model's default_range`,
        path: `slots.${slot.name}`,
      });
    }
    found.push(...unitProblems(slot, where));
    found.push(
      ...missingLocales(slot.title, locales, `slot '${slot.name}'`, where, `slots.${slot.name}`),
    );
  }

  // One IRI means one thing: two slots bound to it in different units would put two numbers
  // for the same measurement on the wire, and a federated reader could not tell which is right.
  const unitsByIri = new Map<string, LinkmlSlot>();
  for (const slot of model.slots) {
    const code = unitCode(slot.unit);
    if (!slot.slot_uri || code === undefined) continue;
    const first = unitsByIri.get(slot.slot_uri);
    const firstCode = unitCode(first?.unit);
    if (first === undefined) {
      unitsByIri.set(slot.slot_uri, slot);
    } else if (firstCode !== code) {
      found.push({
        ...at(["slots", slot.name]),
        severity: "error",
        message: `slots '${first.name}' and '${slot.name}' are both bound to '${slot.slot_uri}' but measure in ${firstCode ?? ""} and ${code}; one IRI takes one unit`,
        path: `slots.${slot.name}`,
      });
    }
  }

  // The annotation is free text in YAML, so a typo would only surface at generation.
  for (const [name, raw] of Object.entries(record(root.slots))) {
    const kind = text(record(record(raw).annotations).ngsi_ld_kind);
    if (kind && !(NGSI_LD_KINDS as readonly string[]).includes(kind)) {
      found.push({
        ...at(["slots", name]),
        severity: "error",
        message: `slot '${name}' declares ngsi_ld_kind '${kind}', which is not one of ${NGSI_LD_KINDS.join(", ")}`,
        path: `slots.${name}`,
      });
    }
  }

  // Relationships are strict (DM-68): every broken rule is an error that blocks Save, and the
  // server refuses the same list through Model Tools, so this is the early copy, not the gate.
  const { problems, unverified } = relationshipsOf(model, imported);
  for (const problem of problems) {
    found.push({
      ...at(problem.path.split(".")),
      severity: "error",
      message: problem.message,
      path: problem.path,
      rule: problem.rule,
    });
  }
  for (const [path, range] of unverified) {
    found.push({
      ...at(path.split(".")),
      severity: "warning",
      message: `${path.split(".").pop()} points at '${range}', which is no class of this model; the imports it may come from are not loaded here, so saving checks it`,
      path,
    });
  }
  return found;
}

/**
 * The CEFACT common code a unit carries in `exact_mappings` (DM-06): the `ucefact:` mapping,
 * whichever position it has, and the `unece:` spelling older models used.
 */
export function unitCode(unit: LinkmlUnit | undefined): string | undefined {
  const mapping = unit?.exact_mappings?.find((entry) => /^(ucefact|unece):/.test(entry));
  return mapping?.slice(mapping.indexOf(":") + 1) || undefined;
}

const NUMERIC_RANGES = ["integer", "float", "double", "decimal"];

/**
 * A slot name that reads as a measured quantity (DM-06): a number of such a slot without a unit
 * is a number nobody can compare. A heuristic, so what it finds is a warning.
 */
const QUANTITY_NAME =
  /temperature|concentration|speed|velocity|pressure|humidity|precipitation|rainfall|distance|length|height|depth|width|weight|mass|energy|power|volume|duration|flow|level|pm10|pm25|pm2_5|no2|so2|co2|o3/i;

/** What is wrong with a slot's unit, if anything (DM-06, DM-59). */
function unitProblems(slot: LinkmlSlot, where: Pick<Diagnostic, "line" | "column">): Diagnostic[] {
  const path = `slots.${slot.name}`;
  const code = unitCode(slot.unit);
  const numeric = NUMERIC_RANGES.includes(slot.range ?? "");
  if (slot.unit === undefined) {
    return numeric && slot.kind === "Property" && QUANTITY_NAME.test(slot.name)
      ? [
          {
            ...where,
            severity: "warning",
            message: `slot '${slot.name}' is a number that reads as a measured quantity but declares no unit; pick one, or every reader guesses`,
            path,
          },
        ]
      : [];
  }
  const found: Diagnostic[] = [];
  if (code === undefined) {
    found.push({
      ...where,
      severity: "warning",
      message: `slot '${slot.name}' declares a unit without a UN/CEFACT common code, so exports and dashboards cannot label it`,
      path,
    });
  } else if (unitOf(code) === undefined) {
    found.push({
      ...where,
      severity: "error",
      message: `slot '${slot.name}' declares unit '${code}', which is not a UN/CEFACT Recommendation 20 code; pick one from the unit list`,
      path,
    });
  }
  if (!numeric) {
    found.push({
      ...where,
      severity: "error",
      message: `slot '${slot.name}' declares a unit but its range is '${slot.range ?? "string"}'; a unit belongs on a number (integer, float, double or decimal)`,
      path,
    });
  }
  return found;
}

function missingLocales(
  title: Record<string, string> | undefined,
  locales: string[],
  what: string,
  where: Pick<Diagnostic, "line" | "column">,
  path: string,
): Diagnostic[] {
  if (locales.length === 0 || !title) {
    return [];
  }
  const missing = locales.filter((locale) => !title[locale]);
  return missing.length === 0
    ? []
    : [
        {
          ...where,
          severity: "warning",
          message: `${what} has no title in ${missing.join(", ")}`,
          path,
        },
      ];
}

/** Where a path sits in the source, so a message can point at the line it is about. */
export function positionOf(
  document: Document.Parsed,
  path: (string | number)[],
): { line: number; column: number } {
  const node = path.length > 0 ? document.getIn(path, true) : undefined;
  const offset =
    node && typeof node === "object" && "range" in node
      ? (node as { range?: [number, number, number] }).range?.[0]
      : undefined;
  if (offset === undefined) {
    return { line: 1, column: 1 };
  }
  const before = document.toString().slice(0, offset);
  const lines = before.split("\n");
  return { line: lines.length, column: lines[lines.length - 1].length + 1 };
}

/**
 * Applies one structured edit to the source and returns the new source.
 *
 * Every visual edit goes through the YAML document rather than through a re-serialised object,
 * so the user's comments, key order and formatting survive an edit made in the tree (DM-13).
 */
/** What a merge could not take, because the model already had something by that name. */
export interface MergeConflict {
  section: "classes" | "slots" | "enums" | "prefixes";
  name: string;
}

/**
 * A second imported model folded into the one being edited (T-1102, DM-07).
 *
 * Importing replaced the source, so a person could hold one catalogue model at a time and never
 * connect two: a Vehicle and an AirQualityObserved had to live in one model before a slot could
 * relate them. This adds what the incoming model has and the current one does not, and keeps
 * what is already there — a name that exists is reported rather than overwritten, because the
 * version in hand may have been edited and the import must not undo that.
 *
 * The current document is mutated through the YAML AST, so its comments and its order survive.
 */
export function mergeModels(
  current: string,
  incoming: string,
): { source: string; conflicts: MergeConflict[] } {
  const conflicts: MergeConflict[] = [];
  const parsed = parseDocument(incoming);
  if (parsed.errors.length > 0) {
    return { source: current, conflicts };
  }
  const other = record(parsed.toJS({ maxAliasCount: 100 }));
  const merged = edit(current, (document) => {
    for (const section of ["prefixes", "classes", "slots", "enums"] as const) {
      const entries = record(other[section]);
      const held = record(parseDocument(current).toJS({ maxAliasCount: 100 })[section]);
      for (const [name, value] of Object.entries(entries)) {
        if (document.hasIn([section, name])) {
          // Two models declaring the same prefix for the same namespace agree; only a name
          // whose definition differs is something the person has to know was kept.
          if (JSON.stringify(held[name]) !== JSON.stringify(value)) {
            conflicts.push({ section, name });
          }
          continue;
        }
        document.setIn([section, name], value);
      }
    }
  });
  return { source: merged, conflicts };
}

export function edit(source: string, mutate: (document: Document) => void): string {
  const document = parseDocument(source);
  if (document.errors.length > 0) {
    return source;
  }
  mutate(document);
  return document.toString();
}

/** Sets a value, or removes the key when the value is empty. */
export function setOrDelete(
  document: Document,
  path: (string | number)[],
  value: unknown,
): void {
  if (value === undefined || value === "" || value === null || value === false) {
    // A missing parent is not an error: clearing what was never set is a no-op, not a throw.
    if (document.hasIn(path)) {
      document.deleteIn(path);
    }
    return;
  }
  document.setIn(path, value);
}

/**
 * Every slot a class carries: its own, and the ones its parent and its mixins bring (DM-13).
 *
 * A class that specialises another answers with the parent's slots too, so an endpoint
 * projecting by class sees what the entity will actually carry rather than the line the class
 * happens to declare itself (T-1112). A hierarchy that loops is walked once and no further.
 */
export function effectiveSlots(model: LinkmlModel, klass: LinkmlClass): string[] {
  return [...slotOrigins(model, klass).keys()];
}

/**
 * The same slots in the same order, each with the class it comes from (T-6106): the class itself
 * for a slot it names, else the nearest parent or mixin that brought it, so the structure view
 * can grey an inherited field and say where it is defined.
 */
export function slotOrigins(model: LinkmlModel, klass: LinkmlClass): Map<string, string> {
  const byName = new Map(model.classes.map((one) => [one.name, one]));
  const origins = new Map<string, string>();
  const walked = new Set<string>();
  const walk = (current: LinkmlClass): void => {
    if (walked.has(current.name)) {
      return;
    }
    walked.add(current.name);
    // The parent's first: a reader meets the inherited shape before what this class adds.
    const parent = current.is_a === undefined ? undefined : byName.get(current.is_a);
    if (parent !== undefined) {
      walk(parent);
    }
    for (const mixin of current.mixins ?? []) {
      const mixed = byName.get(mixin);
      if (mixed !== undefined) {
        walk(mixed);
      }
    }
    for (const slot of current.slots) {
      if (!origins.has(slot)) {
        origins.set(slot, current.name);
      }
    }
  };
  walk(klass);
  for (const slot of klass.slots) {
    origins.set(slot, klass.name);
  }
  return origins;
}

export type Affordance =
  | "range"
  | "select"
  | "multi-select"
  | "temporal"
  | "geometry"
  | "link"
  | "language-map"
  | "code"
  | "text";

/**
 * How a dashboard may use a slot, derived from its range and kind (DM-20).
 *
 * `enums` are the model's own enum names: a slot whose range is one of them is a select, which
 * the classification could never reach while it judged the slot alone (T-1113).
 *
 * The kind is read before the range, because the four NGSI-LD kinds beyond Property and
 * Relationship say what the value *is* and the range only says what one element of it is
 * (Architecture/11 §1.1, T-1175). Without them a language map and an opaque document both
 * arrived here as `text`, and the editor offered a text filter over a JSON object.
 */
export function slotAffordance(slot: LinkmlSlot, enums: readonly string[] = []): Affordance {
  if (slot.kind === "GeoProperty") {
    return "geometry";
  }
  if (slot.kind === "Relationship") {
    return "link";
  }
  if (slot.kind === "LanguageProperty") {
    // One value per locale: a single text box would edit whichever language it happened to
    // show and silently drop the rest.
    return "language-map";
  }
  if (slot.kind === "JsonProperty") {
    // The model does not describe what is inside, so nothing can be generated for it but a
    // view of the document itself.
    return "code";
  }
  if (slot.kind === "VocabProperty") {
    // A term of a vocabulary, which the model supplies as an enum.
    return "select";
  }
  if (slot.kind === "ListProperty") {
    // Several values at once. A list of terms picks from them; a list of anything else has no
    // set to pick from, so it stays what its elements are.
    return slot.range !== undefined && enums.includes(slot.range) ? "multi-select" : "text";
  }
  if (slot.range !== undefined && enums.includes(slot.range)) {
    return "select";
  }
  if (slot.range === "date" || slot.range === "datetime") {
    return "temporal";
  }
  if (["integer", "float", "double", "decimal"].includes(slot.range ?? "")) {
    return "range";
  }
  return "text";
}

/**
 * What a dashboard does with a slot of this affordance, beside the filter it offers (DM-20):
 * a numeric slot sizes a mark, an enum colours one. The rest carry the dashboard no dimension
 * of their own.
 */
export function slotDimension(affordance: Affordance): "sizeBy" | "colorBy" | undefined {
  if (affordance === "range") {
    return "sizeBy";
  }
  // Only a single pick colours a mark: a multi-select has no one value per entity to colour
  // by, and a language map and a document have none at all.
  return affordance === "select" ? "colorBy" : undefined;
}

/**
 * Every slot a class has, as it has it: the `slots` it lists (narrowed by its `slot_usage`),
 * then its own `attributes`. A listed slot the model does not declare is left out; `diagnose`
 * reports it.
 */
export function classSlots(model: LinkmlModel, klass: LinkmlClass): LinkmlSlot[] {
  const declared = new Map(model.slots.map((slot) => [slot.name, slot]));
  const listed = klass.slots.flatMap((name) => {
    const slot = declared.get(name);
    return slot === undefined ? [] : [{ ...slot, ...klass.slot_usage?.[name] }];
  });
  const own = new Set(listed.map((slot) => slot.name));
  return [...listed, ...(klass.attributes ?? []).filter((slot) => !own.has(slot.name))];
}

/** Where an import points, as the name of a model: `./air.linkml.yaml` and `air` are both `air`. */
export function importName(target: string): string | undefined {
  if (/^[a-z][a-z0-9+.-]*:/i.test(target) && !/^https?:/i.test(target)) {
    // A CURIE such as `linkml:types` names the metamodel's own schemas, not a model of ours.
    return undefined;
  }
  const last = target.split(/[/#?]/).filter((part) => part !== "" && part !== ".").at(-1);
  const name = last?.replace(/\.linkml\.ya?ml$|\.ya?ml$/i, "");
  return name === undefined || name === "" ? undefined : name;
}

/**
 * The model with what it imports: every class, slot and enum of an imported model the model
 * does not declare itself, the classes and enums marked with the import they came from. The
 * model's own declaration wins a name both carry, as LinkML resolves it.
 */
export function withImports(model: LinkmlModel, imported: Record<string, LinkmlModel>): LinkmlModel {
  const classes = [...model.classes];
  const slots = [...model.slots];
  const enums = [...model.enums];
  const has = (list: { name: string }[], name: string) => list.some((one) => one.name === name);
  for (const [from, other] of Object.entries(imported)) {
    for (const klass of other.classes) {
      if (!has(classes, klass.name)) classes.push({ ...klass, from: klass.from ?? from });
    }
    for (const slot of other.slots) {
      if (!has(slots, slot.name)) slots.push(slot);
    }
    for (const entry of other.enums) {
      if (!has(enums, entry.name)) enums.push({ ...entry, from: entry.from ?? from });
    }
  }
  return { ...model, classes, slots, enums };
}

/**
 * One line of a class's box (T-2881): the slot, what its value is, and whether it is the key or
 * a reference to another class, so a reader never has to guess whether `school` is a string.
 */
export interface GraphRow {
  name: string;
  /** The range, `GeoProperty` for a geometry, the model's default range when the slot names none. */
  type: string;
  /** `pk` for the class's identifier, `fk` for a slot whose range is a class. */
  key?: "pk" | "fk";
  /** How many values one entity holds (DM-65): on every reference, and a list's `[]` otherwise. */
  multiplicity?: Multiplicity;
  multivalued?: boolean;
  /** The model declares no identifier: every NGSI-LD entity still has its `id`, a URN. */
  implied?: boolean;
  /** A value object written inside the entity (T-4829): its own fields, drawn in this box. */
  inline?: string[];
  required?: boolean;
  /** The slot, or its whole class, is marked `personalData` (DM-120). */
  personal?: boolean;
}

/**
 * One box of the graph: a class with its own slots, an enum with its values, or (drawn only,
 * never from `graphData`) an import folded into one box that lists its classes and enums.
 */
export interface GraphNode {
  name: string;
  kind: "class" | "enum" | "import";
  /** A class's own slots (listed and inline), without the inherited ones; an enum's values; a folded import's names. */
  slots: string[];
  /** A class's rows: its key first, then every own slot with its type (T-2881). */
  rows?: GraphRow[];
  /** How far down the `is_a` chain it sits; enums one below the deepest class. The drawing lays boxes out by their lines (`diagramLayout.ts`). */
  depth: number;
  /** The import it came from, when it is not the model's own. */
  from?: string;
}

/** One line between two boxes, and why it is there. */
export interface GraphEdge {
  from: string;
  to: string;
  kind: "is_a" | "mixin" | "range" | "enum" | "relationship";
  /** The slot whose range draws the line, for a `range` or an `enum` edge; a relationship's source slot. */
  label?: string;
  /** A relationship's other end: the slot on `to` pointing back (DM-64). */
  inverse?: string;
  cardinality?: Cardinality;
  /**
   * How many `from` entities one `to` entity is joined to, and the other way round (DM-65). A
   * `range` line has no inverse to read the `from` end off, so it says `*`: nothing limits how
   * many entities point at one target.
   */
  fromMultiplicity?: Multiplicity;
  toMultiplicity?: Multiplicity;
}

export type Multiplicity = "1" | "0..1" | "1..*" | "*";

/** The type a row shows for a slot over LinkML `Any`. */
export const ANY_VALUE = "any value";

/** How many entities of the other class one end holds: its `multivalued` and `required` flags. */
export function multiplicity(end: RelationshipEnd): Multiplicity {
  if (end.required) return end.multivalued ? "1..*" : "1";
  return end.multivalued ? "*" : "0..1";
}

/**
 * The model as boxes and the lines between them (DM-13, T-1111, T-2720).
 *
 * Four kinds of line, because a reader asks four different questions of a model: what a class
 * specialises (`is_a`), what it mixes in (`mixins`), which class one of its slots points at (a
 * `range` that names another class), and which enum a slot picks from. A slot whose range is a
 * primitive draws no line: it is inside the box. Enums are boxes of their own, with their values,
 * on a row below the classes.
 *
 * The depth is the length of the `is_a` chain. Where a box is drawn is `diagramLayout.ts`'s
 * business (T-3589): it lays the boxes out by these lines.
 */
export function graphData(model: LinkmlModel): { nodes: GraphNode[]; edges: GraphEdge[] } {
  const byName = new Map(model.classes.map((klass) => [klass.name, klass]));
  const enumNames = new Set(model.enums.map((entry) => entry.name));

  const depthOf = (klass: LinkmlClass, seen: Set<string> = new Set()): number => {
    // A cycle is a model somebody is still editing, not a reason to hang: the chain stops.
    if (klass.is_a === undefined || seen.has(klass.name)) {
      return 0;
    }
    seen.add(klass.name);
    const parent = byName.get(klass.is_a);
    return parent === undefined ? 0 : depthOf(parent, seen) + 1;
  };

  // The key a class has, its own or one it inherits along `is_a`.
  const keyOf = (klass: LinkmlClass, seen: Set<string> = new Set()): LinkmlSlot | undefined => {
    if (seen.has(klass.name)) return undefined;
    seen.add(klass.name);
    const own = classSlots(model, klass).find((slot) => slot.identifier === true);
    const parent = klass.is_a === undefined ? undefined : byName.get(klass.is_a);
    return own ?? (parent === undefined ? undefined : keyOf(parent, seen));
  };
  // `Any` and value objects are no boxes (T-4829): a slot over `Any` takes "any value", and a
  // value object's fields are drawn inside the box of the class that holds it.
  const anyNames = new Set(model.classes.filter(isAnyClass).map((klass) => klass.name));
  const values = valueClasses(model);
  const pointsAt = (slot: LinkmlSlot): boolean =>
    slot.range !== undefined && byName.has(slot.range) && !anyNames.has(slot.range) && !values.has(slot.range);
  const typeOf = (slot: LinkmlSlot): string =>
    slot.kind === "GeoProperty"
      ? "GeoProperty"
      : slot.range !== undefined && anyNames.has(slot.range)
        ? ANY_VALUE
        : (slot.range ?? model.default_range ?? "string");
  const rowsOf = (klass: LinkmlClass): GraphRow[] => {
    const key = keyOf(klass);
    const declared = classSlots(model, klass);
    const known = new Set(declared.map((slot) => slot.name));
    const rows: GraphRow[] = [
      key === undefined
        ? { name: "id", type: "URN", key: "pk", implied: true }
        : { name: key.name, type: typeOf(key), key: "pk" },
    ];
    for (const slot of declared) {
      if (slot.name === key?.name) continue;
      const reference = pointsAt(slot);
      const held = slot.range !== undefined && values.has(slot.range) ? byName.get(slot.range) : undefined;
      rows.push({
        name: slot.name,
        type: typeOf(slot),
        ...(held ? { inline: classSlots(model, held).map((one) => one.name) } : {}),
        ...(reference ? { key: "fk" as const, multiplicity: multiplicity({ class: klass.name, slot: slot.name, multivalued: slot.multivalued === true, required: slot.required === true }) } : {}),
        ...(slot.multivalued === true ? { multivalued: true } : {}),
        ...(slot.required === true ? { required: true } : {}),
        ...(slot.personal === true || klass.personal === true ? { personal: true } : {}),
      });
    }
    // A listed slot the model does not declare is still a field of the class; `diagnose` says why.
    for (const name of klass.slots) {
      if (!known.has(name) && name !== key?.name) rows.push({ name, type: model.default_range ?? "string" });
    }
    return rows;
  };

  const nodes: GraphNode[] = model.classes.filter((klass) => !anyNames.has(klass.name) && !values.has(klass.name)).map((klass) => ({
    name: klass.name,
    kind: "class",
    slots: [...klass.slots, ...(klass.attributes ?? []).map((slot) => slot.name).filter((name) => !klass.slots.includes(name))],
    rows: rowsOf(klass),
    depth: depthOf(klass),
    ...(klass.from ? { from: klass.from } : {}),
  }));
  const enumRow = nodes.length === 0 ? 0 : Math.max(...nodes.map((node) => node.depth)) + 1;
  for (const entry of model.enums) {
    nodes.push({
      name: entry.name,
      kind: "enum",
      slots: entry.permissible_values.map((value) => value.name),
      depth: enumRow,
      ...(entry.from ? { from: entry.from } : {}),
    });
  }

  // A relationship is one line, not one per end; its ends draw no `range` line of their own.
  const pairs = relationshipsOf(model).relationships;
  const ends = new Set(pairs.flatMap((pair) => [pair.source, pair.target].map((end) => `${end.class}.${end.slot}`)));
  const edges: GraphEdge[] = [];
  for (const klass of model.classes) {
    if (anyNames.has(klass.name) || values.has(klass.name)) continue;
    if (klass.is_a !== undefined && byName.has(klass.is_a)) {
      edges.push({ from: klass.name, to: klass.is_a, kind: "is_a" });
    }
    for (const mixin of klass.mixins ?? []) {
      if (byName.has(mixin)) {
        edges.push({ from: klass.name, to: mixin, kind: "mixin" });
      }
    }
    for (const slot of classSlots(model, klass)) {
      if (ends.has(`${klass.name}.${slot.name}`)) {
        continue;
      }
      if (slot.range !== undefined && pointsAt(slot)) {
        edges.push({
          from: klass.name,
          to: slot.range,
          kind: "range",
          label: slot.name,
          toMultiplicity: multiplicity({ class: klass.name, slot: slot.name, multivalued: slot.multivalued === true, required: slot.required === true }),
          fromMultiplicity: "*",
        });
      } else if (slot.range !== undefined && enumNames.has(slot.range)) {
        edges.push({ from: klass.name, to: slot.range, kind: "enum", label: slot.name });
      }
    }
  }
  for (const pair of pairs) {
    edges.push({
      from: pair.source.class,
      to: pair.target.class,
      kind: "relationship",
      label: pair.source.slot,
      inverse: pair.target.slot,
      cardinality: pair.cardinality,
      // At the target's end of the line: how many targets one source holds, and back.
      toMultiplicity: multiplicity(pair.source),
      fromMultiplicity: multiplicity(pair.target),
    });
  }
  return { nodes, edges };
}

/** What happens to the entities referencing one that is deleted (DM-66); `restrict` by default. */
export const ON_DELETE_RULES = ["restrict", "cascade", "set-null"] as const;
export type OnDelete = (typeof ON_DELETE_RULES)[number];

/** Read from the source class to the target class (DM-65). */
export const CARDINALITIES = ["one-to-one", "one-to-many", "many-to-one", "many-to-many"] as const;
export type Cardinality = (typeof CARDINALITIES)[number];

/** The rule identifiers of Architecture/11 §1.2, shared with the save route and Model Tools. */
export type RelationshipRule =
  | "range-not-a-class"
  | "class-range-not-relationship"
  | "primitive-range"
  | "inverse-missing"
  | "inverse-not-reciprocal"
  | "slot-in-two-relationships"
  | "required-on-computed-end"
  | "on-delete-unknown"
  | "on-delete-on-both-ends";

/** One end of a relationship: the class, its slot and what the slot says of the other class. */
export interface RelationshipEnd {
  class: string;
  slot: string;
  multivalued: boolean;
  required: boolean;
}

/** A relationship whose two ends agree (DM-64): what the editor, the diagram and forms read. */
export interface Relationship {
  source: RelationshipEnd;
  target: RelationshipEnd;
  cardinality: Cardinality;
  onDelete: OnDelete;
  /** The end whose entities hold the NGSI-LD Relationship; the other is a query (DM-67). */
  stored: "source" | "target";
}

export interface RelationshipProblem {
  rule: RelationshipRule;
  /** `slots.{name}`, or `classes.{class}.attributes.{name}` for an inline slot. */
  path: string;
  message: string;
}

/** The cardinality two `multivalued` flags make, read from the source (DM-65). */
export function cardinalityOf(sourceMany: boolean, targetMany: boolean): Cardinality {
  if (sourceMany) return targetMany ? "many-to-many" : "one-to-many";
  return targetMany ? "many-to-one" : "one-to-one";
}

/** The two `multivalued` flags of a cardinality: the source slot's and the target slot's. */
export function flagsOf(cardinality: Cardinality): { source: boolean; target: boolean } {
  return {
    source: cardinality === "one-to-many" || cardinality === "many-to-many",
    target: cardinality === "many-to-one" || cardinality === "many-to-many",
  };
}

/** Where a foreign key would be: the "many" side, or the source of 1:1 and N:M (DM-67). */
export function storedEnd(cardinality: Cardinality): "source" | "target" {
  return cardinality === "one-to-many" ? "target" : "source";
}

/** The shipped import every model may name, and the class it brings (DM-09). */
const CORE_IMPORT = "ngsi-ld-core";

/** A slot where one class has it: listed or inline, with its `slot_usage` applied. */
interface Placed {
  owner: string;
  slot: LinkmlSlot;
  path: string;
  /** Declaration order, which decides the source of a pair no `on_delete` marks (DM-64). */
  order: number;
}

/**
 * Every relationship of the model and every rule it breaks (DM-64…DM-69).
 *
 * `imported` are the models the model imports, by import name, as far as the caller has them.
 * A range naming no class the model can see is an error when every import is in hand, and is
 * returned as `unverified` otherwise: the save route resolves the imports and decides.
 */
export function relationshipsOf(
  model: LinkmlModel,
  imported: Record<string, LinkmlModel> = {},
): { relationships: Relationship[]; problems: RelationshipProblem[]; unverified: [string, string][] } {
  const all = withImports(model, imported);
  const classes = new Set(all.classes.map((klass) => klass.name));
  const imports = (model.imports ?? []).map(importName).filter((name): name is string => name !== undefined);
  if (imports.includes(CORE_IMPORT)) classes.add("Entity");
  const complete = imports.every((name) => name === CORE_IMPORT || imported[name] !== undefined);
  const enums = new Set(all.enums.map((entry) => entry.name));
  const primitives = new Set<string>(RANGES);

  // Where each slot sits. A declared slot listed by two classes is one slot with two owners.
  const declaredOrder = new Map(model.slots.map((slot, index) => [slot.name, index]));
  const listedBy = new Map<string, string[]>();
  for (const klass of model.classes) {
    for (const name of klass.slots) listedBy.set(name, [...(listedBy.get(name) ?? []), klass.name]);
  }
  const placedIn = (klass: LinkmlClass, order: (name: string, inline: boolean) => number): Placed[] =>
    classSlots(all, klass).map((slot) => {
      const inline = !klass.slots.includes(slot.name);
      return {
        owner: klass.name,
        slot,
        path: inline ? `classes.${klass.name}.attributes.${slot.name}` : `slots.${slot.name}`,
        order: order(slot.name, inline),
      };
    });
  let inlineOrder = model.slots.length;
  const own: Placed[] = model.classes.flatMap((klass) =>
    placedIn(klass, (name, inline) => (inline ? inlineOrder++ : (declaredOrder.get(name) ?? 0))),
  );
  const endOn = (klass: string, name: string): Placed | undefined => {
    const holder = all.classes.find((one) => one.name === klass);
    return holder === undefined ? undefined : placedIn(holder, () => Number.MAX_SAFE_INTEGER).find((one) => one.slot.name === name);
  };

  const problems: RelationshipProblem[] = [];
  const unverified: [string, string][] = [];
  const relationships: Relationship[] = [];
  const seen = new Set<string>();
  const reported = new Set<string>();
  const problem = (rule: RelationshipRule, path: string, message: string) => {
    const key = `${rule}|${path}`;
    if (!reported.has(key)) {
      reported.add(key);
      problems.push({ rule, path, message });
    }
  };

  for (const end of own) {
    const { slot, owner, path } = end;
    const range = slot.range;
    const classRange = range !== undefined && classes.has(range);
    if (slot.kind !== "Relationship") {
      // A nested value's class describes its shape (the importer's `address`), no entity (DM-68).
      if (classRange && slot.kind !== "JsonProperty" && slot.inlined !== true) {
        problem(
          "class-range-not-relationship",
          path,
          `${slot.name} on ${owner} points at the class ${range} but is a ${slot.kind}; a slot whose range is a class is a Relationship`,
        );
      }
      continue;
    }
    // An external reference: the id of an entity outside the model, with nothing to check.
    if (range === undefined || range === "uriorcurie") {
      if (slot.inverse !== undefined) {
        problem(
          "range-not-a-class",
          path,
          `${slot.name} on ${owner} names the inverse ${slot.inverse} but its range is no class; name the class it points at`,
        );
      }
      continue;
    }
    if (primitives.has(range) || enums.has(range)) {
      problem(
        "primitive-range",
        path,
        `${slot.name} on ${owner} is a Relationship with the range ${range}; a Relationship points at a class, or at uriorcurie for an entity outside the model`,
      );
      continue;
    }
    if (!classRange) {
      if (complete) {
        problem("range-not-a-class", path, `${slot.name} on ${owner} points at ${range}, which is no class of this model or of its imports`);
      } else {
        unverified.push([path, range]);
      }
      continue;
    }
    if (slot.on_delete !== undefined && !(ON_DELETE_RULES as readonly string[]).includes(slot.on_delete)) {
      problem("on-delete-unknown", path, `${slot.name} on ${owner} has on_delete '${slot.on_delete}'; it is one of ${ON_DELETE_RULES.join(", ")}`);
    }
    if (!path.startsWith("classes.") && (listedBy.get(slot.name) ?? []).length > 1) {
      problem(
        "slot-in-two-relationships",
        path,
        `${slot.name} is used by ${(listedBy.get(slot.name) ?? []).join(" and ")}, so it would be an end of two relationships; give each class a slot of its own`,
      );
      continue;
    }
    if (slot.inverse === undefined) {
      problem(
        "inverse-missing",
        path,
        `${slot.name} (${owner} → ${range}) names no inverse; add the slot on ${range} that points back`,
      );
      continue;
    }
    const other = endOn(range, slot.inverse);
    if (other === undefined) {
      const holder = all.classes.find((one) => one.name === range);
      problem(
        "inverse-missing",
        path,
        holder?.from !== undefined
          ? `${slot.name} names the inverse ${slot.inverse}, which ${range} from ${holder.from} does not have; declare the relationship in that model, or make ${slot.name} an external reference`
          : `${slot.name} names the inverse ${slot.inverse}, which ${range} does not have`,
      );
      continue;
    }
    if (other.slot.name === slot.name && other.owner === owner) {
      problem("inverse-not-reciprocal", path, `${slot.name} names itself as its inverse; the other end is a slot of its own`);
      continue;
    }
    if (other.slot.kind !== "Relationship" || other.slot.range !== owner || other.slot.inverse !== slot.name) {
      problem(
        "inverse-not-reciprocal",
        path,
        `${slot.name} (${owner} → ${range}) names ${range}.${slot.inverse} as its inverse, which ${
          other.slot.kind !== "Relationship"
            ? "is not a Relationship"
            : other.slot.range !== owner
              ? `points at ${other.slot.range ?? "nothing"}, not at ${owner}`
              : `names ${other.slot.inverse ?? "no inverse"} back, not ${slot.name}`
        }`,
      );
      continue;
    }
    const key = [`${owner}.${slot.name}`, `${range}.${other.slot.name}`].sort().join("|");
    if (seen.has(key)) continue;
    seen.add(key);

    const otherOrder = own.find((one) => one.owner === range && one.slot.name === other.slot.name)?.order ?? Number.MAX_SAFE_INTEGER;
    if (slot.on_delete !== undefined && other.slot.on_delete !== undefined) {
      problem(
        "on-delete-on-both-ends",
        path,
        `both ${slot.name} and ${other.slot.name} carry on_delete; it goes on the source end only`,
      );
      continue;
    }
    const thisIsSource =
      slot.on_delete !== undefined || (other.slot.on_delete === undefined && end.order <= otherOrder);
    const [source, target] = thisIsSource ? [end, other] : [other, end];
    const cardinality = cardinalityOf(source.slot.multivalued === true, target.slot.multivalued === true);
    const stored = storedEnd(cardinality);
    const computed = stored === "source" ? target : source;
    if (computed.slot.required) {
      problem(
        "required-on-computed-end",
        computed.path,
        `${computed.slot.name} on ${computed.owner} is computed from ${(stored === "source" ? source : target).slot.name} and cannot be required; make the stored end required instead`,
      );
    }
    const onDelete = (source.slot.on_delete ?? "restrict") as OnDelete;
    relationships.push({
      source: { class: source.owner, slot: source.slot.name, multivalued: source.slot.multivalued === true, required: source.slot.required === true },
      target: { class: target.owner, slot: target.slot.name, multivalued: target.slot.multivalued === true, required: target.slot.required === true },
      cardinality,
      onDelete: (ON_DELETE_RULES as readonly string[]).includes(onDelete) ? onDelete : "restrict",
      stored,
    });
  }
  return { relationships, problems, unverified };
}

/** The relationships whose two ends agree; a broken one is a diagnostic, not a relationship. */
export function relationships(model: LinkmlModel, imported: Record<string, LinkmlModel> = {}): Relationship[] {
  return relationshipsOf(model, imported).relationships;
}
