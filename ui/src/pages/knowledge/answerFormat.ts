/**
 * What the chat shows of an answer (T-3325, AG-117): a small, safe part of Markdown read into a
 * plain tree that React renders as elements, so text the model writes stays text, and the
 * answer's citations grouped into its sources. The public widget reads answers with the same
 * rules (platform `crates/assistant/widget/render.js`), so both channels look alike.
 */
import type { Citation } from "./knowledge";

export type Inline =
  | string
  | { tag: "strong" | "em" | "code"; children: Inline[] }
  | { tag: "a"; href: string; children: Inline[] }
  | { tag: "cite"; numbers: number[] }
  | { tag: "br" };

export type Cell = { tag: "th" | "td"; children: Inline[] };

export type Block =
  | { tag: "p"; children: Inline[] }
  | { tag: "ul" | "ol"; children: { tag: "li"; children: Inline[] }[] }
  | { tag: "table"; children: { tag: "tr"; children: Cell[] }[] }
  | { tag: "pre"; text: string };

// Bold, italic, inline code, an http(s) link, and a citation marker `[1]` or `[1, 2]`.
const INLINE =
  /\*\*(.+?)\*\*|__(.+?)__|`([^`]+)`|\[([^\]\n]+)\]\((https?:\/\/[^\s()]+)\)|\[(\d+(?:\s*,\s*\d+)*)\]|\*(?=\S)(.+?)\*|(^|[^\w])_(?=\S)(.+?)_(?!\w)/g;

function inline(text: string, known: (n: number) => boolean): Inline[] {
  const out: Inline[] = [];
  let last = 0;
  const re = new RegExp(INLINE.source, "g");
  for (let match = re.exec(text); match !== null; match = re.exec(text)) {
    // `_em_` takes the character before it, so a snake_case name stays one word.
    out.push(text.slice(last, match.index) + (match[8] ?? ""));
    if (match[1] !== undefined || match[2] !== undefined) {
      out.push({ tag: "strong", children: inline(match[1] ?? match[2], known) });
    } else if (match[3] !== undefined) {
      out.push({ tag: "code", children: [match[3]] });
    } else if (match[4] !== undefined) {
      out.push({ tag: "a", href: match[5], children: inline(match[4], known) });
      // The model chooses a link's words: where it leads stands beside them unless they say so.
      const host = domain(match[5]);
      if (!namesHost(match[4], host)) out.push(` (${host})`);
    } else if (match[6] !== undefined) {
      const numbers = match[6]
        .split(",")
        .map((n) => Number(n.trim()))
        .filter(known);
      // A marker no citation stands behind is dropped, never shown as typed.
      if (numbers.length > 0) out.push({ tag: "cite", numbers });
    } else {
      out.push({ tag: "em", children: inline(match[7] ?? match[9], known) });
    }
    last = re.lastIndex;
  }
  out.push(text.slice(last));
  return out.filter((node) => node !== "");
}

const BULLET = /^\s*[-*+]\s+(.*)$/;
const NUMBERED = /^\s*\d{1,3}[.)]\s+(.*)$/;
const HEADING = /^\s{0,3}#{1,6}\s+(.*?)\s*#*\s*$/;
const FENCE = /^\s{0,3}(```|~~~)/;
const ROW = /^\s*\|.*\|\s*$/;
const RULE = /^\s*\|?(\s*:?-+:?\s*\|)+\s*(:?-+:?\s*)?$/;

// A table row's cells: the text between its pipes, a `\|` kept as a pipe.
function cells(line: string, known: (n: number) => boolean): Cell[] {
  return line
    .trim()
    .replace(/\\\|/g, "\u0000")
    .replace(/^\|/, "")
    .replace(/\|$/, "")
    .split("|")
    .map((cell) => ({ tag: "td", children: inline(cell.trim().split("\u0000").join("|"), known) }));
}

/**
 * The answer as blocks: paragraphs (lines joined by line breaks), bullet and numbered lists,
 * headings read as bold paragraphs, pipe tables and fenced code (T-4649, as the widget reads
 * them). `known(n)` says whether citation n exists.
 */
