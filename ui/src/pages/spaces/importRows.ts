/**
 * Importing a file into one entity type of a space (T-3109, ADR-N-042 §3.6): the file is read in
 * the browser, its columns are mapped to the type's slots, every row is checked against what the
 * model says of each slot, and the valid rows are created in batches through the gateway with the
 * person's session, like any edit (§3.3). A batch create never overwrites an entity: an id that
 * exists comes back refused, in the report with every row the check or the gateway refused.
 *
 * Every format is read with what the browser has: CSV and JSON as text, XML with `DOMParser`, and
 * an XLSX workbook as the ZIP of XML it is, unpacked with `DecompressionStream`.
 */
import { classSlots, importName, parseModel } from "../models/linkml";
import type { LinkmlClass, NgsiLdKind } from "../models/linkml";
import { ENTITY_ID } from "./formView";

/** A file read into rows of text, numbers and flags, keyed by its column names. */
export interface Table {
  columns: string[];
  rows: Record<string, string>[];
  /** How many rows the file holds when it holds more than MAX_ROWS, which `rows` stops at (F-UI3-011). */
  total?: number;
  /** What the reading found and the person is shown (T-3248): absent for a format with no choice. */
  detected?: Detected;
}

/** How a text file was read: its encoding and separator, and how its numbers and dates are written. */
export interface Detected {
  encoding?: "utf-8" | "windows-1250";
  separator?: "," | ";" | "\t";
  decimalComma: boolean;
  dateFormat?: "d.m.yyyy" | "yyyy-mm-dd";
}

/** One slot of the type as the import needs it. */
export interface ImportSlot {
  name: string;
  range?: string;
  kind: NgsiLdKind;
  required: boolean;
  /** What a person reads for the slot, per language: an inferred slot's title is the file's own column. */
  titles?: string[];
  /** The permissible values, when the range is an enum of the model. */
  values?: string[];
  minimum?: number;
  maximum?: number;
}

/** What a column feeds: a slot by name, the entity's `id`, or nothing. */
export type Mapping = Record<string, string>;

/** The column that names the entity: a URN, or the local part the import completes. */
export const ID = "id";

/** How many entities one create through the gateway carries. */
export const BATCH = 100;

/** The most rows one import reads: a bigger file is a feed, a DataSource and a Pipeline. */
export const MAX_ROWS = 10_000;

/** The largest file an import reads (T-3840): refused by its size before a byte is read. */
export const MAX_BYTES = 50 * 1024 * 1024;

/** Mapping targets of two columns that together make the entity's point location (T-3840). */
export const LATITUDE = "@latitude";
/** The mapping target of a column that holds each entity's `expiresAt` (TS 104 175 4.22, T-4822). */
export const EXPIRES_AT = "@expiresAt";
export const LONGITUDE = "@longitude";

/**
 * The geometry NGSI-LD gives every entity (ETSI GS CIM 009 §4.5.4): a class whose `is_a` chain ends
 * at `ngsi-ld-core`'s `Entity` carries it without listing it, so a GeoJSON file's points have a field.
 */
const CORE_LOCATION: ImportSlot = { name: "location", kind: "GeoProperty", required: false };

/**
 * The slots of one class of the space's model with those it inherits along `is_a` (a child's own
 * wins a name both carry), with `required` from each class's own usage.
 */
export function importSlotsOf(source: string | undefined, type: string): ImportSlot[] {
  if (source === undefined || !type) return [];
  const model = parseModel(source);
  const byName = new Map(model.classes.map((c) => [c.name, c]));
  const chain: LinkmlClass[] = [];
  for (let at = byName.get(type); at && !chain.includes(at); at = at.is_a ? byName.get(at.is_a) : undefined) chain.push(at);
  if (chain.length === 0) return [];
  const slots = new Map<string, ImportSlot>();
  for (const klass of chain.reverse()) {
    for (const slot of classSlots(model, klass)) {
      if (slot.name === "id" || slot.name === "type") continue;
      slots.set(slot.name, {
        name: slot.name,
        range: slot.range,
        kind: slot.kind,
        required: slot.required === true,
        titles: Object.values(slot.title ?? {}),
        values: model.enums.find((e) => e.name === slot.range)?.permissible_values.map((v) => v.name),
        minimum: slot.minimum_value,
        maximum: slot.maximum_value,
      });
    }
  }
  const root = chain[0];
  const core = (model.imports ?? []).some((name) => importName(name) === "ngsi-ld-core");
  if (core && root.is_a === "Entity" && !byName.has("Entity") && !slots.has(CORE_LOCATION.name)) {
    slots.set(CORE_LOCATION.name, CORE_LOCATION);
  }
  return [...slots.values()];
}

// ---------------------------------------------------------------------------------------------
// Reading a file
// ---------------------------------------------------------------------------------------------

/** A file over `MAX_BYTES`, refused before a byte is read. */
export class FileTooLarge extends Error {
  constructor(readonly bytes: number) {
    super(`a file of at most ${MAX_BYTES / 1024 / 1024} MB`);
  }
}

/** Why a file was not read, in the person's words: its size against the limit, or the reader's reason. */
export function unreadableText(t: (key: string, options?: Record<string, unknown>) => string, error: unknown): string {
  if (error instanceof FileTooLarge) {
    return t("spaces.import.tooLarge", { size: (error.bytes / 1024 / 1024).toFixed(1), limit: MAX_BYTES / 1024 / 1024 });
  }
  return t("spaces.import.unreadable", { reason: error instanceof Error ? error.message : String(error) });
}

/** The file as rows, by its extension; an error names what could not be read. */
export async function readTable(file: File): Promise<Table> {
  if (file.size > MAX_BYTES) throw new FileTooLarge(file.size);
  const name = file.name.toLowerCase();
  if (name.endsWith(".csv") || name.endsWith(".tsv") || name.endsWith(".txt")) {
    const { text, encoding } = decodeText(new Uint8Array(await file.arrayBuffer()));
    const table = parseCsv(text);
    return { ...table, detected: { ...detectedOf(table), encoding, separator: separatorOf(text) } };
  }
  if (name.endsWith(".json") || name.endsWith(".geojson")) return parseJson(await file.text());
  if (name.endsWith(".xml")) return parseXml(await file.text());
  if (name.endsWith(".xlsx")) return parseXlsx(new Uint8Array(await file.arrayBuffer()));
  throw new Error("read CSV, Excel (.xlsx), JSON or XML");
}

