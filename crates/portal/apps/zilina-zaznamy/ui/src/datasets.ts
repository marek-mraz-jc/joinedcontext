/**
 * The Žilina project's public datasets, one grid each (T-3140, UI-64, UI-71): which space and
 * type a dataset is, the columns a person reads it by, and the whole dataset as a CSV file. A ui
 * app reads its own space and one more (AP-04), so the six indicators of `zilina-kpi` are the
 * dashboard zilina-ukazovatele's, not a grid here.
 *
 * Every dataset is read-only: each is written by its pipeline, whose next run would replace a
 * value a person typed. The columns are the model's own attributes, in the model's order; their
 * names on screen come from the locale.
 */
import { toCsv } from "@joinedcontext/sdk";
import type { Cell, EntitySource, GridColumn, ResolvedGridConfig, RichCell, RichRow, Row } from "@joinedcontext/sdk";

export const DATASETS = ["monuments", "stations", "air", "works"] as const;
export type Dataset = (typeof DATASETS)[number];

export interface DatasetSpec {
  space: string;
  type: string;
  columns: readonly string[];
  /** The columns the endpoint can be asked a `q` about: text and number attributes. */
  filters: readonly string[];
}

export const SPEC: Readonly<Record<Dataset, DatasetSpec>> = {
  monuments: {
    space: "zilina-verejne",
    type: "PointOfInterest",
    columns: ["name", "monumentKind", "architecturalStyle", "constructionPeriod", "cadastralArea", "ownershipForm", "monumentNumber", "address"],
    filters: ["monumentKind", "architecturalStyle", "constructionPeriod", "cadastralArea", "ownershipForm", "monumentNumber", "address"],
  },
  stations: {
    space: "zilina-verejne",
    type: "GtfsStop",
    columns: ["name", "dailyDepartures", "stopCode"],
    filters: ["dailyDepartures", "stopCode"],
  },
  air: {
    space: "zilina-verejne",
    type: "AirQualityObserved",
    columns: ["name", "pm10", "pm25", "no2", "o3", "co", "dateObserved"],
    filters: [],
  },
  works: {
    space: "zilina-uniza",
    type: "Article",
    columns: ["name", "workType", "yearPublished", "collectionName", "publisher", "license", "url"],
    filters: ["workType", "yearPublished", "collectionName", "publisher", "license"],
  },
};

/** The grid one dataset is: view only, its columns, filters the endpoint answers. */
export function gridConfig(dataset: Dataset, slug: string, labels: Record<string, string>): ResolvedGridConfig {
  const spec = SPEC[dataset];
  const columns: GridColumn[] = spec.columns.map((attr) => ({
    attr,
    label: labels[attr] ?? attr,
    ...(attr === "url" ? { format: "link" as const } : {}),
    // A reading's hour is part of it (ozone's can be a day older); its unit is in the header.
    ...(dataset === "air" && attr !== "name" && attr !== "dateObserved" ? { show: { observedAt: true } } : {}),
  }));
  return {
    source: { kind: "endpoint", slug },
    type: spec.type,
    columns,
    entityTimestamps: false,
    filters: { allowed: [...spec.filters], preset: {} },
    pageSize: 25,
    mode: "view",
    editableAttrs: [],
    // The public endpoints grant no temporal operation, so an offered history would be a 403.
    history: { enabled: false, maxPoints: 0 },
    density: "comfortable",
    rowActions: [],
    map: { enabled: false },
  };
}

/** The most rows one export reads: every dataset of the project is below it (DREPO: 787). */
export const EXPORT_MOST = 5000;
const EXPORT_PAGE = 500;

function one(cell: RichCell | RichCell[] | undefined): RichCell | undefined {
  return Array.isArray(cell) ? cell[0] : cell;
}

/** A cell as one CSV value: the reader's language of a name, a window as its two ends, a point. */
export function plain(cell: RichCell | RichCell[] | undefined, language: string): Cell {
  const first = one(cell);
  if (!first) return null;
  if (first.languageMap) return first.languageMap[language] ?? first.languageMap.sk ?? Object.values(first.languageMap)[0] ?? null;
  if (first.kind === "relationship") return Array.isArray(first.object) ? first.object.join(" ") : (first.object ?? null);
  const value = first.value;
  if (value === null || value === undefined) return null;
  if (typeof value === "string" || typeof value === "number" || typeof value === "boolean") return value;
  if (typeof value === "object") {
    const object = value as Record<string, unknown>;
    if (typeof object["@value"] === "string") return object["@value"];
    if (typeof object.start === "string" && typeof object.end === "string") return `${object.start}/${object.end}`;
    if (object.type === "Point") return value as Cell;
  }
  return JSON.stringify(value);
}

/** The whole dataset, page by page, as RFC 4180 CSV with a header of the attribute names. */
export async function exportCsv(dataset: Dataset, source: EntitySource, language: string): Promise<{ file: Blob; rows: number; truncated: boolean }> {
  const spec = SPEC[dataset];
  const rows: RichRow[] = [];
  let truncated = true;
  for (let offset = 0; offset < EXPORT_MOST; offset += EXPORT_PAGE) {
    const page = await source.query({ type: spec.type }, { offset, limit: EXPORT_PAGE });
    rows.push(...page.rows);
    if (page.rows.length < EXPORT_PAGE) {
      truncated = false;
      break;
    }
  }
  const columns = ["id", ...spec.columns];
  const table: Row[] = rows.map((row) => {
    const out: Row = { id: row.id, type: row.type };
    for (const attr of spec.columns) out[attr] = plain(row.cells[attr], language);
    return out;
  });
  return { file: toCsv(columns, table), rows: table.length, truncated };
}
