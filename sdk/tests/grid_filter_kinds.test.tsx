import { describe, expect, it } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { EntityGrid } from "../src/grid/EntityGrid";
import { parseGridConfig } from "../src/grid/config";
import { fixtureSource } from "../src/grid/source";
import type { EntitySource, GridQuery } from "../src/grid/source";
import { queryFromFilters, typeOfFilter } from "../src/grid/filters";

// T-6328 (owner): the filter row under the headers, one typed control per field kind, the lead
// columns (ID, Type, Name) included; every value goes to the endpoint as data.
const kpi = (n: number, extra: Record<string, unknown>) => ({
  id: `urn:ngsi-ld:KeyPerformanceIndicator:bratislava.kpi.${n}`,
  type: "KeyPerformanceIndicator",
  name: { type: "LanguageProperty", languageMap: { sk: `Ukazovateľ ${n}`, en: `Indicator ${n}` } },
  kpiValue: { type: "Property", value: n * 10 },
  category: { type: "Property", value: n % 2 ? "traffic" : "health" },
  dateObserved: { type: "Property", value: { "@type": "DateTime", "@value": `2026-10-0${n}T08:30:00Z` } },
  issueDate: { type: "Property", value: `2026-10-0${n}` },
  verified: { type: "Property", value: n % 2 === 1 },
  ...extra,
});
const rows = [kpi(1, {}), kpi(2, {}), kpi(3, {})];
const config = (type = "KeyPerformanceIndicator") =>
  parseGridConfig({
    source: { kind: "fixture", name: "t" },
    type,
    columns: [
      { attr: "name", label: "Name" },
      { attr: "kpiValue", label: "Value" },
      { attr: "category", label: "Category" },
      { attr: "dateObserved", label: "Observed" },
      { attr: "issueDate", label: "Issued" },
      { attr: "verified", label: "Verified" },
    ],
  }).config!;
const enums = { category: [{ value: "traffic", title: "Traffic" }, { value: "health", title: "Health" }] };

function recording() {
  const asked: GridQuery[] = [];
  const inner = fixtureSource(rows);
  const source: EntitySource = { query: async (q, page) => (asked.push(q), inner.query(q, page)), get: inner.get };
  return { asked, source };
}

async function grid(type?: string) {
  const { asked, source } = recording();
  render(<EntityGrid config={config(type)} source={source} enums={enums} />);
  // The fixture reads one type at a time, so a two-type grid shows no rows: its filter row still stands.
  if (type?.includes(",")) await screen.findByRole("combobox", { name: "Filter: Type" });
  else await screen.findAllByText(/Indicator 1|Ukazovateľ 1/);
  return asked;
}
const op = (label: string, value: string) => fireEvent.change(screen.getByRole("combobox", { name: `Filter: ${label}` }), { target: { value } });