/**
 * The bytes of a text file as text: UTF-8 when they are UTF-8, else Windows-1250, the code page a
 * Slovak or Czech Excel writes its CSV in, so `č` and `ž` arrive as themselves.
 */
export function decodeText(bytes: Uint8Array): { text: string; encoding: "utf-8" | "windows-1250" } {
  try {
    return { text: new TextDecoder("utf-8", { fatal: true }).decode(bytes), encoding: "utf-8" };
  } catch {
    return { text: new TextDecoder("windows-1250").decode(bytes), encoding: "windows-1250" };
  }
}

/**
 * Bytes in the encoding the whole file was read in; a slice cut inside a UTF-8 character still
 * reads as UTF-8 rather than falling back to Windows-1250. No encoding known: as decodeText.
 */
export function decodeAs(bytes: Uint8Array, encoding: "utf-8" | "windows-1250" | undefined): string {
  return encoding ? new TextDecoder(encoding).decode(bytes) : decodeText(bytes).text;
}

/** The separator of `,`, `;` and tab the header line holds most of. */
export function separatorOf(text: string): "," | ";" | "\t" {
  const header = text.replace(/^\uFEFF/, "").split(/\r?\n/, 1)[0] ?? "";
  return ([",", ";", "\t"] as const).reduce((best, sep) =>
    header.split(sep).length > header.split(best).length ? sep : best,
  );
}

const DECIMAL_COMMA = /^-?\d{1,3}(?:[ .\u00a0]?\d{3})*,\d+$/;
const SLOVAK_DATE = /^(\d{1,2})\.\s?(\d{1,2})\.\s?(\d{4})(?:\s+(\d{1,2}):(\d{2})(?::(\d{2}))?)?$/;
const ISO_DATE = /^\d{4}-\d{2}-\d{2}/;

/** A number that can only be read one way: both marks, or a mark not followed by three digits. */
const POINT_DECIMAL = /^-?(?:\d{1,3}(?:,\d{3})+\.\d+|\d+\.(?:\d{1,2}|\d{4,}))$/;
const COMMA_DECIMAL = /^-?(?:\d{1,3}(?:\.\d{3})+,\d+|\d+,(?:\d{1,2}|\d{4,}))$/;

/**
 * The decimal mark the file's numbers show beyond doubt, or `undefined` when they show none or
 * both. ponytail: one mark per file, not per column; a file mixing them keeps the old reading.
 */
export function decimalMarkOf(table: Table): "." | "," | undefined {
  const cells = table.rows.slice(0, 200).flatMap((row) => Object.values(row).map((cell) => cell.replace(/[\s\u00a0]/g, "")));
  const point = cells.some((cell) => POINT_DECIMAL.test(cell));
  const comma = cells.some((cell) => COMMA_DECIMAL.test(cell));
  return point === comma ? undefined : point ? "." : ",";
}

/** How the table writes its numbers and dates, read from its first rows. */
export function detectedOf(table: Table): Detected {
  const cells = table.rows.slice(0, 200).flatMap((row) => Object.values(row));
  return {
    decimalComma: cells.some((cell) => DECIMAL_COMMA.test(cell)),
    dateFormat: cells.some((cell) => SLOVAK_DATE.test(cell))
      ? "d.m.yyyy"
      : cells.some((cell) => ISO_DATE.test(cell))
        ? "yyyy-mm-dd"
        : undefined,
  };
}

/**
 * A number as a person writes it: `4,5` and `1 234,5` and `1.234,5` (decimal comma, the group
 * mark a space or a dot) as well as `1,234.5` and `4.5`. When both marks occur, the last one is the
 * decimal mark; a lone comma before digits is one.
 */
export function numberOf(text: string, mark?: "." | ","): number {
  const bare = text.replace(/[\s\u00a0]/g, "");
  // The file's own decimal mark, where it showed one beyond doubt: `1,234` in a file that writes
  // `4.5` is a thousand and more, never 1.234 (F-UI3-012).
  if (mark === ".") return bare === "" ? Number.NaN : Number(bare.replace(/,/g, ""));
  if (mark === ",") return bare === "" ? Number.NaN : Number(bare.replace(/\./g, "").replace(",", "."));
  const comma = bare.lastIndexOf(",");
  const dot = bare.lastIndexOf(".");
  const plain =
    comma >= 0 && dot >= 0
      ? comma > dot
        ? bare.replace(/\./g, "").replace(",", ".")
        : bare.replace(/,/g, "")
      : bare.replace(/,(?=\d+$)/, ".");
  return plain === "" ? Number.NaN : Number(plain);
}

/** A date as a Slovak file writes it, `7.10.2026` or `7. 10. 2026 14:30`, in ISO form; else as given. */
export function isoDateOf(text: string): string {
  const match = SLOVAK_DATE.exec(text.trim());
  if (!match) return text;
  const [, day, month, year, hour, minute, second] = match;
  const date = `${year}-${month.padStart(2, "0")}-${day.padStart(2, "0")}`;
  return hour === undefined ? date : `${date}T${hour.padStart(2, "0")}:${minute}:${second ?? "00"}`;
}

