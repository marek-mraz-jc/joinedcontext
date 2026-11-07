/**
 * A form view of a space's type (T-3103, ADR-N-042 §3.2, API/01 §30): the fields come from the
 * type's LinkML class (order, label, help, required), the view's `settings` choose, order and
 * relabel them, a condition shows a field only while another field holds a value, and the page's
 * URL may prefill the fields the form asks for. A submission is one NGSI-LD entity; who may create
 * it is the gateway's to decide.
 */
import type { LinkmlSlot } from "../models/linkml";

/** How a field is asked for. */
export type FieldKind =
  | "text"
  | "integer"
  | "number"
  | "boolean"
  | "enum"
  | "date"
  | "datetime"
  | "language"
  | "relationship"
  | "point";

export interface FieldOption {
  value: string;
  title?: string;
}

export interface FormField {
  attr: string;
  label: string;
  help?: string;
  required: boolean;
  kind: FieldKind;
  options?: FieldOption[];
  /** The entity type a relationship points to, when the model names it. */
  target?: string;
}

/** One field as a view's settings name it. */
export interface FieldSetting {
  attr: string;
  label?: string;
  help?: string;
  required?: boolean;
}

/** `attr` is shown only while the answer to `when.attr` equals `when.equals`. */
export interface Condition {
  attr: string;
  when: { attr: string; equals: string };
}

export interface FormSettings {
  fields?: FieldSetting[];
  conditions?: Condition[];
  prefill?: boolean;
}

/** The answers, as typed: a point is two answers, `{attr}.lon` and `{attr}.lat`. */
export type Answers = Record<string, string>;

/** NGSI-LD kinds a form cannot ask for as one field. */
const UNASKED = new Set(["ListProperty", "JsonProperty", "VocabProperty"]);
const NUMBERS: Record<string, FieldKind> = {
  integer: "integer",
  float: "number",
  double: "number",
  decimal: "number",
};
/** An NGSI-LD entity id, `urn:ngsi-ld:{Type}:{nss}`: no segment of the nss says where it lives (ADR-N-041). */
export const ENTITY_ID = /^urn:ngsi-ld:[A-Za-z][A-Za-z0-9_-]*:\S+$/;

function kindOf(slot: LinkmlSlot, options: FieldOption[] | undefined): FieldKind {
  if (slot.kind === "Relationship") return "relationship";
  if (slot.kind === "LanguageProperty") return "language";
  if (slot.kind === "GeoProperty") return "point";
  if (options && options.length > 0) return "enum";
  const range = (slot.range ?? "").toLowerCase();
  if (NUMBERS[range]) return NUMBERS[range];
  if (range === "boolean") return "boolean";
  if (range === "date") return "date";
  if (range === "datetime") return "datetime";
  return "text";
}

/** Whether a slot can be a field: not the key, not deprecated, not a list or a JSON blob. */
export function askable(slot: LinkmlSlot): boolean {
  return !slot.identifier && !slot.deprecated && !slot.multivalued && !UNASKED.has(slot.kind);
}

/**
 * The form's fields: the settings' own, in their order, or every askable slot of the class in the
 * model's order. A setting naming an attribute the class does not have is left out; `required`
 * can only add to what the model requires.
 */
export function fieldsOf(
  slots: LinkmlSlot[],
  enums: Record<string, FieldOption[]>,
  settings: FormSettings | undefined,
  locale: string,
): FormField[] {
  const bySlot = new Map(slots.filter(askable).map((slot) => [slot.name, slot]));
  const chosen: FieldSetting[] = settings?.fields?.length ? settings.fields : [...bySlot.keys()].map((attr) => ({ attr }));
  const seen = new Set<string>();
  const fields: FormField[] = [];
  for (const setting of chosen) {
    const slot = bySlot.get(setting.attr);
    if (!slot || seen.has(slot.name)) continue;
    seen.add(slot.name);
    const options = enums[slot.name];
    // The model's label for the slot (DM-100) before its title, in the reader's language first.
    const lang = locale.slice(0, 2);
    const title = slot.label?.[lang] ?? slot.title?.[lang] ?? slot.label?.en ?? slot.title?.en ?? slot.label?.[""];
    fields.push({
      attr: slot.name,
      label: setting.label?.trim() || title || slot.name,
      help: setting.help?.trim() || slot.description || undefined,
      required: Boolean(slot.required) || Boolean(setting.required),
      kind: kindOf(slot, options),
      ...(options && options.length > 0 ? { options } : {}),
      ...(slot.kind === "Relationship" && slot.range ? { target: slot.range } : {}),
    });
  }
  return fields;
}