export function markdown(text: string, known: (n: number) => boolean = () => false): Block[] {
  const blocks: Block[] = [];
  let paragraph: { tag: "p"; children: Inline[] } | null = null;
  let list: { tag: "ul" | "ol"; children: { tag: "li"; children: Inline[] }[] } | null = null;
  let table: Table | null = null;
  let fence: { tag: "pre"; text: string; lines: string[] } | null = null;
  for (const line of text.replace(/\r\n?/g, "\n").split("\n")) {
    if (fence) {
      if (FENCE.test(line)) fence = null;
      else fence.lines.push(line);
      continue;
    }
    if (FENCE.test(line)) {
      fence = { tag: "pre", text: "", lines: [] };
      blocks.push(fence);
      paragraph = list = table = null;
      continue;
    }
    if (ROW.test(line) || (table && RULE.test(line))) {
      paragraph = list = null;
      table = tableLine(line, table, blocks, known);
      continue;
    }
    table = null;
    const item = BULLET.exec(line) ?? NUMBERED.exec(line);
    const tag = BULLET.test(line) ? "ul" : "ol";
    const heading = HEADING.exec(line);
    if (item) {
      if (!list || list.tag !== tag) {
        list = { tag, children: [] };
        blocks.push(list);
      }
      list.children.push({ tag: "li", children: inline(item[1], known) });
      paragraph = null;
    } else if (line.trim() === "") {
      paragraph = null;
      list = null;
    } else if (heading) {
      blocks.push({ tag: "p", children: [{ tag: "strong", children: inline(heading[1], known) }] });
      paragraph = null;
      list = null;
    } else if (list && /^\s{2,}\S/.test(line)) {
      // An indented line goes on with the list item above it.
      const last = list.children[list.children.length - 1];
      last.children = [...last.children, { tag: "br" }, ...inline(line.trim(), known)];
    } else {
      list = null;
      if (!paragraph) {
        paragraph = { tag: "p", children: [] };
        blocks.push(paragraph);
      } else {
        paragraph.children.push({ tag: "br" });
      }
      paragraph.children.push(...inline(line.trim(), known));
    }
  }
  closeFences(blocks);
  return blocks;
}

type Table = { tag: "table"; children: { tag: "tr"; children: Cell[] }[] };

// One table line: a row, or the rule under the first row, which makes it the header.
function tableLine(line: string, table: Table | null, blocks: Block[], known: (n: number) => boolean): Table {
  if (table && table.children.length === 1 && RULE.test(line)) {
    for (const cell of table.children[0].children) cell.tag = "th";
    return table;
  }
  if (!table) {
    table = { tag: "table", children: [] };
    blocks.push(table);
  }
  table.children.push({ tag: "tr", children: cells(line, known) });
  return table;
}

// A fenced block's lines become its text once the whole answer is read.
function closeFences(blocks: Block[]) {
  for (const block of blocks) {
    if (block.tag === "pre" && "lines" in block) {
      block.text = (block.lines as string[]).join("\n");
      delete (block as { lines?: string[] }).lines;
    }
  }
}

export interface Source {
  url: string | null;
  title: string | null;
  live: boolean;
  endpoint: string | null;
  domain: string;
  numbers: number[];
}

// Whether one host-shaped word of a link's text is the host itself: "hel.fi" does not name
// el.fi, nor "mybank.com.example" bank.com (F-R2UI3-002 review).
function namesHost(text: string, host: string): boolean {
  const words = text.toLowerCase().match(/[a-z0-9-]+(?:\.[a-z0-9-]+)+/g) ?? [];
  return words.some((word) => word.replace(/^www\./, "") === host.toLowerCase());
}

function domain(url: string): string {
  return /^https?:\/\/([^/?#:]+)/i.exec(url)?.[1].replace(/^www\./, "") ?? "";
}

/**
 * The citations as the sources a person reads: one per address, the numbers that share it
 * merged, each with its title and domain; live data never by its tool's name. `position[n]` is
 * the 1-based place of citation n in `list`.
 */
export function sources(citations: Citation[]): { list: Source[]; position: Record<number, number> } {
  const list: Source[] = [];
  const byKey = new Map<string, number>();
  const position: Record<number, number> = {};
  for (const citation of citations) {
    const url = citation.url && /^https?:\/\//i.test(citation.url) ? citation.url : null;
    // An uploaded file's passage cites `upload:{source}/{file}`: a name, never a link (T-4869).
    const file = citation.url?.startsWith("upload:") ? citation.url.replace(/#page=\d+$/, "") : null;
    const live = Boolean(citation.tool);
    const key = url ?? file ?? (live ? `live:${citation.endpoint ?? ""}` : null);
    if (!key) continue;
    const title = citation.title?.trim() || (file ? file.slice(file.indexOf("/") + 1) || null : null);
    let at = byKey.get(key);
    if (at === undefined) {
      at = list.length;
      byKey.set(key, at);
      list.push({ url, title, live, endpoint: citation.endpoint ?? null, domain: url ? domain(url) : "", numbers: [] });
    }
    const entry = list[at];
    entry.title ??= title;
    entry.numbers.push(citation.n);
    position[citation.n] = at + 1;
  }
  return { list, position };
}

/** What a source's link says: its title, else the address without its scheme. */
export function label(source: Pick<Source, "url" | "title" | "endpoint">): string {
  if (source.title) return source.title;
  if (source.url) return source.url.replace(/^https?:\/\//i, "").replace(/\/$/, "");
  return source.endpoint ?? "";
}