/** CSV by RFC 4180, its separator the one of `,`, `;` and tab the header line holds most of. */
export function parseCsv(text: string): Table {
  const body = text.replace(/^\uFEFF/, "");
  const separator = separatorOf(body);
  const records: string[][] = [];
  let record: string[] = [];
  let field = "";
  let quoted = false;
  for (let at = 0; at < body.length; at += 1) {
    const char = body[at];
    if (quoted) {
      if (char === '"' && body[at + 1] === '"') {
        field += '"';
        at += 1;
      } else if (char === '"') {
        quoted = false;
      } else {
        field += char;
      }
    } else if (char === '"' && field === "") {
      quoted = true;
    } else if (char === separator) {
      record.push(field);
      field = "";
    } else if (char === "\n" || char === "\r") {
      if (char === "\r" && body[at + 1] === "\n") at += 1;
      record.push(field);
      records.push(record);
      record = [];
      field = "";
    } else {
      field += char;
    }
  }
  if (field !== "" || record.length > 0) {
    record.push(field);
    records.push(record);
  }
  return fromRecords(records.filter((r) => r.some((cell) => cell.trim() !== "")));
}

/** The file's row count when the table stops short of it, so the wizard can say so. */
function cutAt(total: number): { total?: number } {
  return total > MAX_ROWS ? { total } : {};
}

/** A header record and the records under it, as a table; a column without a name is `column N`. */
function fromRecords(records: string[][]): Table {
  const [head, ...rest] = records;
  if (!head) return { columns: [], rows: [] };
  const columns = head.map((name, at) => name.trim() || `column ${at + 1}`);
  const rows = rest.slice(0, MAX_ROWS).map((record) =>
    Object.fromEntries(columns.map((column, at) => [column, (record[at] ?? "").trim()])),
  );
  return { columns, rows, ...cutAt(rest.length) };
}

/**
 * JSON: an array of objects, an object holding one such array, or a GeoJSON FeatureCollection,
 * whose features give their properties and their geometry as `location`. A nested value is kept
 * as its JSON text.
 */
export function parseJson(text: string): Table {
  let data: unknown;
  try {
    data = JSON.parse(text);
  } catch {
    throw new Error("the file is not JSON");
  }
  let items: unknown[] | undefined;
  if (Array.isArray(data)) items = data;
  else if (data && typeof data === "object") {
    const object = data as Record<string, unknown>;
    if (object.type === "FeatureCollection" && Array.isArray(object.features)) {
      items = object.features.map((feature) => {
        const f = (feature ?? {}) as { properties?: Record<string, unknown>; geometry?: unknown; id?: unknown };
        return { ...(f.id !== undefined ? { id: f.id } : {}), ...(f.properties ?? {}), location: f.geometry };
      });
    } else {
      items = Object.values(object).find(Array.isArray);
    }
  }
  if (!items) throw new Error("the JSON holds no list of records");
  const objects = items.filter((item): item is Record<string, unknown> => !!item && typeof item === "object" && !Array.isArray(item));
  const columns = [...new Set(objects.flatMap((item) => Object.keys(item)))];
  const rows = objects.slice(0, MAX_ROWS).map((item) =>
    Object.fromEntries(columns.map((column) => [column, textOf(item[column])])),
  );
  return { columns, rows, ...cutAt(objects.length) };
}

function textOf(value: unknown): string {
  if (value === null || value === undefined) return "";
  if (typeof value === "object") return JSON.stringify(value);
  return String(value).trim();
}

/** XML: the root's repeated children are the records, each child or attribute of one a field. */
export function parseXml(text: string): Table {
  const document = new DOMParser().parseFromString(text, "application/xml");
  if (document.getElementsByTagName("parsererror").length > 0) throw new Error("the file is not well-formed XML");
  const root = document.documentElement;
  const children = Array.from(root.children);
  // The records are the most frequent element name under the root (a `<rows><row/>…</rows>`).
  const counts = new Map<string, number>();
  for (const child of children) counts.set(child.tagName, (counts.get(child.tagName) ?? 0) + 1);
  const recordName = [...counts.entries()].sort((a, b) => b[1] - a[1])[0]?.[0];
  const records = children.filter((child) => child.tagName === recordName);
  const objects = records.map((record) => {
    const fields: Record<string, string> = {};
    for (const attribute of Array.from(record.attributes)) fields[attribute.name] = attribute.value.trim();
    for (const field of Array.from(record.children)) fields[field.tagName] = (field.textContent ?? "").trim();
    return fields;
  });
  const columns = [...new Set(objects.flatMap((item) => Object.keys(item)))];
  return {
    columns,
    rows: objects.slice(0, MAX_ROWS).map((item) => Object.fromEntries(columns.map((c) => [c, item[c] ?? ""]))),
    ...cutAt(objects.length),
  };
}

/** The entries a workbook's sheets are read from: their names, order, strings, number formats and cells. */
const WORKBOOK_PARTS = /^xl\/(sharedStrings|workbook|styles|worksheets\/[^/]+)\.xml$|^xl\/_rels\/workbook\.xml\.rels$/;

/** What a whole workbook may unpack to, its sheets together. */
const MAX_UNPACKED = 2 * MAX_BYTES;

/** The entries of a ZIP archive by name, read from its central directory (stored or deflated). */
async function unzip(bytes: Uint8Array): Promise<Map<string, Uint8Array>> {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  // The end-of-central-directory record is within the last 64 KiB plus its own 22 bytes.
  let end = -1;
  for (let at = bytes.length - 22; at >= Math.max(0, bytes.length - 65_557); at -= 1) {
    if (view.getUint32(at, true) === 0x06054b50) {
      end = at;
      break;
    }
  }
  if (end < 0) throw new Error("the file is not an Excel workbook (.xlsx): save it from the spreadsheet as .xlsx or CSV and choose it again");
  const count = view.getUint16(end + 10, true);
  let at = view.getUint32(end + 16, true);
  const entries = new Map<string, Uint8Array>();
  const names = new TextDecoder();
  // An offset past the file's end is a damaged workbook, said as one, not a RangeError (JC1-45).
  const damaged = () => new Error("the workbook's directory is damaged");
  // Every sheet is unpacked now (JC1-21): their sum is bounded too, not each alone.
  const budget = { left: MAX_UNPACKED };
  for (let n = 0; n < count; n += 1) {
    if (at + 46 > bytes.length || view.getUint32(at, true) !== 0x02014b50) throw damaged();
    const method = view.getUint16(at + 10, true);
    const compressed = view.getUint32(at + 20, true);
    const nameLength = view.getUint16(at + 28, true);
    const extraLength = view.getUint16(at + 30, true);
    const commentLength = view.getUint16(at + 32, true);
    const local = view.getUint32(at + 42, true);
    const name = names.decode(bytes.subarray(at + 46, at + 46 + nameLength));
    if (local + 30 > bytes.length) throw damaged();
    const dataAt = local + 30 + view.getUint16(local + 26, true) + view.getUint16(local + 28, true);
    if (dataAt + compressed > bytes.length) throw damaged();
    const data = bytes.subarray(dataAt, dataAt + compressed);
    // Only what the sheets are read from: a workbook's pictures and drawings stay packed.
    if (WORKBOOK_PARTS.test(name)) {
      if (method === 0) entries.set(name, data);
      else if (method === 8) entries.set(name, await inflate(data, budget));
    }
    at += 46 + nameLength + extraLength + commentLength;
  }
  return entries;
}

