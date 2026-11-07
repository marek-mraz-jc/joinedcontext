/**
 * The `spec.representations.ical` mapping as the Endpoint form edits it (EP-125, API/02 §8b). The
 * rules and the sentences that refuse a mapping are jc-core's: the form's dry run answers them.
 * This file holds only what the table needs to draw: the fields, which are required or fixed, and
 * what each preset maps, as `schemas/surface-requirements.md` in the platform lists them.
 */

/** A field's source: an attribute name, or the object form with remaps (written in YAML). */
export type IcalSource = string | Record<string, unknown>;

export interface IcalMapping {
  type?: string;
  preset?: string;
  mapping?: Record<string, IcalSource>;
}

export const ICAL_PRESETS = ["sdm-event", "schema-org"] as const;

/** The VEVENT properties in table order; `fixed` ones the platform sets (UID, DTSTAMP). */
export const ICAL_FIELDS: { field: string; required: boolean; fixed?: string }[] = [
  { field: "UID", required: true, fixed: "id" },
  { field: "DTSTAMP", required: true, fixed: "modifiedAt" },
  { field: "DTSTART", required: true },
  { field: "SUMMARY", required: true },
  { field: "DTEND", required: false },
  { field: "DURATION", required: false },
  { field: "DESCRIPTION", required: false },
  { field: "LOCATION", required: false },
  { field: "GEO", required: false },
  { field: "URL", required: false },
  { field: "CATEGORIES", required: false },
  { field: "STATUS", required: false },
  { field: "ORGANIZER", required: false },
  { field: "LAST-MODIFIED", required: false },
];

const SDM_EVENT: Record<string, string> = {
  DTSTART: "startDate",
  DTEND: "endDate",
  SUMMARY: "name",
  DESCRIPTION: "description",
  LOCATION: "address",
  GEO: "location",
  URL: "url",
};

/** What a preset maps each field from, by attribute name (STATUS of schema-org is mapped). */
export function presetSources(preset: string | undefined): Record<string, string> {
  if (preset === "schema-org") return { ...SDM_EVENT, STATUS: "eventStatus" };
  return preset === "sdm-event" ? SDM_EVENT : {};
}

/** Attributes every NGSI-LD entity has; a model never declares them. */
export const CORE_ATTRIBUTES = ["id", "type", "createdAt", "modifiedAt", "location", "observationSpace", "operationSpace"];

/** The preset the mapping starts from: none written at all means `sdm-event` (EP-125). */
export function baseOf(value: IcalMapping): string | undefined {
  if (value.preset) return value.preset;
  return Object.keys(value.mapping ?? {}).length === 0 ? "sdm-event" : undefined;
}

/** The mapping written back: `undefined` when it says nothing the default does not. */
function tidy(value: IcalMapping): IcalMapping | undefined {
  const mapping = value.mapping && Object.keys(value.mapping).length > 0 ? value.mapping : undefined;
  const type = value.type && value.type !== "Event" ? value.type : undefined;
  // `sdm-event` with nothing over it is what an Endpoint with no mapping serves.
  const preset = value.preset === "sdm-event" && !mapping ? undefined : value.preset;
  const next: IcalMapping = { ...(type ? { type } : {}), ...(preset ? { preset } : {}), ...(mapping ? { mapping } : {}) };
  return Object.keys(next).length > 0 ? next : undefined;
}

/** A field set to `attribute`, or back to the preset's (or to nothing) when emptied. */
export function withSource(value: IcalMapping, field: string, attribute: string): IcalMapping | undefined {
  const mapping = { ...(value.mapping ?? {}) };
  if (attribute.trim() === "") delete mapping[field];
  else mapping[field] = attribute.trim();
  // An override on the default names its preset, or it would read as a whole mapping of its own.
  return tidy({ ...value, preset: value.preset ?? baseOf(value), mapping });
}

/**
 * The start the person picks: a preset keeps the fields they wrote; "own" (`""`) writes the base
 * preset's required fields out, so the mapping stays one of their own and not the default.
 */
export function withPreset(value: IcalMapping, preset: string): IcalMapping | undefined {
  if (preset !== "") return tidy({ ...value, preset });
  const base = presetSources(baseOf(value));
  const mapping: Record<string, IcalSource> = { ...(value.mapping ?? {}) };
  for (const { field, required, fixed } of ICAL_FIELDS) {
    if (required && !fixed && mapping[field] === undefined && base[field]) mapping[field] = base[field];
  }
  return tidy({ ...value, preset: undefined, mapping });
}

export function withType(value: IcalMapping, type: string): IcalMapping | undefined {
  return tidy({ ...value, type: type.trim() || undefined });
}

/** The required fields the mapping in force leaves without a source; the server says the rest. */
export function unmappedRequired(value: IcalMapping): string[] {
  const base = presetSources(baseOf(value));
  return ICAL_FIELDS.filter(({ required, fixed }) => required && !fixed)
    .map(({ field }) => field)
    .filter((field) => value.mapping?.[field] === undefined && base[field] === undefined);
}
