/**
 * The pages of an open data portal built from blocks (T-7418, `spec.pages` in
 * docs/kinds/Portal.md) and the editor's operations on them. Every operation returns a new
 * list; the server's jc-core check is the authority, `faultsOfPages` only names what the form
 * can tell before a publish.
 */

export type Texts = Record<string, string>;

export type BlockType =
  | "hero"
  | "text"
  | "dataset-gallery"
  | "featured-datasets"
  | "map"
  | "numbers"
  | "chart"
  | "link-cards"
  | "file-list";

export const BLOCK_TYPES: readonly BlockType[] = [
  "hero",
  "text",
  "dataset-gallery",
  "featured-datasets",
  "map",
  "numbers",
  "chart",
  "link-cards",
  "file-list",
];

export type PortalNumber = "datasets" | "publishers" | "updated-this-month";
export const PORTAL_NUMBERS: readonly PortalNumber[] = ["datasets", "publishers", "updated-this-month"];

export interface LinkCard {
  title: Texts;
  text?: Texts;
  url: string;
}

export type PortalBlock =
  | { type: "hero"; title?: Texts; intro?: Texts; image?: string; search?: boolean }
  | { type: "text"; markdown: Texts }
  | {
      type: "dataset-gallery";
      title?: Texts;
      tag?: string;
      publisher?: string;
      theme?: string;
      format?: string;
      limit?: number;
      layout?: "cards" | "table";
    }
  | { type: "featured-datasets"; title?: Texts; datasets: string[] }
  | { type: "map"; title?: Texts; dataset: string }
  | { type: "numbers"; title?: Texts; items: PortalNumber[] }
  | { type: "chart"; title?: Texts; dataset: string }
  | { type: "link-cards"; title?: Texts; cards: LinkCard[] }
  | { type: "file-list"; title?: Texts; files: string[] };

export interface PortalRow {
  blocks: PortalBlock[];
}

export interface PortalPage {
  slug: string;
  title?: Texts;
  menu?: boolean;
  rows?: PortalRow[];
}

export const HOME = "home";
export const MAX_PAGES = 12;
export const MAX_ROWS = 20;
export const MAX_ROW_BLOCKS = 3;
export const MAX_ITEMS = 12;
export const MAX_GALLERY = 24;

