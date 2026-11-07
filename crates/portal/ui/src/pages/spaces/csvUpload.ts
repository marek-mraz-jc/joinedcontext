/**
 * T-3840: which Smart Data Models type a CSV most likely is. The catalogue's attribute names
 * (filled for {@link CITY_SUBJECTS}) are matched against the file's columns by name up to case,
 * separators and diacritics, or a common synonym; the attributes every type carries (`name`,
 * `address`, `location`, …) say nothing about which type it is and do not score.
 */
import { LATITUDES, LONGITUDES, normalized, SYNONYMS } from "./importRows";

/** The catalogue subjects the upload fills and ranks against: the city data a CSV usually is. */
export const CITY_SUBJECTS = [
  "dataModel.Parking",
  "dataModel.Environment",
  "dataModel.Weather",
  "dataModel.Transportation",
  "dataModel.PointOfInterest",
  "dataModel.UrbanMobility",
  "dataModel.Building",
  "dataModel.WasteManagement",
] as const;

/** One model of the catalogue index, as `GET /api/v1/tools/sdm-catalog` lists it. */
export interface CatalogueModel {
  id: string;
  name: string;
  description?: string;
  attributes: string[];
}

/** A type the file may be: its score, and the column each of its attributes takes. */
export interface SdmMatch {
  model: CatalogueModel;
  /** The share of the file's own columns (the generic ones aside) the type names, 0..1. */
  score: number;
  /** column → attribute, for every column the type has a place for. */
  mapping: Record<string, string>;
}

/** Attributes most types declare: they map a column but do not tell one type from another. */
const GENERIC = new Set(
  ["id", "type", "name", "alternateName", "description", "address", "location", "dateCreated", "dateModified", "source", "dataProvider", "seeAlso", "owner", "areaServed"].map(
    normalized,
  ),
);

/** A type is offered only when it names this share of the file's own columns, and two of them. */
export const MIN_SCORE = 0.4;
const MIN_SPECIFIC = 2;

function attributeFor(column: string, attributes: Map<string, string>): string | undefined {
  const key = normalized(column);
  if (LATITUDES.has(key) || LONGITUDES.has(key)) return attributes.get("location");
  const direct = attributes.get(key);
  if (direct) return direct;
  for (const [attribute, synonyms] of Object.entries(SYNONYMS)) {
    if (synonyms.some((synonym) => normalized(synonym) === key)) return attributes.get(normalized(attribute));
  }
  return undefined;
}

/**
 * The best two types for these columns, best first, each above {@link MIN_SCORE}; none when no
 * type names enough of the file, and the file then gets its own model.
 */
export function rankModels(columns: string[], models: CatalogueModel[]): SdmMatch[] {
  const specific = columns.filter((column) => {
    const key = normalized(column);
    return !GENERIC.has(key) && !LATITUDES.has(key) && !LONGITUDES.has(key) && key !== "urn";
  });
  if (specific.length === 0) return [];
  const matches: (SdmMatch & { matched: number })[] = [];
  for (const model of models) {
    if (model.attributes.length === 0) continue;
    const attributes = new Map(model.attributes.map((attribute) => [normalized(attribute), attribute]));
    const mapping: Record<string, string> = {};
    let matched = 0;
    for (const column of columns) {
      const attribute = attributeFor(column, attributes);
      if (!attribute) continue;
      mapping[column] = attribute;
      if (specific.includes(column)) matched += 1;
    }
    const score = matched / specific.length;
    if (matched >= MIN_SPECIFIC && score >= MIN_SCORE) matches.push({ model, score, mapping, matched });
  }
  // Ties go to the type the file covers more of, then to the name, so the order is stable.
  matches.sort(
    (a, b) =>
      b.score - a.score ||
      b.matched / b.model.attributes.length - a.matched / a.model.attributes.length ||
      a.model.id.localeCompare(b.model.id),
  );
  return matches.slice(0, 2).map(({ model, score, mapping }) => ({ model, score, mapping }));
}

/** Every model of a catalogue index, flattened from its subjects. */
export function modelsOf(catalogue: { subjects?: { models?: CatalogueModel[] }[] } | undefined): CatalogueModel[] {
  return (catalogue?.subjects ?? []).flatMap((subject) => subject.models ?? []);
}

/** The op's files total at most 10 MiB (DM-55): the sample keeps room for the model beside it. */
export const SAMPLE_BYTES = 8 * 1024 * 1024;

/** What Complete this space takes at once, the op's own limit (DM-55). */
export const MAX_FILES_BYTES = 10 * 1024 * 1024;

/**
 * The entity type a file's rows get, in ASCII PascalCase with its diacritics kept as their letters
 * (`Kultúrne miesta.csv` is KulturneMiesta), singular as the op makes it; none when no type comes of it.
 */
export function typeNameOf(fileName: string): string | undefined {
  const file = fileName.replace(/\.[^.]+$/, "");
  const stem = /^\p{ASCII}*[^siu]s$/u.test(file) ? file.slice(0, -1) : file;
  const words = stem.normalize("NFD").replace(/[\u0300-\u036f]/g, "").split(/[^A-Za-z0-9]+/);
  const name = words.map((word) => word.charAt(0).toUpperCase() + word.slice(1)).join("");
  return /^[A-Z][A-Za-z0-9]{0,62}$/.test(name) && name !== "Entity" ? name : undefined;
}

/** What a table is called: a workbook sheet's own name unless it is a default one, else the file's stem. */
export function tableNameOf(fileName: string, sheet?: string): string {
  const own = fileName.toLowerCase().endsWith(".xlsx") ? (sheet?.trim() ?? "") : "";
  return own && !/^(sheet|list|h[aá]rok|taulukko|tabelle|blatt|feuil)\s*\d*$/iu.test(own) ? own : fileName.replace(/\.[^.]+$/, "");
}

/** A name for the space a table makes: its name as a DNS-1123 label, the person may change it. */
export function spaceNameOf(name: string): string {
  const stem = name.replace(/\.(csv|tsv|txt|xlsx|json|geojson)$/i, "").normalize("NFD").replace(/\p{M}/gu, "").toLowerCase();
  return stem.replace(/[^a-z0-9]+/g, "-").slice(0, 40).replace(/^-+|-+$/g, "");
}

/** The head of a CSV no longer than `max` UTF-8 bytes, cut after a whole line. */
export function sampleOf(text: string, max = SAMPLE_BYTES): string {
  const bytes = new TextEncoder().encode(text);
  if (bytes.length <= max) return text;
  const head = new TextDecoder().decode(bytes.subarray(0, max));
  const end = head.lastIndexOf("\n");
  return end < 0 ? "" : head.slice(0, end + 1);
}

/**
 * The `jc_space_complete` input of an upload (DM-81): the sample, the Smart Data Models type's
 * LinkML when one was chosen, the type, and `upload` so the space gets its Endpoint and the
 * person the grant to write the rows.
 */
export function uploadInput(input: {
  space: string;
  fileName: string;
  sample: string;
  typeName: string;
  linkml?: string;
}): Record<string, unknown> {
  const files = [{ name: input.fileName, content: input.sample }];
  if (input.linkml) files.push({ name: `${input.typeName}.linkml.yaml`, content: input.linkml });
  return { space: input.space, files, typeName: input.typeName, upload: true, propose: true };
}
