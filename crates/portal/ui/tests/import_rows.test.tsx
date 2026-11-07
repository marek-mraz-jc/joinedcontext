/**
 * Import and export on a space's data (T-3109, ADR-N-042 §3.6): a file read in the browser, its
 * columns mapped to the type's slots, every row checked, the valid ones created in batches through
 * the gateway with the person's session, and every refused row reported.
 */
import { describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import en from "../src/locales/en.json";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import i18n from "../src/i18n";
import { ExportLinks, ImportRowsDialog } from "../src/pages/spaces/ImportRows";
import { deflated, workbook, zip } from "./xlsxFixture";
import {
  BATCH,
  createInBatches,
  decodeText,
  detectedOf,
  isoDateOf,
  numberOf,
  exportUrl,
  importSlotsOf,
  parseCsv,
  parseJson,
  csvOf,
  isDateFormat,
  cellValue,
  serialDate,
  parseWorkbook,
  parseXlsx,
  parseXml,
  readTable,
  rejectedCsv,
  suggestMapping,
  toEntities,
  unreadableText,
  FileTooLarge,
  MAX_BYTES,
} from "../src/pages/spaces/importRows";
import type { ImportSlot, Send } from "../src/pages/spaces/importRows";
import type { Manifest } from "../src/api/manifest";

const MODEL = `id: https://hel.fi/models/stations
name: stations
default_prefix: hel
prefixes:
  hel: https://hel.fi/models/
enums:
  StatusEnum:
    permissible_values:
      working: {}
      outOfService: {}
classes:
  BikeHireDockingStation:
    slots: [name, availableBikeNumber, status, location, refStreet, dateObserved, open]
slots:
  name: { range: string, required: true }
  availableBikeNumber: { range: integer, minimum_value: 0 }
  status: { range: StatusEnum }
  location:
    annotations: { ngsi_ld_kind: GeoProperty }
  refStreet:
    range: Street
    annotations: { ngsi_ld_kind: Relationship }
  dateObserved: { range: datetime }
  open: { range: boolean }
`;

const TYPE = "BikeHireDockingStation";
const SLOTS = importSlotsOf(MODEL, TYPE);
const TARGET = { type: TYPE, orgDomain: "hel.fi", space: "helsinki" };
const urn = (local: string) => `urn:ngsi-ld:${TYPE}:hel.fi:helsinki:${local}`;

describe("reading a file", () => {
  it("reads CSV with quotes, doubled quotes, a semicolon separator and a BOM", () => {
    const table = parseCsv('﻿name;note\r\n"Kamppi; centre";"said ""hi"""\r\nKallio;\r\n\r\n');
    expect(table.columns).toEqual(["name", "note"]);
    expect(table.rows).toEqual([
      { name: "Kamppi; centre", note: 'said "hi"' },
      { name: "Kallio", note: "" },
    ]);
  });

  it("reads JSON lists, an object holding one, and GeoJSON features with their geometry", () => {
    expect(parseJson('[{"name":"A","n":3,"tags":["x"]}]').rows).toEqual([{ name: "A", n: "3", tags: '["x"]' }]);
    expect(parseJson('{"stations":[{"name":"B"}]}').rows).toEqual([{ name: "B" }]);
    const geo = parseJson(
      '{"type":"FeatureCollection","features":[{"type":"Feature","properties":{"name":"C"},"geometry":{"type":"Point","coordinates":[24.9,60.1]}}]}',
    );
    expect(geo.rows[0]).toEqual({ name: "C", location: '{"type":"Point","coordinates":[24.9,60.1]}' });
    expect(() => parseJson("{nope")).toThrow("not JSON");
    expect(() => parseJson('{"a":1}')).toThrow("no list");
  });

  it("reads XML records from the root's repeated children, attributes included", () => {
    const table = parseXml('<stations><station id="s1"><name>Kamppi</name></station><station id="s2"><name>Kallio</name></station><meta/></stations>');
    expect(table.rows).toEqual([
      { id: "s1", name: "Kamppi" },
      { id: "s2", name: "Kallio" },
    ]);
    expect(() => parseXml("<a><b></a>")).toThrow("well-formed");
  });

  it("reads the first sheet of a workbook, shared and inline strings, a skipped cell and a flag", async () => {
    const workbook = zip({
      "xl/sharedStrings.xml": '<sst><si><t>name</t></si><si><t>bikes</t></si><si><t>Kamppi</t></si></sst>',
      "xl/worksheets/sheet1.xml":
        '<worksheet><sheetData><row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="s"><v>1</v></c><c r="C1" t="inlineStr"><is><t>open</t></is></c></row>' +
        '<row r="2"><c r="A2" t="s"><v>2</v></c><c r="C2" t="b"><v>1</v></c></row></sheetData></worksheet>',
    });
    const table = await parseXlsx(workbook);
    expect(table.columns).toEqual(["name", "bikes", "open"]);
    expect(table.rows).toEqual([{ name: "Kamppi", bikes: "", open: "true" }]);
  });

  it("takes a formula's saved value and never its text: nothing in a workbook runs (T-6445)", async () => {
    const book = zip({
      "xl/worksheets/sheet1.xml": `<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>total</t></is></c></row><row r="2"><c r="A2"><f>HYPERLINK("https://evil.example/","x")+SUM(A1:A1)</f><v>42</v></c></row></sheetData></worksheet>`,
      "xl/vbaProject.bin": "macro bytes the reader never opens",
    });
    expect((await parseXlsx(book)).rows).toEqual([{ total: "42" }]);
  });

  it("reads every sheet in the workbook's own order, named as the spreadsheet shows them (JC1-21)", async () => {
    const sheets = await parseWorkbook(
      workbook({
        Stations: [["name", "opened"], ["Kamppi", { date: 46086 }]],
        Stops: [["name", "lines"], ["Rautatientori", 12]],
      }),
    );
    expect(sheets.map((s) => s.name)).toEqual(["Stations", "Stops"]);
    expect(sheets[0]?.table.rows).toEqual([{ name: "Kamppi", opened: "2026-03-05" }]);
    expect(sheets[1]?.table.rows).toEqual([{ name: "Rautatientori", lines: "12" }]);
    expect((await parseXlsx(workbook({ A: [["x"], ["1"]], B: [["y"], ["2"]] }), "B")).rows).toEqual([{ y: "2" }]);
    await expect(parseXlsx(workbook({ A: [["x"], ["1"]] }), "Nope")).rejects.toThrow("no sheet 'Nope'");
  });

  it("orders sheet2 before sheet10 when the workbook lists no sheets", async () => {
    const sheet = (header: string) => `<worksheet><sheetData><row><c r="A1" t="inlineStr"><is><t>${header}</t></is></c></row></sheetData></worksheet>`;
    const sheets = await parseWorkbook(zip({ "xl/worksheets/sheet10.xml": sheet("ten"), "xl/worksheets/sheet2.xml": sheet("two") }));
    expect(sheets.map((s) => s.table.columns[0])).toEqual(["two", "ten"]);
  });

  it("writes a date cell as ISO 8601, a time with it, and tells date formats from number formats", () => {
    expect(serialDate(46086)).toBe("2026-03-05");
    expect(serialDate(46086.5)).toBe("2026-03-05T12:00:00");
    expect(serialDate(0.25)).toBe("06:00:00");
    expect(serialDate(44624, true)).toBe("2026-03-05");
    expect(isDateFormat("d.m.yyyy")).toBe(true);
    expect(isDateFormat("[$-405]h:mm")).toBe(true);
    expect(isDateFormat("#,##0.00")).toBe(false);
    expect(isDateFormat('"days" 0')).toBe(false);
  });

  it("writes a sheet as CSV that reads back as the same table", async () => {
    const table = (await parseWorkbook(workbook({ S: [["name", "note"], ["A, B", 'said "hi"']] })))[0]!.table;
    expect(csvOf(table)).toBe('name,note\r\n"A, B","said ""hi"""\r\n');
    expect(parseCsv(csvOf(table)).rows).toEqual(table.rows);
  });

  it("inflates a deflated sheet, as every workbook a spreadsheet saves holds it", async () => {
    const sheet = '<worksheet><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>name</t></is></c></row>' +
      '<row r="2"><c r="A2" t="inlineStr"><is><t>Töölö</t></is></c></row></sheetData></worksheet>';
    const table = await parseXlsx(zip({ "xl/worksheets/sheet1.xml": await deflated(sheet) }, { "xl/worksheets/sheet1.xml": sheet.length }));
    expect(table.rows).toEqual([{ name: "Töölö" }]);
    await expect(parseXlsx(new TextEncoder().encode("not a zip"))).rejects.toThrow("not an Excel workbook");
  });

  it("says a workbook whose directory points past its end is damaged (JC1-45)", async () => {
    const bad = zip({ "xl/worksheets/sheet1.xml": "<worksheet/>" });
    const view = new DataView(bad.buffer);
    const directory = view.getUint32(bad.length - 22 + 16, true);
    view.setUint32(directory + 42, bad.length - 4, true);
    await expect(parseXlsx(bad)).rejects.toThrow("the workbook's directory is damaged");
  });

  it("stops a sheet that unpacks past the file limit, a few kilobytes of ZIP or not (F-R2UI4-008)", async () => {
    const bomb = await deflated(" ".repeat(MAX_BYTES + 1));
    expect(bomb.length).toBeLessThan(MAX_BYTES / 100);
    await expect(parseXlsx(zip({ "xl/worksheets/sheet1.xml": bomb }, { "xl/worksheets/sheet1.xml": 0 }))).rejects.toThrow(
      "unpacks to more than 50 MB",
    );
  });

  it("refuses a file over the limit by its size, before a byte is read", async () => {
    const big = new File(["x"], "big.csv");
    Object.defineProperty(big, "size", { value: 53_000_000 });
    const error = await readTable(big).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(FileTooLarge);
    expect(unreadableText(i18n.getFixedT("en"), error)).toBe(
      "The file is 50.5 MB; an import reads at most 50 MB. Split it into smaller files, or remove the columns you do not need, and import each part.",
    );
  });

  it("refuses a file of another kind by name", async () => {
    await expect(readTable(new File(["x"], "rows.pdf"))).rejects.toThrow("CSV, Excel");
  });
});

describe("a Slovak file (T-3248)", () => {
  it("reads numbers with a decimal comma and a space or dot between thousands", () => {
    expect(numberOf("4,5")).toBe(4.5);
    expect(numberOf("1 234,5")).toBe(1234.5);
    expect(numberOf("1\u00a0234,5")).toBe(1234.5);
    expect(numberOf("1.234,5")).toBe(1234.5);
    expect(numberOf("1,234.5")).toBe(1234.5);
    expect(numberOf("-0,25")).toBe(-0.25);
    expect(numberOf("12")).toBe(12);
    expect(numberOf("")).toBeNaN();
    expect(numberOf("veľa")).toBeNaN();
  });

  it("reads a day.month.year date, with or without the time and the spaces", () => {
    expect(isoDateOf("7.10.2026")).toBe("2026-10-07");
    expect(isoDateOf("07. 10. 2026 8:05")).toBe("2026-10-07T08:05:00");
    expect(isoDateOf("7.10.2026 14:30:15")).toBe("2026-10-07T14:30:15");
    expect(isoDateOf("2026-10-07")).toBe("2026-10-07");
  });

  it("decodes Windows-1250 when the bytes are not UTF-8, and UTF-8 when they are", () => {
    // "Žilina" in Windows-1250: Ž is 0x8E.
    expect(decodeText(new Uint8Array([0x8e, 0x69, 0x6c, 0x69, 0x6e, 0x61]))).toEqual({ text: "Žilina", encoding: "windows-1250" });
    expect(decodeText(new TextEncoder().encode("Žilina"))).toEqual({ text: "Žilina", encoding: "utf-8" });
  });

  it("detects the decimal comma and the date format, and shows them", async () => {
    const table = parseCsv("názov;bicykle;videné\nNámestie SNP;1 204,5;7.10.2026 8:00\n");
    expect(detectedOf(table)).toEqual({ decimalComma: true, dateFormat: "d.m.yyyy" });
    const file = new File([new Uint8Array([0x6e, 0x3b, 0x62, 0x0a, 0x8e, 0x3b, 0x34, 0x2c, 0x35, 0x0a])], "s.csv");
    const read = await readTable(file);
    expect(read.rows).toEqual([{ n: "Ž", b: "4,5" }]);
    expect(read.detected).toEqual({ encoding: "windows-1250", separator: ";", decimalComma: true, dateFormat: undefined });
  });

  it("loads a Slovak row: decimal comma, day.month.year and a semicolon", () => {
    const table = parseCsv("id;názov;bicykle;videné\ns1;Námestie SNP;4;7.10.2026 8:00\n");
    const mapping = { id: "id", "názov": "name", bicykle: "availableBikeNumber", "videné": "dateObserved" };
    const { entities, rejected } = toEntities(table, mapping, SLOTS, TARGET);
    expect(rejected).toEqual([]);
    expect(entities[0].entity).toMatchObject({
      name: { value: "Námestie SNP" },
      availableBikeNumber: { value: 4 },
      dateObserved: { value: new Date("2026-10-07T08:00:00").toISOString() },
    });
  });
});

describe("mapping and checking", () => {
  it("reads the slots of the class with what the model requires", () => {
    expect(SLOTS.find((s) => s.name === "name")?.required).toBe(true);
    expect(SLOTS.find((s) => s.name === "status")?.values).toEqual(["working", "outOfService"]);
    expect(SLOTS.find((s) => s.name === "location")?.kind).toBe("GeoProperty");
  });

  it("suggests each slot once, by its name up to case and separators or a Smart Data Models synonym", () => {
    expect(suggestMapping(["ID", "Title", "available_bike_number", "Name", "Geometry", "colour"], SLOTS)).toEqual({
      ID: "id",
      Title: "name",
      available_bike_number: "availableBikeNumber",
      Name: "",
      Geometry: "location",
      colour: "",
    });
  });

  it("makes one entity per valid row, every value in its slot's shape", () => {
    const table = parseCsv(
      "id,name,bikes,status,where,street,seen,open\n" +
        "s1,Kamppi,4,working,\"24.93, 60.17\",urn:ngsi-ld:Street:hel.fi:helsinki:x,2026-10-06T08:00:00Z,yes\n",
    );
    const mapping = { id: "id", name: "name", bikes: "availableBikeNumber", status: "status", where: "location", street: "refStreet", seen: "dateObserved", open: "open" };
    const { entities, rejected } = toEntities(table, mapping, SLOTS, TARGET);
    expect(rejected).toEqual([]);
    expect(entities[0]).toEqual({
      row: 2,
      entity: {
        id: urn("s1"),
        type: TYPE,
        name: { type: "Property", value: "Kamppi" },
        availableBikeNumber: { type: "Property", value: 4 },
        status: { type: "Property", value: "working" },
        location: { type: "GeoProperty", value: { type: "Point", coordinates: [24.93, 60.17] } },
        refStreet: { type: "Relationship", object: "urn:ngsi-ld:Street:hel.fi:helsinki:x" },
        dateObserved: { type: "Property", value: "2026-10-06T08:00:00.000Z" },
        open: { type: "Property", value: true },
      },
    });
  });

  it("reads a Slovak yes and no, accents and all (T-6567)", () => {
    const table = parseCsv("id,name,open\ns1,Tulská,áno\ns2,Mládežnícka,Nie\n");
    const { entities, rejected } = toEntities(table, { id: "id", name: "name", open: "open" }, SLOTS, TARGET);
    expect(rejected).toEqual([]);
    expect(entities.map((one) => (one.entity.open as { value: unknown }).value)).toEqual([true, false]);
  });

  it("refuses a row for every reason at once, an id of another space and a repeated id, and mints the rest", () => {
    const table = parseCsv(
      "id,name,bikes,status\n" +
        ",,-1,broken\n" +
        "urn:ngsi-ld:Street:hel.fi:helsinki:s9,Tapiola,1,working\n" +
        "s2,Kallio,2.5,working\n" +
        "s3,Töölö,0,working\n" +
        "s3,Again,0,working\n" +
        "bad id,X,0,working\n",
    );
    const mapping = { id: "id", name: "name", bikes: "availableBikeNumber", status: "status" };
    const { entities, rejected } = toEntities(table, mapping, SLOTS, TARGET, () => "fresh");
    expect(entities.map((e) => e.entity.id)).toEqual([urn("s3")]);
    expect(rejected).toEqual([
      { row: 2, id: urn("fresh"), reason: "availableBikeNumber: -1 is below 0; status: `broken` is not one of working, outOfService; name is required" },
      { row: 3, reason: "`urn:ngsi-ld:Street:hel.fi:helsinki:s9` names another type than BikeHireDockingStation" },
      { row: 4, id: urn("s2"), reason: "availableBikeNumber: `2.5` is not a whole number" },
      { row: 6, id: urn("s3"), reason: "the file names this id twice" },
      { row: 7, reason: "`bad id` cannot be the last part of an id" },
    ]);
  });

  it("takes any URN of the type as it is: no segment of it says which space an entity is in (ADR-N-041, F-R2UI4-004)", () => {
    const table = parseCsv(
      "id,name,bikes,status\n" +
        "urn:ngsi-ld:BikeHireDockingStation:abc,Kamppi,1,working\n" +
        "urn:ngsi-ld:BikeHireDockingStation:hel.fi:espoo:s9,Tapiola,1,working\n" +
        "urn:ngsi-ld:BikeHireDockingStation:,Empty,1,working\n",
    );
    const mapping = { id: "id", name: "name", bikes: "availableBikeNumber", status: "status" };
    const { entities, rejected } = toEntities(table, mapping, SLOTS, TARGET, () => "fresh");
    expect(entities.map((e) => e.entity.id)).toEqual([
      "urn:ngsi-ld:BikeHireDockingStation:abc",
      "urn:ngsi-ld:BikeHireDockingStation:hel.fi:espoo:s9",
    ]);
    expect(rejected).toEqual([{ row: 4, reason: "`urn:ngsi-ld:BikeHireDockingStation:` is not an NGSI-LD URN" }]);
  });

  it("writes the refused rows as CSV a spreadsheet opens", () => {
    expect(rejectedCsv([{ row: 3, id: "u", reason: 'a, "b"' }])).toBe('row,id,reason\r\n3,u,"a, ""b"""');
  });
});

describe("creating through the gateway", () => {
  const entities = Array.from({ length: BATCH + 2 }, (_, at) => ({ row: at + 2, entity: { id: urn(`s${at}`), type: TYPE } }));

  it("creates in batches with the person's transport and counts a 201 whole", async () => {
    const send = vi.fn<Send>(async () => ({ status: 201 }));
    const progress: number[] = [];
    const answer = await createInBatches(send, "helsinki", entities, (sent) => progress.push(sent));
    expect(answer).toEqual({ created: BATCH + 2, rejected: [] });
    expect(send).toHaveBeenCalledTimes(2);
    // DM-113: an import through the Portal names why it writes.
    expect(send.mock.calls[0][0]).toMatchObject({ method: "POST", path: "/cs/helsinki/ngsi-ld/v1/entityOperations/create", reason: "imported from a file" });
    expect((send.mock.calls[0][0].body as unknown[]).length).toBe(BATCH);
    expect(progress).toEqual([BATCH, BATCH + 2]);
  });

  it("reads a 207 by entity and refuses a batch the gateway refuses whole, in its words", async () => {
    const send = vi
      .fn<Send>()
      .mockResolvedValueOnce({
        status: 207,
        body: { success: [], errors: [{ entityId: urn("s1"), error: { type: "AlreadyExists", title: "Already exists" } }] },
      })
      .mockResolvedValueOnce({ status: 403, body: { detail: "you may not createEntity here" } });
    const answer = await createInBatches(send, "helsinki", entities, () => {});
    expect(answer.created).toBe(BATCH - 1);
    expect(answer.rejected[0]).toEqual({ row: 3, id: urn("s1"), reason: "Already exists" });
    expect(answer.rejected.slice(1).map((r) => r.reason)).toEqual(["you may not createEntity here", "you may not createEntity here"]);
  });

  it("stops at the first batch the person may not write and names the refusal (T-4867)", async () => {
    const send = vi.fn<Send>(async () => ({ status: 403, body: { detail: "no policy grants this operation" } }));
    const progress: number[] = [];
    const answer = await createInBatches(send, "helsinki", entities, (sent) => progress.push(sent));
    expect(send).toHaveBeenCalledTimes(1);
    expect(answer.created).toBe(0);
    expect(answer.denied).toBe("no policy grants this operation");
    expect(answer.rejected).toHaveLength(BATCH + 2);
    expect(progress).toEqual([BATCH + 2]);
  });

  it("refuses the batch when the request itself fails", async () => {
    const send = vi.fn<Send>(async () => {
      throw new Error("offline");
    });
    const answer = await createInBatches(send, "helsinki", entities.slice(0, 1));
    expect(answer).toEqual({ created: 0, rejected: [{ row: 2, id: urn("s0"), reason: "offline" }] });
  });
});

describe("export", () => {
  it("narrows an Endpoint's file to the type, the filter and the fields", () => {
    expect(exportUrl("abc", "csv", TYPE, 'name=="A"', ["name", "status"])).toBe(
      `/api/endpoint/abc/file.csv?type=${TYPE}&q=name%3D%3D%22A%22&attrs=name%2Cstatus`,
    );
    expect(exportUrl("abc", "csv", TYPE, undefined, [])).toBe(`/api/endpoint/abc/file.csv?type=${TYPE}`);
    // Checked rows go as their ids, and the view's filter does not narrow them further (T-5186).
    expect(exportUrl("abc", "csv", TYPE, 'name=="A"', ["name"], ["urn:ngsi-ld:X:1", "urn:ngsi-ld:X:2"])).toBe(
      `/api/endpoint/abc/file.csv?type=${TYPE}&id=urn%3Angsi-ld%3AX%3A1%2Curn%3Angsi-ld%3AX%3A2&attrs=name`,
    );
    // The grid's ID filter narrows the file like the grid (T-6107); checked ids replace it.
    expect(exportUrl("abc", "csv", TYPE, undefined, [], [], "^urn:ngsi-ld:X:1$")).toBe(
      `/api/endpoint/abc/file.csv?type=${TYPE}&idPattern=%5Eurn%3Angsi-ld%3AX%3A1%24`,
    );
    expect(exportUrl("abc", "csv", TYPE, undefined, [], ["urn:ngsi-ld:X:1"], "X")).not.toContain("idPattern");
  });

  it("offers only the formats an Endpoint of the space serves, and says when none does", () => {
    const endpoint = (name: string, slug: string, reps: string[]) =>
      ({ apiVersion: "x", kind: "Endpoint", metadata: { name }, spec: { slug, enabledRepresentations: reps } }) as unknown as Manifest;
    const { rerender } = render(
      <I18nextProvider i18n={i18n}>
        <ExportLinks endpoints={[endpoint("map", "m1", ["geojson"]), endpoint("all", "a1", ["ngsi-ld", "csv", "json"])]} type={TYPE} q={undefined} attrs={["name"]} />
      </I18nextProvider>,
    );
    expect(screen.getByRole("link", { name: "CSV" })).toHaveAttribute("href", `/api/endpoint/a1/file.csv?type=${TYPE}&attrs=name`);
    // CSV is the one file an Endpoint publishes (T-4600): a listed `json` is no link.
    expect(screen.queryByRole("link", { name: "JSON" })).toBeNull();
    expect(screen.queryByRole("link", { name: "Excel" })).toBeNull();
    rerender(
      <I18nextProvider i18n={i18n}>
        <ExportLinks endpoints={[endpoint("map", "m1", ["geojson"])]} type={TYPE} q={undefined} attrs={[]} />
      </I18nextProvider>,
    );
    expect(screen.getByText(en.spaces.export.none)).toBeInTheDocument();
  });
});

/** helsinki.linkml.yaml's shape: classes specialise ngsi-ld-core's Entity and list no geometry of their own. */
const CORE_MODEL = `id: https://hel.fi/models/helsinki
name: helsinki
imports:
  - linkml:types
  - ngsi-ld-core
classes:
  Place:
    is_a: Entity
    slots: [name, address]
    slot_usage:
      address: { required: true }
  PointOfInterest:
    is_a: Place
    slots: [serviceCategory, address]
slots:
  name: { range: string }
  address: { range: string }
  serviceCategory: { range: string, required: true }
`;

describe("slots a class inherits (T-4867)", () => {
  it("gives a class its is_a parents' slots and the NGSI-LD location of the core Entity", () => {
    const slots = importSlotsOf(CORE_MODEL, "PointOfInterest");
    expect(slots.map((slot) => slot.name)).toEqual(["name", "address", "serviceCategory", "location"]);
    // The child's own listing wins the slot both name: `address` is optional on PointOfInterest.
    expect(slots.find((slot) => slot.name === "address")?.required).toBe(false);
    expect(slots.find((slot) => slot.name === "location")).toEqual({ name: "location", kind: "GeoProperty", required: false });
  });

  it("adds no location to a class that does not reach the core Entity", () => {
    expect(importSlotsOf(CORE_MODEL.replace("is_a: Entity", ""), "PointOfInterest").map((slot) => slot.name)).not.toContain("location");
    expect(SLOTS.filter((slot) => slot.name === "location")).toHaveLength(1);
  });

  it("reports a refused value once, not again as a missing required field", () => {
    const slots = importSlotsOf(CORE_MODEL.replace("serviceCategory: { range: string, required: true }", "serviceCategory: { range: Category, required: true }\nenums:\n  Category:\n    permissible_values:\n      library: {}"), "PointOfInterest");
    const table = parseCsv("name,serviceCategory\nOodi,swimming hall\n");
    const { rejected } = toEntities(table, suggestMapping(table.columns, slots), slots, { type: "PointOfInterest", orgDomain: "hel.fi", space: "helsinki" }, () => "x");
    expect(rejected[0].reason).toMatch(/is not one of library/);
    expect(rejected[0].reason).not.toMatch(/is required/);
  });

  it("imports a GeoJSON point into the inherited location", () => {
    const slots = importSlotsOf(CORE_MODEL, "PointOfInterest");
    const table = parseJson(
      '{"type":"FeatureCollection","features":[{"type":"Feature","properties":{"name":"Oodi","serviceCategory":"library"},"geometry":{"type":"Point","coordinates":[24.938,60.174]}}]}',
    );
    const mapping = suggestMapping(table.columns, slots);
    expect(mapping.location).toBe("location");
    const { entities, rejected } = toEntities(table, mapping, slots, { type: "PointOfInterest", orgDomain: "hel.fi", space: "helsinki" }, () => "oodi");
    expect(rejected).toEqual([]);
    expect(entities[0].entity.location).toEqual({ type: "GeoProperty", value: { type: "Point", coordinates: [24.938, 60.174] } });
  });
});

describe("the import dialog", () => {
  it("maps a file's columns, shows what is refused, creates the rest and reports both", async () => {
    const user = userEvent.setup();
    const send = vi.fn<Send>(async () => ({ status: 201 }));
    const onImported = vi.fn();
    render(
      <I18nextProvider i18n={i18n}>
        <ImportRowsDialog open onOpenChange={() => {}} target={TARGET} slots={SLOTS as ImportSlot[]} send={send} onImported={onImported} />
      </I18nextProvider>,
    );
    const file = new File(["Title,bikes\nKamppi,4\n,2\n"], "stations.csv", { type: "text/csv" });
    await user.upload(screen.getByLabelText("Choose a file"), file);

    // `Title` is suggested as `name`; `bikes` matches no slot by name and is mapped by hand.
    expect(await screen.findByRole("combobox", { name: "Title" })).toHaveValue("name");
    await user.selectOptions(screen.getByRole("combobox", { name: "bikes" }), "availableBikeNumber");
    expect(screen.getByText("1 row is ready, 1 refused.")).toBeInTheDocument();
    expect(within(screen.getByRole("list", { name: "Rows that will not be created" })).getByText(/name is required/)).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Create 1 row" }));
    await waitFor(() => expect(screen.getByText("1 row created, 1 refused.")).toBeInTheDocument());
    expect(send).toHaveBeenCalledTimes(1);
    const [created] = send.mock.calls[0][0].body as Record<string, unknown>[];
    expect(created).toMatchObject({ type: TYPE, name: { value: "Kamppi" }, availableBikeNumber: { value: 4 } });
    expect(String(created.id)).toMatch(/^urn:ngsi-ld:BikeHireDockingStation:hel\.fi:helsinki:/);
    expect(onImported).toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Download the refused rows (CSV)" })).toBeInTheDocument();
  });

  it("writes each row's expiresAt from the column picked and names the space's history rule (T-4822)", async () => {
    await i18n.changeLanguage("en");
    const user = userEvent.setup();
    const send = vi.fn<Send>(async () => ({ status: 201 }));
    render(
      <I18nextProvider i18n={i18n}>
        <ImportRowsDialog
          open
          onOpenChange={() => {}}
          target={TARGET}
          slots={SLOTS}
          send={send}
          onImported={() => {}}
          retention={[{ entityType: "*", history: "P90D" }]}
        />
      </I18nextProvider>,
    );
    await user.upload(
      screen.getByLabelText("Choose a file"),
      new File(["Title,ends\nKamppi,2999-01-01T00:00:00Z\nTöölö,2001-01-01T00:00:00Z\n"], "stations.csv"),
    );
    const expiry = await screen.findByTestId("import-expiry");
    expect(within(expiry).getByRole("radio", { name: "Never" })).toBeChecked();
    expect(within(expiry).getByText(/The space default keeps the history of BikeHireDockingStation: 90 days/)).toBeInTheDocument();
    await user.click(within(expiry).getByRole("radio", { name: "At the time a column gives" }));
    await user.selectOptions(within(expiry).getByRole("combobox", { name: "Column with the time" }), "ends");
    expect(screen.getByRole("combobox", { name: "ends" })).toHaveValue("@expiresAt");
    expect(screen.getByText("1 row is ready, 1 refused.")).toBeInTheDocument();
    expect(screen.getByText(/is already past/)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Create 1 row" }));
    await waitFor(() => expect(send).toHaveBeenCalledTimes(1));
    const [created] = send.mock.calls[0][0].body as Record<string, unknown>[];
    expect(created.expiresAt).toBe("2999-01-01T00:00:00.000Z");
  });

  it("says a type no rule names keeps no history (T-5058, DM-132)", async () => {
    await i18n.changeLanguage("en");
    const user = userEvent.setup();
    render(
      <I18nextProvider i18n={i18n}>
        <ImportRowsDialog open onOpenChange={() => {}} target={TARGET} slots={SLOTS} send={vi.fn<Send>()} onImported={() => {}} retention={[]} />
      </I18nextProvider>,
    );
    await user.upload(screen.getByLabelText("Choose a file"), new File(["Title\nKamppi\n"], "stations.csv"));
    const expiry = await screen.findByTestId("import-expiry");
    expect(within(expiry).getByText(/No rule of this space names BikeHireDockingStation, so it keeps no history/)).toBeInTheDocument();
  });

  it("says in one sentence that nothing was written when the space refuses the person (T-4867)", async () => {
    await i18n.changeLanguage("en");
    const user = userEvent.setup();
    const send = vi.fn<Send>(async () => ({ status: 403, body: { detail: "no policy grants this operation" } }));
    render(
      <I18nextProvider i18n={i18n}>
        <ImportRowsDialog open onOpenChange={() => {}} target={TARGET} slots={SLOTS} send={send} onImported={() => {}} />
      </I18nextProvider>,
    );
    await user.upload(screen.getByLabelText("Choose a file"), new File(["Title\nKamppi\nKallio\n"], "stations.csv"));
    await user.click(await screen.findByRole("button", { name: "Create 2 rows" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Nothing was written: the space helsinki does not let your role create BikeHireDockingStation entities (no policy grants this operation). Ask a project owner",
    );
    expect(screen.queryByRole("list", { name: "Rows that will not be created" })).not.toBeInTheDocument();
  });

  it("names the file's size and the limit when it is too large (T-4867)", async () => {
    await i18n.changeLanguage("en");
    const user = userEvent.setup();
    render(
      <I18nextProvider i18n={i18n}>
        <ImportRowsDialog open onOpenChange={() => {}} target={TARGET} slots={SLOTS} send={vi.fn<Send>()} onImported={() => {}} />
      </I18nextProvider>,
    );
    const big = new File(["x"], "big.csv");
    Object.defineProperty(big, "size", { value: 61 * 1024 * 1024 });
    await user.upload(screen.getByLabelText("Choose a file"), big);
    expect(await screen.findByText(/The file is 61\.0 MB; an import reads at most 50 MB\./)).toBeInTheDocument();
  });

  it("shows how the file was read and previews its first rows (T-3248)", async () => {
    const user = userEvent.setup();
    render(
      <I18nextProvider i18n={i18n}>
        <ImportRowsDialog open onOpenChange={() => {}} target={TARGET} slots={SLOTS} send={vi.fn<Send>()} onImported={() => {}} />
      </I18nextProvider>,
    );
    await user.upload(screen.getByLabelText("Choose a file"), new File(["Title;bikes\nKamppi;4,5\n"], "stations.csv"));
    expect(await screen.findByTestId("import-detected")).toHaveTextContent(
      "Encoding UTF-8 · separated by semicolons · decimal comma",
    );
    await user.click(screen.getByText("Preview the first 1 row"));
    const preview = screen.getByRole("table", { name: "The file's first rows as read" });
    expect(within(preview).getByText("Kamppi")).toBeInTheDocument();
  });

  it("says why a file cannot be read and stays on the first step", async () => {
    const user = userEvent.setup({ applyAccept: false });
    render(
      <I18nextProvider i18n={i18n}>
        <ImportRowsDialog open onOpenChange={() => {}} target={TARGET} slots={SLOTS} send={vi.fn<Send>()} onImported={() => {}} />
      </I18nextProvider>,
    );
    await user.upload(screen.getByLabelText("Choose a file"), new File(["{nope"], "rows.json"));
    expect(await screen.findByRole("alert")).toHaveTextContent("The file could not be read: the file is not JSON");
    expect(screen.getByLabelText("Choose a file")).toBeInTheDocument();
  });
});

describe("cellValue", () => {
  const cell = (xml: string) => new DOMParser().parseFromString(xml, "application/xml").documentElement;

  it("reads each cell type as its text", () => {
    expect(cellValue(cell('<c t="s"><v>1</v></c>'), ["a", "b"], [], false)).toBe("b");
    expect(cellValue(cell('<c t="s"><v>9</v></c>'), ["a"], [], false)).toBe("");
    expect(cellValue(cell('<c t="inlineStr"><is><t>x</t><t>y</t></is></c>'), [], [], false)).toBe("xy");
    expect(cellValue(cell('<c t="b"><v>1</v></c>'), [], [], false)).toBe("true");
    expect(cellValue(cell('<c t="b"><v>0</v></c>'), [], [], false)).toBe("false");
    expect(cellValue(cell("<c><v>42.5</v></c>"), [], [false], false)).toBe("42.5");
  });

  it("reads a dated style's serial as its ISO date, in either date system", () => {
    expect(cellValue(cell('<c s="1"><v>46086</v></c>'), [], [false, true], false)).toBe("2026-03-05");
    expect(cellValue(cell('<c s="1"><v>44624</v></c>'), [], [false, true], true)).toBe("2026-03-05");
    expect(cellValue(cell('<c t="n" s="1"><v></v></c>'), [], [false, true], false)).toBe("");
  });
});
