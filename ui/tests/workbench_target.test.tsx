/**
 * T-4842: the workbench's Target card names each attribute of the type the pipeline writes with
 * the server's write judgement (a lock and the reason when the Endpoint grants it not), and the
 * Mapped output shows a record's attributes as words, never raw JSON.
 */
import { render, screen, within } from "@testing-library/react";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import { RecordTable, TargetCard, grantedNames, readWritable, valueText } from "../src/pages/pipelines/WorkbenchTarget";
import type { TargetType } from "../src/pages/pipelines/WorkbenchTarget";

const TARGET: TargetType = {
  model: { name: "helsinki", version: "v1" },
  attributes: [
    { name: "location", kind: "GeoProperty", valueType: "object", required: false },
    { name: "speed", kind: "Property", valueType: "number", required: false, unit: { code: "MTS" }, minimum: 0 },
    { name: "transportMode", kind: "Property", valueType: "string", required: true, values: ["bus", "tram"] },
    { name: "nextStop", kind: "Relationship", required: false, relationship: { target: "Stop", many: false } },
  ],
  writable: [
    { name: "location", writable: true, reason: "only inside the area its grant names" },
    { name: "speed", writable: true },
    { name: "transportMode", writable: true },
    { name: "nextStop", writable: false, reason: "AttributeOutsideGrant: not writable through helsinki-all" },
  ],
  loading: false,
};

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

const inI18n = (node: React.ReactNode) => render(<I18nextProvider i18n={i18n}>{node}</I18nextProvider>);

describe("the Target card", () => {
  it("shows the space, the model version, the type and each attribute's writability with the reason", () => {
    inI18n(<TargetCard space="helsinki" type="Vehicle" target={TARGET} />);
    const card = screen.getByTestId("workbench-target-card");
    expect(card).toHaveTextContent("helsinki v1");
    expect(card).toHaveTextContent("Vehicle");
    const table = within(card).getByRole("table", { name: "Attributes of Vehicle" });
    const row = (name: string) => within(table).getAllByRole("row").find((r) => within(r).queryByText(name)) as HTMLElement;
    expect(row("speed")).toHaveTextContent("number ≥ 0, MTS");
    expect(row("speed")).toHaveTextContent("writable");
    expect(row("transportMode")).toHaveTextContent("bus · tram");
    expect(row("transportMode")).toHaveTextContent("required");
    expect(row("nextStop")).toHaveTextContent("→ Stop");
    expect(row("nextStop")).toHaveTextContent("not writable");
    expect(row("nextStop")).toHaveTextContent("not writable through helsinki-all");
    expect(row("location")).toHaveTextContent("only inside the area its grant names");
    expect(row("expiresAt")).toHaveTextContent("set in Lifetime");
  });

  it("lists for a lifetime only the attributes the Endpoint grants", () => {
    expect(grantedNames(TARGET)).toEqual(["location", "speed", "transportMode"]);
    expect(grantedNames({ ...TARGET, writable: undefined })).toEqual(["location", "speed", "transportMode", "nextStop"]);
  });

  it("reads the server's writable list and drops what is no attribute", () => {
    expect(readWritable({ writable: [{ name: "speed", writable: true }, { name: "x", writable: "yes" }, { nope: 1 }, null] })).toEqual([
      { name: "speed", writable: true },
      { name: "x", writable: false },
    ]);
    expect(readWritable({})).toEqual([]);
  });
});

describe("a mapped record", () => {
  it("writes each attribute's value in words", () => {
    expect(valueText({ type: "Property", value: 8.4, unitCode: "MTS" })).toBe("8.4 MTS");
    expect(valueText({ type: "Property", value: "+42" })).toBe('"+42"');
    expect(valueText({ type: "GeoProperty", value: { type: "Point", coordinates: [24.9384, 60.1699] } })).toBe("Point 24.9384, 60.1699");
    expect(valueText({ type: "Relationship", object: "urn:ngsi-ld:Stop:1" })).toBe("urn:ngsi-ld:Stop:1");
    expect(valueText({ type: "LanguageProperty", languageMap: { fi: "Rautatientori", sv: "Järnvägstorget" } })).toBe("fi: Rautatientori · sv: Järnvägstorget");
    expect(valueText("2026-10-20T17:46:07Z")).toBe('"2026-10-20T17:46:07Z"');
  });

  it("puts each finding at its field and the ends in the Expires column", () => {
    inI18n(
      <RecordTable
        index={0}
        record={{ id: "urn:ngsi-ld:Vehicle:1012", type: "Vehicle", delay: { type: "Property", value: "+42", observedAt: "2026-10-20T17:41:07Z" }, speed: { type: "Property", value: 8.4 } }}
        verdict={{
          ok: false,
          findings: [
            { source: "model", rule: "sh:datatype", field: "delay", reason: "The model wants a whole number" },
            { source: "endpoint", rule: "policy", field: "", reason: "no Policy grants this pipeline a batch upsert" },
          ],
          expires: { entity: "2026-10-20T17:46:07Z", attributes: { speed: "2026-10-20T17:43:07Z" } },
        }}
      />,
    );
    const record = screen.getByTestId("workbench-record-0");
    expect(within(record).getByText("2 problems")).toBeInTheDocument();
    const rows = within(record).getAllByRole("row");
    const row = (name: string) => rows.find((r) => within(r).queryByText(name)) as HTMLElement;
    expect(row("delay")).toHaveTextContent("sh:datatype");
    expect(row("delay")).toHaveTextContent("observed 2026-10-20T17:41:07Z");
    expect(row("speed")).toHaveTextContent("passes");
    expect(row("speed")).toHaveTextContent("2026-10-20T17:43:07Z");
    expect(row("expiresAt")).toHaveTextContent("2026-10-20T17:46:07Z");
    // A finding of no field stands above the table.
    expect(within(record).getByText("no Policy grants this pipeline a batch upsert")).toBeInTheDocument();
  });
});
