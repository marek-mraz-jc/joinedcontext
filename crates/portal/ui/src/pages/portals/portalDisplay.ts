/**
 * `spec.appearance.display` of a Portal (T-7743): which sections of the open data portal's
 * dataset page show, in which order and under which heading, the about table's rows and the
 * dataset list's fields. Each list left empty shows the level above's: the Organization's
 * `spec.appearance.display`, then the installation's built-in page. jc-core checks every name
 * again at propose; these lists are the same as its own.
 */

/** The dataset page's sections in their built-in order (jc-core `DATASET_SECTIONS`). */
export const DATASET_SECTIONS = [
  "publisher",
  "subscribe",
  "explorer",
  "map",
  "visualizations",
  "dashboards",
  "about",
  "live",
  "mcp",
  "resources",
  "additional-info",
  "shacl",
] as const;

/** The about table's DCAT rows in their built-in order (jc-core `ABOUT_FIELDS`). */
export const ABOUT_FIELDS = [
  "publisher",
  "license",
  "rights",
  "source",
  "keyword",
  "accrualPeriodicity",
  "issued",
  "modified",
  "nextUpdate",
  "spatial",
  "temporal",
  "contactPoint",
  "identifier",
  "conformsTo",
] as const;

/** What the dataset list shows after each title (jc-core `DATASET_LIST_FIELDS`). */
export const DATASET_LIST_FIELDS = ["notes", "format", "publisher", "modified"] as const;

/** A dataset extra a display may name, as jc-core's `is_extra_key`. */
export const EXTRA_KEY = /^[a-z0-9_-]{1,64}$/;

export interface DisplaySection {
  name: string;
  heading?: Record<string, string>;
}

export interface Display {
  dataset?: { sections?: DisplaySection[]; about?: string[] };
  datasetList?: { fields?: string[] };
}

/** The three lists a display holds, each edited, inherited and reset on its own. */
export type DisplayList = "sections" | "about" | "fields";

/** Where a list shows from. */
export type DisplayLevel = "portal" | "organization" | "builtIn";

/** One row of a list editor: a name, whether it shows, and (sections only) its headings. */
export interface DisplayRow {
  name: string;
  shown: boolean;
  heading: Record<string, string>;
}

function listOf(display: Display | undefined, list: DisplayList): DisplaySection[] {
  if (list === "sections") return display?.dataset?.sections ?? [];
  const names = list === "about" ? display?.dataset?.about : display?.datasetList?.fields;
  return (names ?? []).map((name) => ({ name }));
}

function builtIn(list: DisplayList): readonly string[] {
  return list === "sections" ? DATASET_SECTIONS : list === "about" ? ABOUT_FIELDS : DATASET_LIST_FIELDS;
}

/** The list that shows and the level it comes from: the portal's, else the Organization's, else built-in. */
export function resolved(own: Display | undefined, organization: Display | undefined, list: DisplayList): { rows: DisplayRow[]; from: DisplayLevel } {
  const mine = listOf(own, list);
  const theirs = listOf(organization, list);
  const from: DisplayLevel = mine.length > 0 ? "portal" : theirs.length > 0 ? "organization" : "builtIn";
  const chosen: DisplaySection[] = from === "portal" ? mine : from === "organization" ? theirs : builtIn(list).map((name) => ({ name }));
  const rows: DisplayRow[] = chosen.map((s) => ({ name: s.name, shown: true, heading: { ...(s.heading ?? {}) } }));
  // Every name the list knows shows in the editor; those the level leaves out are hidden.
  for (const name of builtIn(list)) {
    if (!rows.some((r) => r.name === name)) rows.push({ name, shown: false, heading: {} });
  }
  return { rows, from };
}

/** `rows` moved by one place, `at` going up (`-1`) or down (`1`). */
export function moved<T>(rows: T[], at: number, by: -1 | 1): T[] {
  const to = at + by;
  if (to < 0 || to >= rows.length) return rows;
  const next = [...rows];
  [next[at], next[to]] = [next[to], next[at]];
  return next;
}

/**
 * The display the editor proposes: each list set on the portal from its shown rows (headings
 * without text dropped), `undefined` for a list reset to inherited.
 */
export function displayOf(lists: Partial<Record<DisplayList, DisplayRow[]>>): Display | undefined {
  const shown = (list: DisplayList) => lists[list]?.filter((r) => r.shown);
  const sections = shown("sections")?.map((r) => {
    const heading = Object.fromEntries(Object.entries(r.heading).map(([l, h]) => [l, h.trim()]).filter(([, h]) => h !== ""));
    return Object.keys(heading).length > 0 ? { name: r.name, heading } : { name: r.name };
  });
  const about = shown("about")?.map((r) => r.name);
  const fields = shown("fields")?.map((r) => r.name);
  const display: Display = {};
  if (sections?.length || about?.length) {
    display.dataset = {};
    if (sections?.length) display.dataset.sections = sections;
    if (about?.length) display.dataset.about = about;
  }
  if (fields?.length) display.datasetList = { fields };
  return Object.keys(display).length > 0 ? display : undefined;
}

/** What the editor refuses before sending, as locale keys by list: nothing shown, a bad extra key, a heading with < or >. */
export function displayFaults(lists: Partial<Record<DisplayList, DisplayRow[]>>): Partial<Record<DisplayList, string>> {
  const found: Partial<Record<DisplayList, string>> = {};
  for (const list of ["sections", "about", "fields"] as const) {
    const rows = lists[list];
    if (rows && !rows.some((r) => r.shown)) found[list] = "portals.display.fault.empty";
  }
  if (lists.about?.some((r) => r.name.startsWith("extras.") && !EXTRA_KEY.test(r.name.slice(7)))) found.about = "portals.display.fault.extra";
  if (lists.sections?.some((r) => Object.values(r.heading).some((h) => /[<>]/.test(h) || h.length > 120))) found.sections = "portals.display.fault.heading";
  return found;
}
