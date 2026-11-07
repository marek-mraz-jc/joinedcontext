import { asCsv } from "../src/grid/EntityHistory";
import { afterEach, beforeEach, describe, expect, it, vi, type Mock } from "vitest";
import {
  csvText,
  download,
  pdfText,
  stamp,
  toCsv,
  toGeoJson,
  toPdf,
  toPng,
} from "../src/artifact";
import type { Row } from "../src/ngsi";

const SAMPLE_ROWS: Row[] = [
  {
    id: "urn:ngsi-ld:Station:001",
    type: "Station",
    name: "Kaivopuisto",
    bikes: 12,
    notes: 'South shore, "busy"',
    location: { type: "Point", coordinates: [24.95, 60.155] },
  },
  {
    id: "urn:ngsi-ld:Station:002",
    type: "Station",
    name: "Kamppi\nTerminal",
    bikes: 0,
    notes: "Central hub",
    location: { type: "Point", coordinates: [24.93, 60.17] },
  },
  {
    id: "urn:ngsi-ld:Station:003",
    type: "Station",
    name: "Ghost Station",
    bikes: null,
    notes: null,
    location: null,
  },
];

const COLUMNS = ["name", "bikes", "notes"];

describe("csv exports", () => {
  it("opens a cell a spreadsheet would run as a formula as text, and keeps a negative number (T-6230)", () => {
    const lines = csvText(["a", "b"], [
      { id: "u1", type: "T", a: '=HYPERLINK("https://x.example","y")', b: -3.5 },
      { id: "u2", type: "T", a: "@SUM(A1)", b: "+cmd|' /C calc'!A0" },
    ]).slice(1).split("\r\n");
    expect(lines[1]).toBe(`"'=HYPERLINK(""https://x.example"",""y"")",-3.5`);
    expect(lines[2]).toBe("'@SUM(A1),'+cmd|' /C calc'!A0");
    expect(asCsv([{ at: "t", value: "=1+1" }], "n")).toBe("observedAt,n\nt,'=1+1");
  });

  it("generates CSV text starting with a UTF-8 BOM, header line, and RFC 4180 quoting", () => {
    const text = csvText(COLUMNS, SAMPLE_ROWS);

    // UTF-8 BOM is required for spreadsheet compatibility
    expect(text.startsWith("\uFEFF")).toBe(true);

    const lines = text.slice(1).split("\r\n");
    expect(lines[0]).toBe("name,bikes,notes");

    // Line 1: embedded comma and escaped quotes
    expect(lines[1]).toBe('Kaivopuisto,12,"South shore, ""busy"""');

    // Line 2: embedded newline
    expect(text).toContain('"Kamppi\nTerminal",0,Central hub');

    // Line 3: null values formatted as empty strings
    expect(text).toContain("Ghost Station,,");
  });

  it("handles empty columns or empty rows gracefully", () => {
    expect(csvText([], SAMPLE_ROWS)).toBe("\uFEFF\r\n\r\n\r\n");
    expect(csvText(COLUMNS, [])).toBe("\uFEFFname,bikes,notes");
  });

  it("produces a Blob with text/csv mime type matching csvText output", async () => {
    const blob = toCsv(COLUMNS, SAMPLE_ROWS);
    expect(blob).toBeInstanceOf(Blob);
    expect(blob.type).toBe("text/csv;charset=utf-8");

    const bytes = new Uint8Array(await blob.arrayBuffer());
    expect(Array.from(bytes.slice(0, 3))).toEqual([0xef, 0xbb, 0xbf]);
    expect(new TextDecoder("utf-8", { ignoreBOM: true }).decode(bytes)).toBe(
      csvText(COLUMNS, SAMPLE_ROWS),
    );
  });
});

describe("geojson exports", () => {
  it("converts rows into a GeoJSON FeatureCollection, setting feature id and skipping non-geo rows", () => {
    const geo = toGeoJson(SAMPLE_ROWS, "location");

    expect(geo.type).toBe("FeatureCollection");
    expect(Array.isArray(geo.features)).toBe(true);
    // Station 003 has location null and must be skipped
    expect(geo.features).toHaveLength(2);

    expect(geo.features[0]).toEqual({
      type: "Feature",
      id: "urn:ngsi-ld:Station:001",
      geometry: {
        type: "Point",
        coordinates: [24.95, 60.155],
      },
      properties: {
        id: "urn:ngsi-ld:Station:001",
        type: "Station",
        name: "Kaivopuisto",
        bikes: 12,
        notes: 'South shore, "busy"',
        location: { type: "Point", coordinates: [24.95, 60.155] },
      },
    });

    expect(geo.features[1].id).toBe("urn:ngsi-ld:Station:002");
    expect(geo.features[1].geometry).toEqual({
      type: "Point",
      coordinates: [24.93, 60.17],
    });
  });

  it("returns an empty FeatureCollection when rows is empty", () => {
    const geo = toGeoJson([], "location");
    expect(geo).toEqual({ type: "FeatureCollection", features: [] });
  });
});