/** The fields shown for these answers: a field under a condition only while it holds. */
export function visibleFields(fields: FormField[], conditions: Condition[] | undefined, answers: Answers): FormField[] {
  return fields.filter((field) =>
    (conditions ?? [])
      .filter((condition) => condition.attr === field.attr)
      .every((condition) => (answers[condition.when.attr] ?? "") === condition.when.equals),
  );
}

/** The answers the page's URL gives the fields the form asks for, when the form takes them. */
export function prefilled(fields: FormField[], search: URLSearchParams, settings: FormSettings | undefined): Answers {
  if (settings && settings.prefill === false) return {};
  const answers: Answers = {};
  for (const field of fields) {
    if (field.kind === "point") {
      for (const part of ["lon", "lat"]) {
        const value = search.get(`${field.attr}.${part}`);
        if (value !== null) answers[`${field.attr}.${part}`] = value;
      }
      continue;
    }
    const value = search.get(field.attr);
    if (value === null) continue;
    // A prefilled choice outside the field's options would be a value the form cannot show.
    if (field.kind === "enum" && !field.options?.some((option) => option.value === value)) continue;
    answers[field.attr] = value;
  }
  return answers;
}

/** Why an answer cannot be sent. */
export type Problem = "required" | "integer" | "number" | "urn" | "point" | "date";

function blank(value: string | undefined): boolean {
  return value === undefined || value.trim() === "";
}

/** The problems of the shown fields, by attribute; an empty map means the form can be sent. */
export function problemsOf(fields: FormField[], answers: Answers): Record<string, Problem> {
  const problems: Record<string, Problem> = {};
  for (const field of fields) {
    if (field.kind === "point") {
      const [lon, lat] = [answers[`${field.attr}.lon`], answers[`${field.attr}.lat`]];
      if (blank(lon) && blank(lat)) {
        if (field.required) problems[field.attr] = "required";
        continue;
      }
      const x = Number(lon);
      const y = Number(lat);
      if (blank(lon) || blank(lat) || !Number.isFinite(x) || !Number.isFinite(y) || Math.abs(x) > 180 || Math.abs(y) > 90) {
        problems[field.attr] = "point";
      }
      continue;
    }
    const value = answers[field.attr];
    // A checkbox is always an answer: unticked is `false`.
    if (field.kind === "boolean") continue;
    if (blank(value)) {
      if (field.required) problems[field.attr] = "required";
      continue;
    }
    const text = (value as string).trim();
    if (field.kind === "integer" && !/^-?\d+$/.test(text)) problems[field.attr] = "integer";
    if (field.kind === "number" && !Number.isFinite(Number(text))) problems[field.attr] = "number";
    if (field.kind === "relationship" && !ENTITY_ID.test(text)) problems[field.attr] = "urn";
    if (field.kind === "date" && !/^\d{4}-\d{2}-\d{2}$/.test(text)) problems[field.attr] = "date";
    if (field.kind === "datetime" && Number.isNaN(Date.parse(text))) problems[field.attr] = "date";
  }
  return problems;
}

/** A fresh id under the type (ADR-N-041: any NSS; the gateway decides the write). */
export function newId(type: string): string {
  return `urn:ngsi-ld:${type}:${crypto.randomUUID()}`;
}