/**
 * One deflated entry, stopped at `MAX_BYTES` unpacked and at what the workbook has left: a few
 * kilobytes of ZIP can unpack to gigabytes, which would take the tab down before a row is read
 * (F-R2UI4-008).
 */
async function inflate(data: Uint8Array, budget: { left: number }): Promise<Uint8Array> {
  const body = new Response(new Uint8Array(data)).body;
  if (!body) throw new Error("the workbook cannot be unpacked here");
  const reader = body.pipeThrough(new DecompressionStream("deflate-raw")).getReader();
  const chunks: Uint8Array[] = [];
  let size = 0;
  for (let read = await reader.read(); !read.done; read = await reader.read()) {
    size += read.value.byteLength;
    if (size > MAX_BYTES || size > budget.left) {
      await reader.cancel();
      throw new Error(`the workbook unpacks to more than ${MAX_BYTES / 1024 / 1024} MB`);
    }
    chunks.push(read.value);
  }
  budget.left -= size;
  const whole = new Uint8Array(size);
  let at = 0;
  for (const chunk of chunks) {
    whole.set(chunk, at);
    at += chunk.byteLength;
  }
  return whole;
}

/** A column's letters as its index: `A` is 0, `AA` is 26. */
function columnIndex(reference: string): number {
  const letters = /^[A-Z]+/.exec(reference)?.[0] ?? "A";
  return [...letters].reduce((sum, letter) => sum * 26 + letter.charCodeAt(0) - 64, 0) - 1;
}

/** One sheet of a workbook: its name as the spreadsheet shows it, and its rows. */
export interface Sheet {
  name: string;
  table: Table;
}

/** The built-in number formats that show a date or a time (ECMA-376 part 1, 18.8.30). */
const DATE_FORMATS = new Set([14, 15, 16, 17, 18, 19, 20, 21, 22, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 45, 46, 47, 50, 51, 52, 53, 54, 55, 56, 57, 58]);

/** Whether a custom format code shows a date or time: a d, m, y, h or s outside quotes and brackets. */
export function isDateFormat(code: string): boolean {
  const bare = code.replace(/"[^"]*"|\[[^\]]*\]|\\./g, "");
  return /[dmyhs]/i.test(bare);
}

/**
 * A date cell's serial as ISO 8601: `2026-03-05` for a whole day, `2026-03-05T14:30:00` with a
 * time, `14:30:00` for a time alone. A spreadsheet keeps no time zone, so none is said (JC1-21).
 */
export function serialDate(serial: number, date1904 = false): string {
  // Day 25569 of the 1900 system is 1970-01-01; the 1904 system starts 1462 days later.
  const iso = new Date(Math.round((serial + (date1904 ? 1462 : 0) - 25569) * 86_400_000)).toISOString();
  if (serial >= 0 && serial < 1) return iso.slice(11, 19);
  return Number.isInteger(serial) ? iso.slice(0, 10) : iso.slice(0, 19);
}

type Xml = (name: string) => Document | undefined;

/** One cell's text: a shared or inline string, a boolean, a date cell as its ISO date, else the raw value. */
export function cellValue(cell: Element, shared: string[], dated: boolean[], date1904: boolean): string {
  const type = cell.getAttribute("t");
  const value = cell.getElementsByTagName("v")[0]?.textContent ?? "";
  if (type === "s") return shared[Number(value)] ?? "";
  if (type === "inlineStr") return Array.from(cell.getElementsByTagName("t")).map((t) => t.textContent ?? "").join("");
  if (type === "b") return value === "1" ? "true" : "false";
  const date = (type === null || type === "n") && value !== "" && dated[Number(cell.getAttribute("s") ?? 0)];
  return date && Number.isFinite(Number(value)) ? serialDate(Number(value), date1904) : value;
}

/** Per cell style index, whether its number format shows a date or a time. */
function datedStyles(styles: Document | undefined): boolean[] {
  const custom = new Map(
    Array.from(styles?.getElementsByTagName("numFmt") ?? []).map((f) => [Number(f.getAttribute("numFmtId")), f.getAttribute("formatCode") ?? ""]),
  );
  const cellXfs = styles?.getElementsByTagName("cellXfs")[0];
  return Array.from(cellXfs?.getElementsByTagName("xf") ?? []).map((xf) => {
    const id = Number(xf.getAttribute("numFmtId") ?? 0);
    return DATE_FORMATS.has(id) || isDateFormat(custom.get(id) ?? "");
  });
}

/** The sheets' names and parts: the workbook's own list orders them; without one, sheet2 still comes before sheet10. */
function sheetParts(entries: Map<string, Uint8Array>, book: Document | undefined, xml: Xml): { name: string; part: string }[] {
  const targets = new Map(
    Array.from(xml("xl/_rels/workbook.xml.rels")?.getElementsByTagName("Relationship") ?? []).map((r) => {
      const target = r.getAttribute("Target") ?? "";
      return [r.getAttribute("Id") ?? "", target.startsWith("/") ? target.slice(1) : `xl/${target}`];
    }),
  );
  const listed = Array.from(book?.getElementsByTagName("sheet") ?? []).map((s) => ({
    name: s.getAttribute("name") ?? "",
    part: targets.get(s.getAttribute("r:id") ?? "") ?? "",
  }));
  if (listed.length > 0) return listed;
  return [...entries.keys()]
    .filter((name) => /^xl\/worksheets\/sheet\d+\.xml$/.test(name))
    .sort((a, b) => Number(/\d+/.exec(a)?.[0]) - Number(/\d+/.exec(b)?.[0]))
    .map((part) => ({ name: /sheet\d+/.exec(part)?.[0] ?? part, part }));
}