describe("pdf exports and stamps", () => {
  const PDF_INPUT = {
    title: "Air Quality Summary",
    endpoint: "helsinki-air-quality",
    filters: "district=Kamppi",
    takenAt: "2026-09-12T10:00:00Z",
    attribution: "© OpenStreetMap contributors, CartoDB",
    lines: [
      "Station Kaivopuisto: PM10 = 12, PM2.5 = 6",
      "Station Kamppi: PM10 = 24, PM2.5 = 14",
    ],
  };

  it("generates a valid single-page PDF 1.4 document containing title, endpoint, stamp, and attribution", () => {
    const pdf = pdfText(PDF_INPUT);

    expect(pdf.startsWith("%PDF-1.4")).toBe(true);
    expect(pdf.trimEnd().endsWith("%%EOF")).toBe(true);

    expect(pdf).toContain("Air Quality Summary");
    expect(pdf).toContain("helsinki-air-quality");
    expect(pdf).toContain("district=Kamppi");
    expect(pdf).toContain("2026-09-12T10:00:00Z");
    // © is WinAnsi 0xA9, written as the octal escape \251.
    expect(pdf).toContain("\\251 OpenStreetMap contributors, CartoDB");
    expect(pdf).toContain("Station Kaivopuisto");
    expect(pdf).toContain("Station Kamppi");
  });

  it("omits attribution cleanly when not provided", () => {
    const withoutAttribution = {
      title: "Quick Report",
      endpoint: "bikes",
      filters: "all",
      takenAt: "2026-09-12T12:00:00Z",
      lines: ["Line 1"],
    };
    const pdf = pdfText(withoutAttribution);

    expect(pdf.startsWith("%PDF-1.4")).toBe(true);
    expect(pdf.trimEnd().endsWith("%%EOF")).toBe(true);
    expect(pdf).toContain("Quick Report");
  });

  it("toPdf produces an application/pdf Blob matching pdfText", async () => {
    const blob = toPdf(PDF_INPUT);
    expect(blob).toBeInstanceOf(Blob);
    expect(blob.type).toBe("application/pdf");

    const text = await blob.text();
    expect(text).toBe(pdfText(PDF_INPUT));
  });

  it("stamp creates a traceable metadata string with endpoint, filters, and time", () => {
    const s = stamp("ep-bikes", "bikes >= 5", "2026-09-12T15:30:00Z");
    expect(s).toContain("ep-bikes");
    expect(s).toContain("bikes >= 5");
    expect(s).toContain("2026-09-12T15:30:00Z");
  });
});

describe("canvas png and browser download", () => {
  let createObjectURLSpy: Mock<(obj: Blob | MediaSource) => string>;
  let revokeObjectURLSpy: Mock<(url: string) => void>;

  beforeEach(() => {
    createObjectURLSpy = vi.fn((_obj: Blob | MediaSource) => "blob:http://localhost/test-uuid");
    revokeObjectURLSpy = vi.fn((_url: string) => undefined);
    window.URL.createObjectURL = createObjectURLSpy;
    window.URL.revokeObjectURL = revokeObjectURLSpy;
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("toPng converts an HTMLCanvasElement to a PNG Blob via toBlob", async () => {
    const canvas = document.createElement("canvas");
    canvas.width = 100;
    canvas.height = 100;
    const fakeBlob = new Blob(["fake png"], { type: "image/png" });

    canvas.toBlob = vi.fn((callback: BlobCallback) => {
      callback(fakeBlob);
    });

    const result = await toPng(canvas);
    expect(canvas.toBlob).toHaveBeenCalledWith(expect.any(Function), "image/png");
    expect(result).toBe(fakeBlob);
  });

  it("toPng rejects if toBlob fails to produce a Blob", async () => {
    const canvas = document.createElement("canvas");
    canvas.toBlob = vi.fn((callback: BlobCallback) => {
      callback(null);
    });

    await expect(toPng(canvas)).rejects.toThrow("Failed to capture canvas as PNG");
  });

  it("download triggers a simulated click on an anchor with an object URL and cleans up", () => {
    const clickSpy = vi.fn();
    const origCreateElement = document.createElement.bind(document);
    vi.spyOn(document, "createElement").mockImplementation((tag: string) => {
      const el = origCreateElement(tag);
      if (tag === "a") {
        el.click = clickSpy;
      }
      return el;
    });

    const blob = new Blob(["hello world"], { type: "text/plain" });
    download(blob, "report.csv");

    expect(createObjectURLSpy).toHaveBeenCalledWith(blob);
    expect(clickSpy).toHaveBeenCalled();
    expect(revokeObjectURLSpy).toHaveBeenCalledWith("blob:http://localhost/test-uuid");
  });
});

describe("network isolation (AP-66)", () => {
  it("never executes fetch during any artifact generation or export", async () => {
    const fetchSpy = vi.spyOn(globalThis, "fetch");

    csvText(COLUMNS, SAMPLE_ROWS);
    toCsv(COLUMNS, SAMPLE_ROWS);
    toGeoJson(SAMPLE_ROWS, "location");
    pdfText({
      title: "Test",
      endpoint: "ep",
      filters: "none",
      takenAt: "2026-09-12T10:00:00Z",
      lines: ["Row 1"],
    });
    toPdf({
      title: "Test",
      endpoint: "ep",
      filters: "none",
      takenAt: "2026-09-12T10:00:00Z",
      lines: ["Row 1"],
    });
    stamp("ep", "none", "2026-09-12T10:00:00Z");

    const canvas = document.createElement("canvas");
    canvas.toBlob = (cb) => cb(new Blob([""], { type: "image/png" }));
    await toPng(canvas);

    download(new Blob(["content"]), "file.csv");

    expect(fetchSpy).not.toHaveBeenCalled();
  });
});
