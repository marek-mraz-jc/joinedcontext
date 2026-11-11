// covers (T-2137, the module gate in gate_modules.test.ts): src/components/grid/RecordsGrid.tsx, src/components/grid/LazyAdminGrid.tsx.
/**
 * T-6752: records a page already holds (a pipeline sample, an import preview, an answer's rows)
 * and a public page's rows on the shared grid. The rows are what the read returned; the grid
 * sorts, searches, hides a column and says when nothing matches.
 */
import { fireEvent, render, screen, within } from "@testing-library/react";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { RecordsGrid } from "../src/components/grid/RecordsGrid";
import { LazyAdminGrid } from "../src/components/grid/LazyAdminGrid";
import { EntityWriteCard } from "../src/pages/apps/EntityWriteCard";

const RECORDS = [
  { station: "Kamppi", pm10: "18" },
  { station: "Töölö", pm10: "9" },
  { station: "Kallio", pm10: "31" },
];

const firstCells = () => [...document.querySelectorAll(".jc-data-grid tbody tr")].map((tr) => tr.children[1]?.textContent ?? "");

function records(rows: typeof RECORDS) {
  render(
    <I18nextProvider i18n={i18n}>
      <RecordsGrid
        caption="Sample records"
        fields={["station", "pm10"]}
        records={rows}
        text={(row, field) => row[field as keyof (typeof RECORDS)[number]]}
        lead={[{ key: "line", label: "#", kind: "number", text: (row) => String(row.at + 2), filter: false }]}
        storageKey="test.records"
        empty={<p>The file holds no rows.</p>}
      />
    </I18nextProvider>,
  );
}

describe("records on the shared grid (T-6752)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    try {
      window.localStorage.clear();
    } catch {
      // no storage in this run
    }
  });

  it("keeps the file's line beside each record, sorts a field and searches the rows", () => {
    records(RECORDS);
    expect(screen.getByRole("rowheader", { name: "2" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: `${en.entityGrid.columnMenu}: station` }));
    fireEvent.click(screen.getByRole("menuitemradio", { name: en.entityGrid.sortAsc }));
    expect(firstCells()).toEqual(["Kallio", "Kamppi", "Töölö"]);
    // The line stays the record's own after the sort: Kallio is the file's fourth line.
    expect(within(screen.getAllByRole("row")[1]).getByRole("rowheader")).toHaveTextContent("4");

    fireEvent.change(screen.getByRole("searchbox", { name: en.dataGrid.searchRows }), { target: { value: "kamp" } });
    expect(firstCells()).toEqual(["Kamppi"]);
    fireEvent.change(screen.getByRole("searchbox", { name: en.dataGrid.searchRows }), { target: { value: "nowhere" } });
    expect(screen.getByText(en.dataGrid.noMatch)).toBeInTheDocument();
  });

  it("hides a field from Arrange columns", () => {
    records(RECORDS);
    fireEvent.click(screen.getByRole("button", { name: en.entityGrid.arrangeColumns }));
    const panel = screen.getByRole("dialog", { name: en.entityGrid.arrangeColumns });
    fireEvent.click(within(panel).getByRole("checkbox", { name: `${en.entityGrid.showColumn}: pm10` }));
    expect(screen.queryByRole("columnheader", { name: /^pm10/ })).toBeNull();
  });

  it("says what the page says when there is no record", () => {
    records([]);
    expect(screen.getByText("The file holds no rows.")).toBeInTheDocument();
  });

  it("loads the grid of a public page with its first drawing", async () => {
    render(
      <I18nextProvider i18n={i18n}>
        <LazyAdminGrid
          caption="Resources"
          rows={[{ name: "Stations as CSV" }]}
          rowKey={(row) => row.name}
          columns={[{ key: "name", label: "Resource", text: (row) => row.name }]}
          storageKey="test.lazy"
          empty={<p>none</p>}
        />
      </I18nextProvider>,
    );
    const table = await screen.findByRole("table", { name: "Resources" });
    expect(within(table).getByRole("rowheader", { name: "Stations as CSV" })).toBeInTheDocument();
  });

  it("names the entity on every change of a proposed write, after a sort too", () => {
    render(
      <I18nextProvider i18n={i18n}>
        <EntityWriteCard
          live={false}
          write={{
            endpoint: "air",
            slug: "airslug000000000000000000000000",
            entities: [
              { id: "urn:ngsi-ld:AirQualityObserved:kamppi", changes: [{ attribute: "pm10", before: 12, after: 18 }] },
              { id: "urn:ngsi-ld:AirQualityObserved:kallio", changes: [{ attribute: "no2", before: 3, after: 4 }] },
            ],
          }}
        />
      </I18nextProvider>,
    );
    fireEvent.click(screen.getByRole("button", { name: `${en.entityGrid.columnMenu}: ${en.agentRun.write.attribute}` }));
    fireEvent.click(screen.getByRole("menuitemradio", { name: en.entityGrid.sortAsc }));
    const rows = screen.getAllByRole("row").slice(1);
    expect(rows.map((row) => within(row).getByRole("rowheader").textContent)).toEqual(["kallio", "kamppi"]);
  });
});
