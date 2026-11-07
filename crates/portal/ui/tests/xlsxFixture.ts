/** ZIP and XLSX builders for tests: the smallest workbooks the Portal's reader takes (JC1-07). */

/** Raw DEFLATE of a text, as a ZIP entry of method 8 holds it. */
export async function deflated(text: string): Promise<Uint8Array> {
  const body = new Response(text).body;
  if (!body) throw new Error("a Response of a text has a body");
  return new Uint8Array(await new Response(body.pipeThrough(new CompressionStream("deflate-raw"))).arrayBuffer());
}

/** A ZIP of stored (uncompressed) entries, or deflated ones given as bytes, the smallest workbook a test can hold. */
export function zip(files: Record<string, string | Uint8Array>, sizes: Record<string, number> = {}): Uint8Array {
  const encoder = new TextEncoder();
  const parts: number[] = [];
  const central: number[] = [];
  const u16 = (out: number[], n: number) => out.push(n & 0xff, (n >> 8) & 0xff);
  const u32 = (out: number[], n: number) => out.push(n & 0xff, (n >> 8) & 0xff, (n >> 16) & 0xff, (n >>> 24) & 0xff);
  let count = 0;
  for (const [name, text] of Object.entries(files)) {
    const nameBytes = encoder.encode(name);
    const data = typeof text === "string" ? encoder.encode(text) : text;
    const method = typeof text === "string" ? 0 : 8;
    const size = sizes[name] ?? data.length;
    const offset = parts.length;
    u32(parts, 0x04034b50);
    [20, 0, method, 0, 0].forEach((n) => u16(parts, n));
    [0, data.length, size].forEach((n) => u32(parts, n));
    u16(parts, nameBytes.length);
    u16(parts, 0);
    parts.push(...nameBytes, ...data);
    u32(central, 0x02014b50);
    [20, 20, 0, method, 0, 0].forEach((n) => u16(central, n));
    [0, data.length, size].forEach((n) => u32(central, n));
    [nameBytes.length, 0, 0, 0, 0].forEach((n) => u16(central, n));
    u32(central, 0);
    u32(central, offset);
    central.push(...nameBytes);
    count += 1;
  }
  const end: number[] = [];
  u32(end, 0x06054b50);
  [0, 0, count, count].forEach((n) => u16(end, n));
  u32(end, central.length);
  u32(end, parts.length);
  u16(end, 0);
  return new Uint8Array([...parts, ...central, ...end]);
}

/** A cell of `workbook`: text (a shared string), a number, or a date serial shown with a date format. */
export type Cell = string | number | { date: number };

/**
 * A whole .xlsx as a spreadsheet saves one: the workbook's sheet list and relationships, shared
 * strings, a styles part whose second cell format is a date, and one worksheet per sheet, its
 * entries in the archive in the reverse of the workbook's order, so only the list orders them.
 */
export function workbook(sheets: Record<string, Cell[][]>): Uint8Array {
  const strings: string[] = [];
  const shared = (text: string) => {
    const at = strings.indexOf(text);
    return at >= 0 ? at : strings.push(text) - 1;
  };
  const letter = (n: number) => String.fromCharCode(65 + n);
  const escape = (text: string) => text.replace(/&/g, "&amp;").replace(/</g, "&lt;");
  const names = Object.keys(sheets);
  const parts: Record<string, string> = {};
  names.forEach((name, at) => {
    const rows = (sheets[name] ?? [])
      .map((record, r) =>
        `<row r="${r + 1}">${record
          .map((cell, c) => {
            const ref = `${letter(c)}${r + 1}`;
            if (typeof cell === "string") return `<c r="${ref}" t="s"><v>${shared(cell)}</v></c>`;
            if (typeof cell === "number") return `<c r="${ref}"><v>${cell}</v></c>`;
            return `<c r="${ref}" s="1"><v>${cell.date}</v></c>`;
          })
          .join("")}</row>`,
      )
      .join("");
    parts[`xl/worksheets/sheet${at + 1}.xml`] = `<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>${rows}</sheetData></worksheet>`;
  });
  const ns = 'xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"';
  const files: Record<string, string> = {
    "xl/workbook.xml": `<workbook ${ns}><sheets>${names.map((name, at) => `<sheet name="${escape(name)}" sheetId="${at + 1}" r:id="rId${at + 1}"/>`).join("")}</sheets></workbook>`,
    "xl/_rels/workbook.xml.rels": `<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">${names.map((_, at) => `<Relationship Id="rId${at + 1}" Target="worksheets/sheet${at + 1}.xml"/>`).join("")}</Relationships>`,
    "xl/styles.xml": `<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><numFmts count="1"><numFmt numFmtId="164" formatCode="d.m.yyyy"/></numFmts><cellXfs count="2"><xf numFmtId="0"/><xf numFmtId="164"/></cellXfs></styleSheet>`,
    ...Object.fromEntries(Object.entries(parts).reverse()),
  };
  files["xl/sharedStrings.xml"] = `<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">${strings.map((text) => `<si><t>${escape(text)}</t></si>`).join("")}</sst>`;
  return zip(files);
}

/** A workbook as the File a picker hands over. */
export function xlsxFile(name: string, sheets: Record<string, Cell[][]>): File {
  return new File([new Uint8Array(workbook(sheets))], name, { type: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" });
}