describe("the filter row, by field kind (T-6328)", () => {
  it("string: contains with an example placeholder, a LanguageProperty asked in any language", async () => {
    const asked = await grid();
    op("Name", "contains");
    const box = screen.getByRole("textbox", { name: "Value: Name" });
    expect(box).toHaveAttribute("placeholder", "Bratislava");
    fireEvent.change(box, { target: { value: "Ukaz" } });
    await waitFor(() => expect(asked.at(-1)?.q).toBe('name[*]~="Ukaz"'));
  });

  it("enum: picks from its values, asked as a quoted list", async () => {
    const asked = await grid();
    op("Category", "anyOf");
    const pick = screen.getByRole("combobox", { name: "Value: Category" }) as HTMLSelectElement;
    expect([...pick.options].map((o) => o.textContent)).toEqual(["Any", "Traffic", "Health"]);
    fireEvent.change(pick, { target: { value: "traffic" } });
    await waitFor(() => expect(asked.at(-1)?.q).toBe('category=="traffic"'));
  });

  it("number: a range in words, sent as two bare numbers", async () => {
    const asked = await grid();
    const select = screen.getByRole("combobox", { name: "Filter: Value" }) as HTMLSelectElement;
    expect([...select.options].map((o) => o.textContent)).toContain("is greater than");
    op("Value", "between");
    expect(screen.getByRole("spinbutton", { name: "Value: Value" })).toHaveAttribute("placeholder", "1500");
    fireEvent.change(screen.getByRole("spinbutton", { name: "Value: Value" }), { target: { value: "10" } });
    fireEvent.change(screen.getByRole("spinbutton", { name: "Upper value: Value" }), { target: { value: "20" } });
    await waitFor(() => expect(asked.at(-1)?.q).toBe("kpiValue>=10;kpiValue<=20"));
  });

  it("date: a date picker", async () => {
    const asked = await grid();
    op("Issued", "gte");
    const box = screen.getByLabelText("Value: Issued");
    expect(box).toHaveAttribute("type", "date");
    fireEvent.change(box, { target: { value: "2026-10-02" } });
    await waitFor(() => expect(asked.at(-1)?.q).toBe("issueDate>=2026-10-02"));
  });

  it("datetime: a date and time picker, sent as the UTC instant", async () => {
    const asked = await grid();
    op("Observed", "lt");
    const box = screen.getByLabelText("Value: Observed") as HTMLInputElement;
    expect(box.type).toBe("datetime-local");
    fireEvent.change(box, { target: { value: "2026-10-02T10:00" } });
    const sent = new Date("2026-10-02T10:00").toISOString();
    await waitFor(() => expect(asked.at(-1)?.q).toBe(`dateObserved<${sent}`));
    expect(box.value).toBe("2026-10-02T10:00");
  });

  it("boolean: yes or no, sent bare", async () => {
    const asked = await grid();
    op("Verified", "equals");
    const pick = screen.getByRole("combobox", { name: "Value: Verified" });
    fireEvent.change(pick, { target: { value: "true" } });
    await waitFor(() => expect(asked.at(-1)?.q).toBe("verified==true"));
  });

  it("ID: contains or starts with, the text escaped into idPattern", async () => {
    const asked = await grid();
    op("ID", "startsWith");
    fireEvent.change(screen.getByRole("textbox", { name: "Value: ID" }), { target: { value: "bratislava.kpi[1" } });
    await waitFor(() => expect(asked.at(-1)?.idPattern).toBe("^(urn:ngsi-ld:[^:]+:)?bratislava\\.kpi\\[1"));
    expect(asked.at(-1)?.q).toBeUndefined();
    op("ID", "pattern");
    await waitFor(() => expect(asked.at(-1)?.idPattern).toBe("bratislava\\.kpi\\[1"));
  });

  it("Type: offered where the grid reads several types, the picked ones narrow its type list", async () => {
    const asked = await grid("KeyPerformanceIndicator,Alert");
    op("Type", "anyOf");
    const pick = screen.getByRole("combobox", { name: "Value: Type" }) as HTMLSelectElement;
    expect([...pick.options].map((o) => o.value)).toEqual(["", "KeyPerformanceIndicator", "Alert"]);
    fireEvent.change(pick, { target: { value: "Alert" } });
    await waitFor(() => expect(asked.at(-1)?.type).toBe("Alert"));
  });

  it("Type: a name the grid does not read never widens it, and one type offers no filter", async () => {
    expect(typeOfFilter("A,B", { op: "anyOf", value: "", values: ["C"] })).toBe("A,B");
    expect(typeOfFilter("A,B", { op: "anyOf", value: "", values: ["B", "C"] })).toBe("B");
    await grid();
    expect(screen.queryByRole("combobox", { name: "Filter: Type" })).toBeNull();
  });

  it("an equals on the id is one id, anchored at both ends", () => {
    expect(queryFromFilters([{ key: "id", attr: null, kind: "id" }], { id: { op: "equals", value: "a.b" } }).idPattern).toBe("^a\\.b$");
  });
});