const DNS_LABEL = /^[a-z0-9]([-a-z0-9]{0,61}[a-z0-9])?$/;
const FILE_PATH = /^\/files\/[0-9a-f]{64}\/[^/\\"'<>`]+$/;

/** A new block of `type` with what it needs to pass the check, its texts in `lang`. */
export function newBlock(type: BlockType, lang: string): PortalBlock {
  switch (type) {
    case "hero":
      return { type, search: true };
    case "text":
      return { type, markdown: { [lang]: "" } };
    case "dataset-gallery":
      return { type, limit: 6 };
    case "featured-datasets":
      return { type, datasets: [] };
    case "map":
    case "chart":
      return { type, dataset: "" };
    case "numbers":
      return { type, items: ["datasets", "publishers"] };
    case "link-cards":
      return { type, cards: [{ title: { [lang]: "" }, url: "" }] };
    case "file-list":
      return { type, files: [] };
  }
}

/** The home a portal starts from: today's built-in home as blocks (hero, counts, newest). */
export function starterHome(): PortalPage {
  return {
    slug: HOME,
    rows: [
      { blocks: [{ type: "hero", search: true }] },
      { blocks: [{ type: "numbers", items: ["datasets", "publishers", "updated-this-month"] }] },
      { blocks: [{ type: "dataset-gallery", limit: 6 }] },
    ],
  };
}

/** Where a block sits: its page, row and place in the row. */
export interface At {
  page: number;
  row: number;
  block: number;
}

function withRows(pages: PortalPage[], page: number, rows: PortalRow[]): PortalPage[] {
  return pages.map((p, i) => (i === page ? { ...p, rows: rows.filter((r) => r.blocks.length > 0) } : p));
}

/** `pages` with a new row holding `block` at the end of page `page`. */
export function addRow(pages: PortalPage[], page: number, block: PortalBlock): PortalPage[] {
  const rows = pages[page]?.rows ?? [];
  if (rows.length >= MAX_ROWS) return pages;
  return withRows(pages, page, [...rows, { blocks: [block] }]);
}

/** `pages` with `block` added at the end of a row that has room. */
export function addToRow(pages: PortalPage[], page: number, row: number, block: PortalBlock): PortalPage[] {
  const rows = pages[page]?.rows ?? [];
  if ((rows[row]?.blocks.length ?? MAX_ROW_BLOCKS) >= MAX_ROW_BLOCKS) return pages;
  return withRows(
    pages,
    page,
    rows.map((r, i) => (i === row ? { blocks: [...r.blocks, block] } : r)),
  );
}

/** `pages` with the block at `at` replaced by `block`. */
export function setBlock(pages: PortalPage[], at: At, block: PortalBlock): PortalPage[] {
  const rows = pages[at.page]?.rows ?? [];
  return withRows(
    pages,
    at.page,
    rows.map((r, i) => (i === at.row ? { blocks: r.blocks.map((b, j) => (j === at.block ? block : b)) } : r)),
  );
}

/** `pages` without the block at `at`; a row left empty goes too. */
export function removeBlock(pages: PortalPage[], at: At): PortalPage[] {
  const rows = pages[at.page]?.rows ?? [];
  return withRows(
    pages,
    at.page,
    rows.map((r, i) => (i === at.row ? { blocks: r.blocks.filter((_, j) => j !== at.block) } : r)),
  );
}

/** `pages` with a copy of the block at `at`: beside it when its row has room, else in a new row below. */
export function duplicateBlock(pages: PortalPage[], at: At): PortalPage[] {
  const rows = pages[at.page]?.rows ?? [];
  const row = rows[at.row];
  const block = row?.blocks[at.block];
  if (!row || !block) return pages;
  const copy = structuredClone(block);
  if (row.blocks.length < MAX_ROW_BLOCKS) {
    const blocks = [...row.blocks];
    blocks.splice(at.block + 1, 0, copy);
    return withRows(pages, at.page, rows.map((r, i) => (i === at.row ? { blocks } : r)));
  }
  if (rows.length >= MAX_ROWS) return pages;
  const next = [...rows];
  next.splice(at.row + 1, 0, { blocks: [copy] });
  return withRows(pages, at.page, next);
}

/**
 * `pages` with the block at `at` moved one place: `up`/`down` into its own row above or below
 * (a block alone in its row moves the row), `left`/`right` inside its row.
 */
export function moveBlock(pages: PortalPage[], at: At, where: "up" | "down" | "left" | "right"): PortalPage[] {
  const rows = (pages[at.page]?.rows ?? []).map((r) => ({ blocks: [...r.blocks] }));
  const row = rows[at.row];
  const block = row?.blocks[at.block];
  if (!row || !block) return pages;
  if (where === "left" || where === "right") {
    const to = at.block + (where === "left" ? -1 : 1);
    if (to < 0 || to >= row.blocks.length) return pages;
    [row.blocks[at.block], row.blocks[to]] = [row.blocks[to] as PortalBlock, block];
    return withRows(pages, at.page, rows);
  }
  const to = at.row + (where === "up" ? -1 : 1);
  if (row.blocks.length === 1) {
    if (to < 0 || to >= rows.length) return pages;
    [rows[at.row], rows[to]] = [rows[to] as PortalRow, row];
    return withRows(pages, at.page, rows);
  }
  if (rows.length >= MAX_ROWS) return pages;
  row.blocks.splice(at.block, 1);
  rows.splice(where === "up" ? at.row : at.row + 1, 0, { blocks: [block] });
  return withRows(pages, at.page, rows);
}

/** The text of `texts` in `lang`, else the first one. */
export function textOf(texts: Texts | undefined, lang: string): string {
  if (!texts) return "";
  return texts[lang] ?? Object.values(texts)[0] ?? "";
}

/** `texts` with `lang` set to `value`; an empty value removes the language. */
export function withText(texts: Texts | undefined, lang: string, value: string): Texts | undefined {
  const next = { ...(texts ?? {}) };
  if (value === "") delete next[lang];
  else next[lang] = value;
  return Object.keys(next).length > 0 ? next : undefined;
}

/** What a publish would be refused for, by field path as the server names it. */
export function faultsOfPages(pages: PortalPage[]): Record<string, string> {
  const faults: Record<string, string> = {};
  const seen = new Set<string>();
  pages.forEach((page, p) => {
    const at = `spec.pages[${p}]`;
    if (page.slug !== HOME && !DNS_LABEL.test(page.slug)) faults[`${at}.slug`] = "slug";
    else if (seen.has(page.slug)) faults[`${at}.slug`] = "slugTaken";
    seen.add(page.slug);
    if (page.slug !== HOME && Object.keys(page.title ?? {}).length === 0) faults[`${at}.title`] = "pageTitle";
    (page.rows ?? []).forEach((row, r) =>
      row.blocks.forEach((block, b) => {
        const f = `${at}.rows[${r}].blocks[${b}]`;
        switch (block.type) {
          case "text":
            if (Object.values(block.markdown).every((m) => m.trim() === "")) faults[`${f}.markdown`] = "markdown";
            break;
          case "featured-datasets":
            if (block.datasets.length === 0) faults[`${f}.datasets`] = "pickDataset";
            break;
          case "map":
          case "chart":
            if (block.dataset === "") faults[`${f}.dataset`] = "pickDataset";
            break;
          case "numbers":
            if (block.items.length === 0) faults[`${f}.items`] = "pickNumber";
            break;
          case "file-list":
            if (block.files.length === 0) faults[`${f}.files`] = "pickFile";
            else if (block.files.some((x) => !FILE_PATH.test(x))) faults[`${f}.files`] = "pickFile";
            break;
          case "link-cards":
            block.cards.forEach((card, c) => {
              if (Object.values(card.title).every((t) => t.trim() === "")) faults[`${f}.cards[${c}].title`] = "cardTitle";
              if (!/^(https?:\/\/[^/@\s]+|\/(?!\/))\S*$/.test(card.url)) faults[`${f}.cards[${c}].url`] = "cardUrl";
            });
            break;
          case "dataset-gallery":
            if (block.limit !== undefined && (block.limit < 1 || block.limit > MAX_GALLERY)) faults[`${f}.limit`] = "limit";
            break;
          case "hero":
            break;
        }
      }),
    );
  });
  return faults;
}

/** The pages as the manifest holds them: empty texts and empty optional members dropped. */
export function cleanPages(pages: PortalPage[]): PortalPage[] {
  const texts = (t: Texts | undefined): Texts | undefined => {
    if (!t) return undefined;
    const kept = Object.fromEntries(Object.entries(t).filter(([, v]) => v.trim() !== ""));
    return Object.keys(kept).length > 0 ? kept : undefined;
  };
  const drop = <T extends object>(o: T): T =>
    Object.fromEntries(Object.entries(o).filter(([, v]) => v !== undefined && v !== "")) as T;
  return pages.map((page) =>
    drop({
      slug: page.slug,
      title: texts(page.title),
      menu: page.menu === false ? false : undefined,
      rows: (page.rows ?? []).map((row) => ({
        blocks: row.blocks.map((block): PortalBlock => {
          switch (block.type) {
            case "text":
              return { type: "text", markdown: texts(block.markdown) ?? {} };
            case "hero":
              return drop({ ...block, title: texts(block.title), intro: texts(block.intro), search: block.search === false ? false : undefined });
            case "link-cards":
              return drop({
                ...block,
                title: texts(block.title),
                cards: block.cards.map((c) => drop({ title: texts(c.title) ?? {}, text: texts(c.text), url: c.url.trim() })),
              });
            default:
              return drop({ ...block, title: texts(block.title) }) as PortalBlock;
          }
        }),
      })),
    }),
  );
}