/**
 * Every sheet of an XLSX workbook, in the order the spreadsheet shows them (JC1-21); each first
 * row is the header. Shared and inline strings are read, a date cell as its ISO date. A sheet
 * with no row is left out.
 */
export async function parseWorkbook(bytes: Uint8Array): Promise<Sheet[]> {
  const entries = await unzip(bytes);
  const xml: Xml = (name) => {
    const entry = entries.get(name);
    return entry ? new DOMParser().parseFromString(new TextDecoder().decode(entry), "application/xml") : undefined;
  };
  const shared = Array.from(xml("xl/sharedStrings.xml")?.getElementsByTagName("si") ?? []).map(
    (si) => Array.from(si.getElementsByTagName("t")).map((t) => t.textContent ?? "").join(""),
  );
  const dated = datedStyles(xml("xl/styles.xml"));
  const book = xml("xl/workbook.xml");
  const date1904 = ["1", "true"].includes(book?.getElementsByTagName("workbookPr")[0]?.getAttribute("date1904") ?? "");
  const sheets: Sheet[] = [];
  for (const { name, part } of sheetParts(entries, book, xml)) {
    const sheet = xml(part);
    if (!sheet) continue;
    const records = Array.from(sheet.getElementsByTagName("row")).map((row) => {
      const record: string[] = [];
      for (const cell of Array.from(row.getElementsByTagName("c"))) {
        record[columnIndex(cell.getAttribute("r") ?? "")] = cellValue(cell, shared, dated, date1904);
      }
      return Array.from(record, (cell) => cell ?? "");
    });
    const table = fromRecords(records.filter((r) => r.some((cell) => cell.trim() !== "")));
    if (table.columns.length > 0) sheets.push({ name, table });
  }
  if (sheets.length === 0) throw new Error("the workbook has no sheet");
  return sheets;
}

/** One sheet of an XLSX workbook, the first by default; an unknown name is refused by name. */
export async function parseXlsx(bytes: Uint8Array, sheet?: string): Promise<Table> {
  const sheets = await parseWorkbook(bytes);
  const found = sheet === undefined ? sheets[0] : sheets.find((s) => s.name === sheet);
  if (!found) throw new Error(`the workbook has no sheet '${sheet}'`);
  return found.table;
}

