// T-7706: importRows.ts edges: a model slot named id or type, a date cell that is no date, a workbook whose
// directory entry has no signature, and a workbook that lists a sheet its archive does not hold.
import { describe, expect, it } from "vitest";
import { importSlotsOf, parseCsv, parseXlsx, toEntities } from "../src/pages/spaces/importRows";
import { zip } from "./xlsxFixture";

const MODEL = `id: https://hel.fi/models/stations
name: stations
default_prefix: hel
prefixes:
  hel: https://hel.fi/models/
classes:
  Station:
    slots: [id, type, name, seen]
slots:
  id: { range: string }
  type: { range: string }
  name: { range: string }
  seen: { range: datetime }
`;

describe("importSlotsOf", () => {
  it("leaves out the model's own id and type slots: the entity's identity is not mapped from a column", () => {
    const names = importSlotsOf(MODEL, "Station").map((slot) => slot.name);
    expect(names).toContain("name");
    expect(names).not.toContain("type");
    expect(names.filter((n) => n === "id")).toHaveLength(0);
  });
});

describe("toEntities with a date column", () => {
  it("refuses a row whose date cell is no date and keeps the one that is", () => {
    const slots = importSlotsOf(MODEL, "Station");
    const table = parseCsv("id,seen\ns1,not a date\ns2,2026-10-06T08:00:00Z\n");
    const { entities, rejected } = toEntities(table, { id: "id", seen: "seen" }, slots, { type: "Station", orgDomain: "hel.fi", space: "helsinki" });
    expect(rejected).toHaveLength(1);
    expect(rejected[0].reason).toContain("is not a date");
    expect(entities).toHaveLength(1);
  });
});

describe("parseXlsx damaged workbooks", () => {
  it("says the directory is damaged when an entry has no signature", async () => {
    const bytes = zip({ "xl/worksheets/sheet1.xml": "<worksheet/>" });
    const view = new DataView(bytes.buffer);
    const directory = view.getUint32(bytes.length - 22 + 16, true);
    view.setUint32(directory, 0, true);
    await expect(parseXlsx(bytes)).rejects.toThrow("the workbook's directory is damaged");
  });

  it("skips a sheet the workbook lists but the archive does not hold", async () => {
    const book = '<workbook xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Ghost" r:id="rId9"/></sheets></workbook>';
    await expect(parseXlsx(zip({ "xl/workbook.xml": book }))).rejects.toThrow("the workbook has no sheet");
  });
});
