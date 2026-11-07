/**
 * The university's openly licensed works as the screen reads them (T-3140): one `Article` of
 * `zilina-uniza` each, written by the pipeline drepo from DREPO. No work carries an author, and the
 * screen asks for none. A value the entity does not carry stays `null`, never a zero or "".
 */
import type { RichCell, RichRow } from "@joinedcontext/sdk";

export interface Work {
  id: string;
  title: string | null;
  /** The title's language as the repository recorded it, `null` for none stated. */
  language: string | null;
  kind: string | null;
  year: number | null;
  /** The deposit collection, a journal issue or a proceedings volume. */
  collection: string | null;
  /** The journal or proceedings the collection is an issue of. */
  series: string | null;
  licence: string | null;
  url: string | null;
}

function one(row: RichRow, attr: string): RichCell | undefined {
  const cell = row.cells[attr];
  return Array.isArray(cell) ? cell[0] : cell;
}

function text(row: RichRow, attr: string): string | null {
  const value = one(row, attr)?.value;
  return typeof value === "string" && value.trim() !== "" ? value.trim() : null;
}

/** A link the list may open: http or https only, so a `javascript:` URL in the data never runs. */
export function safeUrl(url: string | null): string | null {
  if (!url) return null;
  try {
    const parsed = new URL(url);
    return parsed.protocol === "https:" || parsed.protocol === "http:" ? parsed.href : null;
  } catch {
    return null;
  }
}

/**
 * The journal a collection is an issue of: the name before its issue, "Krízový manažment" of
 * "Krízový manažment - Ročník 24.; Číslo 2/2025", "Práce a štúdie" of "Práce a štúdie - Vydanie 17".
 */
export function seriesOf(collection: string | null): string | null {
  if (!collection) return null;
  const head = collection.split(/\s+[-–]\s+/)[0].split(";")[0].trim();
  return head === "" ? null : head;
}

export function workOf(row: RichRow): Work {
  const names = one(row, "name")?.languageMap ?? {};
  const [language, title] = Object.entries(names).find(([, value]) => typeof value === "string" && value.trim() !== "") ?? [null, null];
  const year = one(row, "yearPublished")?.value;
  const collection = text(row, "collectionName");
  return {
    id: row.id,
    title: title ? title.trim() : null,
    language: language === "@none" ? null : language,
    kind: text(row, "workType"),
    year: typeof year === "number" && Number.isInteger(year) ? year : null,
    collection,
    series: seriesOf(collection),
    licence: text(row, "license"),
    url: safeUrl(text(row, "url")),
  };
}

/** How many works each value of `key` has, most first, then by name; works without one are left out. */
export function countBy(works: Work[], key: (work: Work) => string | null): { name: string; count: number }[] {
  const counts = new Map<string, number>();
  for (const work of works) {
    const name = key(work);
    if (name !== null) counts.set(name, (counts.get(name) ?? 0) + 1);
  }
  return [...counts.entries()]
    .map(([name, count]) => ({ name, count }))
    .sort((a, b) => b.count - a.count || a.name.localeCompare(b.name, "sk"));
}

/** Works per year, every year from the first to the last, a year with none as zero. */
export function perYear(works: Work[]): { year: number; count: number }[] {
  const years = works.map((work) => work.year).filter((year): year is number => year !== null);
  if (years.length === 0) return [];
  const first = Math.min(...years);
  const last = Math.max(...years);
  const out: { year: number; count: number }[] = [];
  for (let year = first; year <= last; year += 1) out.push({ year, count: years.filter((y) => y === year).length });
  return out;
}

/** Text folded for search: lower case, without diacritics. */
export function folded(value: string): string {
  return value.normalize("NFD").replace(/\p{Diacritic}/gu, "").toLowerCase();
}

export interface Narrowing {
  search: string;
  kind: string | null;
  year: number | null;
}

/** The works a person narrowed to: every word in the title or collection, and the kind and year picked. */
export function narrowed(works: Work[], by: Narrowing): Work[] {
  const words = folded(by.search).split(/\s+/).filter((word) => word !== "");
  return works.filter((work) => {
    if (by.kind !== null && work.kind !== by.kind) return false;
    if (by.year !== null && work.year !== by.year) return false;
    const haystack = folded(`${work.title ?? ""} ${work.collection ?? ""}`);
    return words.every((word) => haystack.includes(word));
  });
}

/** Newest first, then by title. */
export function byNewest(a: Work, b: Work): number {
  return (b.year ?? 0) - (a.year ?? 0) || (a.title ?? "").localeCompare(b.title ?? "", "sk");
}