/** A table as CSV text, header first: what a door that takes text (an op, a question) gets of a sheet. */
export function csvOf(table: Table): string {
  const quote = (text: string) => (/[",\n\r]/.test(text) ? `"${text.replace(/"/g, '""')}"` : text);
  return [table.columns, ...table.rows.map((row) => table.columns.map((column) => row[column] ?? ""))]
    .map((record) => record.map(quote).join(","))
    .join("\r\n") + "\r\n";
}

/**
 * A file as its sheets: every sheet of a workbook, or the one table of any other file under the
 * name "" (JC1-07). An upload offers the sheets to choose from when there is more than one.
 */
export async function readSheets(file: File): Promise<Sheet[]> {
  if (file.size > MAX_BYTES) throw new FileTooLarge(file.size);
  if (file.name.toLowerCase().endsWith(".xlsx")) return parseWorkbook(new Uint8Array(await file.arrayBuffer()));
  return [{ name: "", table: await readTable(file) }];
}

/** The text name a sheet travels under to a door that takes text: `stations.csv`, or `Stops.csv` for one sheet of several. */
export function sheetFileName(fileName: string, sheets: Sheet[], sheet: Sheet): string {
  const stem = fileName.replace(/\.[^.]+$/, "") || "sample";
  return `${sheets.length > 1 && sheet.name.trim() !== "" ? sheet.name.trim() : stem}.csv`.replace(/[/\\]/g, "-");
}

// ---------------------------------------------------------------------------------------------
// Mapping columns to slots
// ---------------------------------------------------------------------------------------------

export const SYNONYMS: Record<string, string[]> = {
  // Smart Data Models' common names for what files call otherwise.
  name: ["title", "label", "nazov", "nimi", "nazev"],
  description: ["desc", "popis", "kuvaus", "notes"],
  address: ["street", "adresa", "osoite"],
  dateObserved: ["date", "timestamp", "datum", "observedat"],
  location: ["geometry", "geom", "coordinates", "position", "point"],
};

/** A name without case, separators or diacritics, so `Date observed` meets `dateObserved`. */
export function normalized(name: string): string {
  return name.normalize("NFD").replace(/[̀-ͯ]/g, "").toLowerCase().replace(/[^a-z0-9]/g, "");
}

/** Column names that hold a latitude or a longitude, normalized. */
export const LATITUDES = new Set(["lat", "latitude", "leveys", "sirka", "zemepisnasirka", "breite"]);
export const LONGITUDES = new Set(["lon", "lng", "long", "longitude", "pituus", "dlzka", "delka", "zemepisnadlzka", "lange"]);

/** The slot of a column: the same name or title up to case, separators and diacritics. */
function slotOfColumn(column: string, slots: ImportSlot[]): ImportSlot | undefined {
  const key = normalized(column);
  return slots.find((s) => normalized(s.name) === key || (s.titles ?? []).some((title) => normalized(title) === key));
}

/**
 * The slot each column most likely feeds: the same name up to case, separators and diacritics, or
 * a Smart Data Models synonym of it; a column named `id` or `urn` names the entity. Each slot is
 * suggested once.
 */
export function suggestMapping(columns: string[], slots: ImportSlot[]): Mapping {
  const mapping: Mapping = {};
  const taken = new Set<string>();
  for (const column of columns) {
    const key = normalized(column);
    let target = "";
    const geo = slots.some((s) => s.kind === "GeoProperty");
    if (key === "id" || key === "urn" || key === "entityid") target = ID;
    else if (geo && LATITUDES.has(key)) target = LATITUDE;
    else if (geo && LONGITUDES.has(key)) target = LONGITUDE;
    else {
      const slot =
        slotOfColumn(column, slots) ??
        slots.find((s) => (SYNONYMS[s.name] ?? []).some((synonym) => normalized(synonym) === key));
      target = slot?.name ?? "";
    }
    if (target && !taken.has(target)) {
      mapping[column] = target;
      taken.add(target);
    } else {
      mapping[column] = "";
    }
  }
  return mapping;
}

// ---------------------------------------------------------------------------------------------
// Rows into entities, each checked
// ---------------------------------------------------------------------------------------------

/** Where the rows go: the type, and what completes a local id into the space's URN. */
export interface Target {
  type: string;
  orgDomain: string;
  space: string;
}

/** One row the import will not create, numbered as the file numbers it (the header is row 1). */
export interface Rejected {
  row: number;
  id?: string;
  reason: string;
}

const LOCAL_ID = /^[A-Za-z0-9._~-]{1,128}$/;

/**
 * The entity a row names: a URN of this type as it is, a local id completed. An entity is this
 * space's by where it is written, never by its URN's segments, so any `urn:ngsi-ld:{type}:{nss}`
 * is taken (ADR-N-041, F-R2UI4-004); the space prefix is only minted for a local id.
 */
function idOf(value: string, target: Target, fallback: string): string | Error {
  const text = value.trim();
  if (!text) return `urn:ngsi-ld:${target.type}:${target.orgDomain}:${target.space}:${fallback}`;
  if (text.startsWith("urn:")) {
    if (!ENTITY_ID.test(text)) return new Error(`\`${text}\` is not an NGSI-LD URN`);
    if (text.split(":")[2] !== target.type) return new Error(`\`${text}\` names another type than ${target.type}`);
    return text;
  }
  if (!LOCAL_ID.test(text)) return new Error(`\`${text}\` cannot be the last part of an id`);
  return `urn:ngsi-ld:${target.type}:${target.orgDomain}:${target.space}:${text}`;
}

const INTEGER = new Set(["integer"]);
const NUMBER = new Set(["float", "double", "decimal", "number"]);
const DATE = new Set(["date", "datetime", "time"]);

/** One value as its slot's range takes it, or why it cannot be. */
function valueOf(text: string, slot: ImportSlot, mark?: "." | ","): unknown | Error {
  if (slot.kind === "JsonProperty" || slot.kind === "ListProperty") {
    try {
      const value: unknown = JSON.parse(text);
      if (slot.kind === "ListProperty" && !Array.isArray(value)) throw new Error("not a list");
      return value;
    } catch {
      return slot.kind === "ListProperty"
        ? text.split(/\s*[;|]\s*/).filter((item) => item !== "")
        : new Error(`${slot.name}: \`${text}\` is not JSON`);
    }
  }
  if (slot.kind === "LanguageProperty") {
    // A map of languages as JSON, or one text in no particular language (NGSI-LD `@none`).
    try {
      const map: unknown = JSON.parse(text);
      if (map && typeof map === "object" && !Array.isArray(map) && Object.values(map).every((v) => typeof v === "string")) {
        return map;
      }
    } catch {
      // Plain text below.
    }
    return { "@none": text };
  }
  if (slot.kind === "GeoProperty") {
    try {
      const geometry = JSON.parse(text) as { type?: unknown; coordinates?: unknown };
      if (typeof geometry.type === "string" && Array.isArray(geometry.coordinates)) return geometry;
    } catch {
      // A pair below.
    }
    const wkt = /^POINT\s*\(\s*(-?[\d.]+)\s+(-?[\d.]+)\s*\)$/i.exec(text);
    if (wkt) return { type: "Point", coordinates: [Number(wkt[1]), Number(wkt[2])] };
    const pair = text.split(/[,;\s]+/).map(Number);
    if (pair.length === 2 && pair.every(Number.isFinite)) return { type: "Point", coordinates: pair };
    return new Error(`${slot.name}: \`${text}\` is neither GeoJSON nor "longitude, latitude"`);
  }
  const range = slot.range ?? "string";
  if (slot.values) {
    if (!slot.values.includes(text)) return new Error(`${slot.name}: \`${text}\` is not one of ${slot.values.join(", ")}`);
    return text;
  }
  if (INTEGER.has(range) || NUMBER.has(range)) {
    const number = numberOf(text, mark);
    if (!Number.isFinite(number) || (INTEGER.has(range) && !Number.isInteger(number))) {
      return new Error(`${slot.name}: \`${text}\` is not ${INTEGER.has(range) ? "a whole number" : "a number"}`);
    }
    if (slot.minimum !== undefined && number < slot.minimum) return new Error(`${slot.name}: ${number} is below ${slot.minimum}`);
    if (slot.maximum !== undefined && number > slot.maximum) return new Error(`${slot.name}: ${number} is above ${slot.maximum}`);
    return number;
  }
  if (range === "boolean") {
    // `áno` is `ano` once folded: a Slovak sheet's yes is no misspelt one (T-6567).
    const key = normalized(text);
    if (["true", "1", "yes", "ano", "kylla", "ja"].includes(key)) return true;
    if (["false", "0", "no", "nie", "ne", "ei", "nein"].includes(key)) return false;
    return new Error(`${slot.name}: \`${text}\` is not yes or no`);
  }
  if (DATE.has(range)) {
    const iso = isoDateOf(text);
    if (Number.isNaN(Date.parse(iso))) return new Error(`${slot.name}: \`${text}\` is not a date`);
    return range === "date" ? iso.slice(0, 10) : new Date(iso).toISOString();
  }
  return text;
}

/**
 * An entity's `expiresAt` from a cell or the fixed one, in UTC: refused when it is no time, or a
 * time already past, which would delete the entity the moment it is written.
 */
export function expiresOf(text: string | undefined, now: number = Date.now()): string | undefined | Error {
  if (text === undefined || text === "") return undefined;
  const ms = Date.parse(isoDateOf(text));
  if (Number.isNaN(ms)) return new Error(`expiresAt: \`${text}\` is not a date and time`);
  if (ms <= now) return new Error(`expiresAt: ${new Date(ms).toISOString()} is already past`);
  return new Date(ms).toISOString();
}

/** A 53-bit hash of a text (cyrb53): an id, never a secret. */
function hashOf(text: string): string {
  let h1 = 0xdeadbeef;
  let h2 = 0x41c6ce57;
  for (let at = 0; at < text.length; at += 1) {
    const code = text.charCodeAt(at);
    h1 = Math.imul(h1 ^ code, 2654435761);
    h2 = Math.imul(h2 ^ code, 1597334677);
  }
  h1 = Math.imul(h1 ^ (h1 >>> 16), 2246822507) ^ Math.imul(h2 ^ (h2 >>> 13), 3266489909);
  h2 = Math.imul(h2 ^ (h2 >>> 16), 2246822507) ^ Math.imul(h1 ^ (h1 >>> 13), 3266489909);
  return (4294967296 * (2097151 & h2) + (h1 >>> 0)).toString(36);
}

/**
 * The `freshId` of {@link toEntities} for a load that may run twice: called once per row in order,
 * it names a row by its content, so the same file written again updates the same entities (a row
 * the file holds twice stays two). A column mapped to `id` still wins.
 */
export function rowIds(table: Table): () => string {
  const seen = new Map<string, number>();
  let at = 0;
  return () => {
    const hash = hashOf(JSON.stringify(table.columns.map((column) => table.rows[at]?.[column] ?? "")));
    at += 1;
    const times = (seen.get(hash) ?? 0) + 1;
    seen.set(hash, times);
    return times === 1 ? hash : `${hash}-${times}`;
  };
}

/**
 * Every row as an NGSI-LD entity of the target type, or the reasons it is not one: an id that
 * names another type or space, a required slot left empty, a value outside its slot's range or
 * enum. A row with no id column gets a fresh local id.
 */
export function toEntities(
  table: Table,
  mapping: Mapping,
  slots: ImportSlot[],
  target: Target,
  freshId: () => string = () => crypto.randomUUID(),
  /** One `expiresAt` for every row (T-4822); a column mapped to `EXPIRES_AT` wins for its row. */
  expiresAt?: string,
  now: number = Date.now(),
): { entities: { row: number; entity: Record<string, unknown> }[]; rejected: Rejected[] } {
  const entities: { row: number; entity: Record<string, unknown> }[] = [];
  const rejected: Rejected[] = [];
  const idColumn = Object.keys(mapping).find((column) => mapping[column] === ID);
  const bySlot = new Map(slots.map((slot) => [slot.name, slot]));
  const seen = new Set<string>();
  const mark = decimalMarkOf(table);
  table.rows.forEach((record, at) => {
    const row = at + 2;
    const id = idOf(idColumn ? (record[idColumn] ?? "") : "", target, freshId());
    if (id instanceof Error) {
      rejected.push({ row, reason: id.message });
      return;
    }
    if (seen.has(id)) {
      rejected.push({ row, id, reason: "the file names this id twice" });
      return;
    }
    const entity: Record<string, unknown> = { id, type: target.type };
    const problems: string[] = [];
    // A slot whose value was refused is reported once, as refused, never again as missing.
    const refusedSlots = new Set<string>();
    for (const [column, slotName] of Object.entries(mapping)) {
      const slot = bySlot.get(slotName);
      const text = (record[column] ?? "").trim();
      if (!slot || text === "") continue;
      const value = valueOf(text, slot, mark);
      if (value instanceof Error) {
        problems.push(value.message);
        refusedSlots.add(slot.name);
      } else if (slot.kind === "Relationship") {
        entity[slot.name] = { type: "Relationship", object: value };
      } else if (slot.kind === "GeoProperty") {
        entity[slot.name] = { type: "GeoProperty", value };
      } else if (slot.kind === "LanguageProperty") {
        entity[slot.name] = { type: "LanguageProperty", languageMap: value };
      } else if (slot.kind === "JsonProperty") {
        entity[slot.name] = { type: "JsonProperty", json: value };
      } else if (slot.kind === "VocabProperty") {
        entity[slot.name] = { type: "VocabProperty", vocab: value };
      } else if (slot.kind === "ListProperty") {
        entity[slot.name] = { type: "ListProperty", valueList: value };
      } else {
        entity[slot.name] = { type: "Property", value };
      }
    }
    const latColumn = Object.keys(mapping).find((column) => mapping[column] === LATITUDE);
    const lonColumn = Object.keys(mapping).find((column) => mapping[column] === LONGITUDE);
    const geoSlot = slots.find((slot) => slot.name === "location" && slot.kind === "GeoProperty") ?? slots.find((slot) => slot.kind === "GeoProperty");
    if (latColumn && lonColumn && geoSlot) {
      const lat = numberOf((record[latColumn] ?? "").trim(), mark);
      const lon = numberOf((record[lonColumn] ?? "").trim(), mark);
      if (Number.isFinite(lat) && Number.isFinite(lon) && Math.abs(lat) <= 90 && Math.abs(lon) <= 180) {
        entity[geoSlot.name] = { type: "GeoProperty", value: { type: "Point", coordinates: [lon, lat] } };
        // A model inferred from the file keeps each coordinate column as a slot of its own, required.
        for (const [column, value] of [[latColumn, lat], [lonColumn, lon]] as const) {
          const own = slotOfColumn(column, slots);
          if (own && own.kind === "Property") entity[own.name] = { type: "Property", value };
        }
      } else if ((record[latColumn] ?? "").trim() !== "" || (record[lonColumn] ?? "").trim() !== "") {
        problems.push(`${geoSlot.name}: \`${record[latColumn] ?? ""}\`, \`${record[lonColumn] ?? ""}\` is not a latitude and a longitude`);
      }
    }
    const expiresColumn = Object.keys(mapping).find((column) => mapping[column] === EXPIRES_AT);
    const expires = expiresOf(expiresColumn ? (record[expiresColumn] ?? "").trim() || expiresAt : expiresAt, now);
    if (expires instanceof Error) problems.push(expires.message);
    else if (expires !== undefined) entity.expiresAt = expires;
    for (const slot of slots) {
      if (slot.required && entity[slot.name] === undefined && !refusedSlots.has(slot.name)) problems.push(`${slot.name} is required`);
    }
    if (problems.length > 0) {
      rejected.push({ row, id, reason: problems.join("; ") });
      return;
    }
    seen.add(id);
    entities.push({ row, entity });
  });
  return { entities, rejected };
}

// ---------------------------------------------------------------------------------------------
// Writing through the gateway
// ---------------------------------------------------------------------------------------------

/** The request the import makes, the shape of the SDK's origin transport. */
export type Send = (request: { method: "POST"; path: string; body?: unknown; reason?: string }) => Promise<{ status: number; body?: unknown }>;

/** What the gateway's problem says, or the status when it says nothing. */
function said(body: unknown, status: number): string {
  const problem = (body ?? {}) as { detail?: unknown; title?: unknown };
  return String(problem.detail ?? problem.title ?? `HTTP ${status}`);
}

/**
 * Creates the entities in batches through the space surface with the person's session. A batch
 * the gateway answers 201 is created whole; a 207 names which ids failed and why (NGSI-LD batch
 * result); a 401 or 403 stops the import and names the refusal as `denied`; any other answer
 * refuses the batch whole with the gateway's words. `onProgress` is told how many have been sent.
 */
export async function createInBatches(
  send: Send,
  space: string,
  entities: { row: number; entity: Record<string, unknown> }[],
  onProgress: (sent: number) => void = () => {},
  // `upsert` makes the same file imported twice the same entities, not a refusal (T-3840).
  operation: "create" | "upsert" = "create",
  // Every write from the Portal's UI names why, sent as `JC-Reason` (DM-113).
  reason = "imported from a file",
  // Where the writes go: the space's own surface, or an Endpoint of the space.
  surface = `/cs/${encodeURIComponent(space)}`,
): Promise<{ created: number; rejected: Rejected[]; denied?: string }> {
  let created = 0;
  const rejected: Rejected[] = [];
  const path = `${surface}/ngsi-ld/v1/entityOperations/${operation}`;
  for (let at = 0; at < entities.length; at += BATCH) {
    const batch = entities.slice(at, at + BATCH);
    let answer: { status: number; body?: unknown };
    try {
      answer = await send({ method: "POST", path, body: batch.map((item) => item.entity), reason });
    } catch (error) {
      answer = { status: 0, body: { detail: error instanceof Error ? error.message : String(error) } };
    }
    const rowOf = new Map(batch.map((item) => [String(item.entity.id), item.row]));
    if (answer.status === 201 || answer.status === 204) {
      created += batch.length;
    } else if (answer.status === 207) {
      const result = (answer.body ?? {}) as { success?: unknown[]; errors?: { entityId?: unknown; error?: unknown }[] };
      const failed = new Set<string>();
      for (const error of result.errors ?? []) {
        const id = String(error.entityId ?? "");
        failed.add(id);
        rejected.push({ row: rowOf.get(id) ?? 0, id, reason: said(error.error, 207) });
      }
      created += batch.filter((item) => !failed.has(String(item.entity.id))).length;
    } else if (answer.status === 401 || answer.status === 403) {
      // The person may not create these entities at all: the next batch would be refused the same
      // way, so nothing more is sent and every row left is reported with the one reason.
      const reason = said(answer.body, answer.status);
      for (const item of entities.slice(at)) rejected.push({ row: item.row, id: String(item.entity.id), reason });
      onProgress(entities.length);
      return { created, rejected, denied: reason };
    } else {
      const reason = said(answer.body, answer.status);
      for (const item of batch) rejected.push({ row: item.row, id: String(item.entity.id), reason });
    }
    onProgress(Math.min(at + BATCH, entities.length));
  }
  return { created, rejected };
}

/** The rejected rows as CSV, for the person to fix and import again. */
export function rejectedCsv(rejected: Rejected[]): string {
  // A cell a spreadsheet would run as a formula (`=`, `+`, `-`, `@`, tab, CR) is written as text.
  const inert = (text: string) => (/^[=+\-@\t\r]/.test(text) ? `'${text}` : text);
  const quote = (raw: string) => {
    const text = inert(raw);
    return /[",\n\r]/.test(text) ? `"${text.replace(/"/g, '""')}"` : text;
  };
  return ["row,id,reason", ...rejected.map((r) => [String(r.row), r.id ?? "", r.reason].map(quote).join(","))].join("\r\n");
}

// ---------------------------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------------------------

/** The file representations an export offers, in the order the menu lists them. */
/** The one file an Endpoint publishes (T-4600, T-4597): its other files are gone. */
export const EXPORTS = ["csv"] as const;

/**
 * Where an Endpoint's file of the view is: its `file.{format}` narrowed to the type, the view's
 * query and the type's fields, read by the gateway with the person's session at the edge.
 */
/** Checked rows one export names at most: their ids travel in the address. */
export const MAX_EXPORT_IDS = 100;

export function exportUrl(
  slug: string,
  format: (typeof EXPORTS)[number],
  type: string,
  q: string | undefined,
  attrs: string[],
  ids: readonly string[] = [],
  idPattern?: string,
): string {
  const params = new URLSearchParams({ type });
  // Checked rows go as their ids (T-5186): the gateway reads them through the view like a filter.
  if (ids.length > 0) params.set("id", ids.join(","));
  else {
    if (q) params.set("q", q);
    // The grid's ID-column filter, so the file holds the rows the grid shows (T-6107).
    if (idPattern) params.set("idPattern", idPattern);
  }
  if (attrs.length > 0) params.set("attrs", attrs.join(","));
  return `/api/endpoint/${encodeURIComponent(slug)}/file.${format}?${params.toString()}`;
}