/** The NGSI-LD entity of the shown fields' answers; a field left blank is not sent. */
export function entityOf(type: string, fields: FormField[], answers: Answers, locale: string, id = newId(type)): Record<string, unknown> {
  const entity: Record<string, unknown> = { id, type };
  for (const field of fields) {
    if (field.kind === "point") {
      const lon = answers[`${field.attr}.lon`];
      const lat = answers[`${field.attr}.lat`];
      if (blank(lon) || blank(lat)) continue;
      entity[field.attr] = { type: "GeoProperty", value: { type: "Point", coordinates: [Number(lon), Number(lat)] } };
      continue;
    }
    const value = answers[field.attr];
    if (field.kind === "boolean") {
      entity[field.attr] = { type: "Property", value: value === "true" };
      continue;
    }
    if (blank(value)) continue;
    const text = (value as string).trim();
    switch (field.kind) {
      case "integer":
      case "number":
        entity[field.attr] = { type: "Property", value: Number(text) };
        break;
      case "language":
        entity[field.attr] = { type: "LanguageProperty", languageMap: { [locale.slice(0, 2)]: text } };
        break;
      case "relationship":
        entity[field.attr] = { type: "Relationship", object: text };
        break;
      case "datetime":
        entity[field.attr] = { type: "Property", value: { "@type": "DateTime", "@value": new Date(text).toISOString() } };
        break;
      case "date":
        entity[field.attr] = { type: "Property", value: { "@type": "Date", "@value": text } };
        break;
      default:
        entity[field.attr] = { type: "Property", value: text };
    }
  }
  return entity;
}

interface SchemaProperty {
  description?: string;
  title?: string;
  type?: string | string[];
  format?: string;
  enum?: unknown[];
  $ref?: string;
  "x-ngsi-ld-kind"?: string;
}

/** The members every entity carries, which the form never asks for. */
const OWN = new Set(["id", "type", "@context", "observedAt", "createdAt", "modifiedAt", "dataProvider", "source"]);

/**
 * The fields of a public form, from the type's definition in the Endpoint's published schema
 * (API/01 §33): what the Endpoint lets the public see of the type, required where the schema
 * requires it, labelled by the schema's title or the attribute and helped by its description.
 */
export function fieldsOfSchema(definition: unknown, defs: Record<string, unknown>): FormField[] {
  const document = (definition ?? {}) as { properties?: Record<string, SchemaProperty>; required?: string[] };
  const required = new Set(document.required ?? []);
  const fields: FormField[] = [];
  for (const [attr, property] of Object.entries(document.properties ?? {})) {
    if (OWN.has(attr)) continue;
    const kind = property["x-ngsi-ld-kind"] ?? "Property";
    if (UNASKED.has(kind)) continue;
    const types = Array.isArray(property.type) ? property.type : [property.type];
    if (types.includes("array")) continue;
    const referenced = property.$ref ? (defs[property.$ref.split("/").pop() ?? ""] as SchemaProperty | undefined) : undefined;
    const values = (property.enum ?? referenced?.enum ?? []).filter((value): value is string => typeof value === "string");
    const options = values.length > 0 ? values.map((value) => ({ value })) : undefined;
    let fieldKind: FieldKind = "text";
    if (kind === "Relationship") fieldKind = "relationship";
    else if (kind === "LanguageProperty") fieldKind = "language";
    else if (kind === "GeoProperty") fieldKind = "point";
    else if (options) fieldKind = "enum";
    else if (types.includes("integer")) fieldKind = "integer";
    else if (types.includes("number")) fieldKind = "number";
    else if (types.includes("boolean")) fieldKind = "boolean";
    else if (property.format === "date") fieldKind = "date";
    else if (property.format === "date-time") fieldKind = "datetime";
    fields.push({
      attr,
      label: property.title?.trim() || attr,
      help: property.description?.trim() || undefined,
      required: required.has(attr),
      kind: fieldKind,
      ...(options ? { options } : {}),
    });
  }
  return fields;
}

/**
 * The name of a public form's trap field: a person never sees it, a bot fills it, and the
 * gateway refuses the create because the form's Policy does not grant it (T-3172).
 */
export function trapName(fields: FormField[]): string {
  return ["website", "homepageUrl", "contactUrl"].find((name) => !fields.some((field) => field.attr === name)) ?? "trapField";
}

/** The id a create's `Location` names, or none when the answer carries no such header. */
export function createdId(location: string | null): string | undefined {
  const at = location?.lastIndexOf("/entities/") ?? -1;
  if (!location || at < 0) return undefined;
  const id = location.slice(at + "/entities/".length).split(/[?#]/)[0];
  return id ? decodeURIComponent(id) : undefined;
}

/** The message a form's page sends its parent when its height changes (API/01 §33). */
export const FORM_HEIGHT_MESSAGE = "jc-form-height";
