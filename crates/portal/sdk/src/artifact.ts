import type { Cell, Row } from "./ngsi";
import { format, pointOf } from "./ngsi";

export interface GeoJsonFeature {
  type: "Feature";
  id: string;
  geometry: {
    type: "Point";
    coordinates: [number, number];
  };
  properties: Record<string, Cell>;
}

export interface GeoJsonCollection {
  type: "FeatureCollection";
  features: GeoJsonFeature[];
}

export interface PdfInput {
  title: string;
  endpoint: string;
  filters: string;
  takenAt: string;
  attribution?: string;
  lines: string[];
}

/**
 * One CSV cell, RFC 4180 quoted. A text a spreadsheet would run as a formula (`=`, `+`, `-`, `@`,
 * a tab or a carriage return first) gets a leading `'`: the data comes from other writers, and a
 * cell such as `=HYPERLINK(…)` must open as text (T-6230). A number such as `-3.5` stays a number.
 */
export function csvCell(val: string): string {
  const text = /^[=+\-@\t\r]/.test(val) && !Number.isFinite(Number(val)) ? `'${val}` : val;
  return /[",\r\n]/.test(text) ? `"${text.replace(/"/g, '""')}"` : text;
}

/** Formats rows into RFC 4180 CSV text with UTF-8 BOM. */
export function csvText(columns: string[], rows: Row[]): string {
  const header = columns.map(csvCell).join(",");
  const dataLines = rows.map((row) =>
    columns.map((col) => csvCell(format(row[col]))).join(","),
  );
  return `\uFEFF${[header, ...dataLines].join("\r\n")}`;
}

/** In-browser CSV artifact synthesized from rendered rows. */
export function toCsv(columns: string[], rows: Row[]): Blob {
  return new Blob([csvText(columns, rows)], { type: "text/csv;charset=utf-8" });
}

/** Synthesizes a GeoJSON FeatureCollection from rows containing a point. */
export function toGeoJson(rows: Row[], locationAttr: string): GeoJsonCollection {
  const features: GeoJsonFeature[] = [];
  for (const row of rows) {
    const pt = pointOf(row[locationAttr]);
    if (!pt) continue;
    features.push({
      type: "Feature",
      id: row.id,
      geometry: {
        type: "Point",
        coordinates: pt,
      },
      properties: { ...row },
    });
  }
  return { type: "FeatureCollection", features };
}

/** Hand-crafted minimal valid single-page PDF 1.4 using standard Helvetica. */
export function pdfText(input: PdfInput): string {
  const maxLines = input.attribution ? 35 : 37;
  const displayLines = input.lines.slice(0, maxLines);
  // WinAnsiEncoding matches Latin-1 from U+00A0 to U+00FF, written as octal escapes so the
  // file stays ASCII and every offset below counts bytes; other characters print as "?".
  const escapePdf = (text: string): string =>
    Array.from(text, (ch) => {
      const code = ch.codePointAt(0) ?? 0x3f;
      if (ch === "\\" || ch === "(" || ch === ")") return `\\${ch}`;
      if (code >= 0x20 && code <= 0x7e) return ch;
      if (code >= 0xa0 && code <= 0xff) return `\\${code.toString(8)}`;
      return "?";
    }).join("");

  const streamLines: string[] = [
    "BT",
    "/F1 16 Tf",
    "50 742 Td",
    `(${escapePdf(input.title)}) Tj`,
    "/F1 9 Tf",
    "0 -18 Td",
    `(${escapePdf(`Endpoint: ${input.endpoint}`)}) Tj`,
    "0 -12 Td",
    `(${escapePdf(`Exported: ${input.takenAt} | Filters: ${input.filters || "none"}`)}) Tj`,
  ];
  if (input.attribution) {
    streamLines.push(
      "0 -12 Td",
      `(${escapePdf(`Attribution: ${input.attribution}`)}) Tj`,
    );
  }
  streamLines.push("0 -16 Td", "/F1 9 Tf");
  for (const line of displayLines) {
    streamLines.push("0 -12 Td", `(${escapePdf(line)}) Tj`);
  }
  streamLines.push("ET");

  const streamContent = streamLines.join("\n");
  const obj1 = "1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n";
  const obj2 = "2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n";
  const obj3 =
    "3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>\nendobj\n";
  const obj4 = `4 0 obj\n<< /Length ${streamContent.length} >>\nstream\n${streamContent}\nendstream\nendobj\n`;
  const obj5 = "5 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>\nendobj\n";

  const header = "%PDF-1.4\n";
  const offset1 = header.length;
  const offset2 = offset1 + obj1.length;
  const offset3 = offset2 + obj2.length;
  const offset4 = offset3 + obj3.length;
  const offset5 = offset4 + obj4.length;
  const xrefOffset = offset5 + obj5.length;

  const pad = (n: number) => String(n).padStart(10, "0");
  const xref = `xref\n0 6\n0000000000 65535 f \n${pad(offset1)} 00000 n \n${pad(offset2)} 00000 n \n${pad(offset3)} 00000 n \n${pad(offset4)} 00000 n \n${pad(offset5)} 00000 n \n`;
  const trailer = `trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n${xrefOffset}\n%%EOF\n`;

  return `${header}${obj1}${obj2}${obj3}${obj4}${obj5}${xref}${trailer}`;
}

/** In-browser PDF export containing endpoint stamp and rendered lines. */
export function toPdf(input: PdfInput): Blob {
  return new Blob([pdfText(input)], { type: "application/pdf" });
}

/** Converts a canvas element into a PNG Blob. */
export function toPng(canvas: HTMLCanvasElement): Promise<Blob> {
  return new Promise((resolve, reject) => {
    if (typeof canvas.toBlob === "function") {
      canvas.toBlob((blob) => {
        if (blob) {
          resolve(blob);
        } else {
          reject(new Error("Failed to capture canvas as PNG"));
        }
      }, "image/png");
    } else {
      try {
        const dataUrl = canvas.toDataURL("image/png");
        const parts = dataUrl.split(",");
        const bytes = atob(parts[1] || "");
        const arr = new Uint8Array(bytes.length);
        for (let i = 0; i < bytes.length; i++) {
          arr[i] = bytes.charCodeAt(i);
        }
        resolve(new Blob([arr], { type: "image/png" }));
      } catch (err) {
        reject(err);
      }
    }
  });
}

/** Client-side file download trigger without network requests. */
export function download(blob: Blob, filename: string): void {
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = filename;
  document.body.appendChild(a);
  a.click();
  document.body.removeChild(a);
  URL.revokeObjectURL(url);
}

/** Formats an audit stamp of endpoint name, active filters, and timestamp. */
export function stamp(endpoint: string, filters: string, takenAt: string): string {
  return [endpoint, filters, takenAt].filter(Boolean).join(" · ");
}
