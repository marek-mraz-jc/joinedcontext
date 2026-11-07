import { Page, useEntities, useEntitySelection, useFilters } from "@joinedcontext/sdk";
import type { FilterDef } from "@joinedcontext/sdk";
import { EntityTable } from "../components/EntityTable";
import type { ColumnDef } from "../components/EntityTable";
import { ExportButton } from "../components/ExportButton";
import { DateRangeFilter, FilterBar, SearchBox, SelectFilter } from "../components/filters";

export const ALERT = "Alert";

/** The table's columns in the order a desk scans them: what, where, and when it holds. */
export const COLUMNS: ColumnDef[] = [
  { attr: "name", label: "Alert" },
  { attr: "category", label: "Category" },
  { attr: "subCategory", label: "Subcategory" },
  { attr: "address", label: "Address" },
  { attr: "validFrom", label: "Valid from" },
  { attr: "validTo", label: "Valid to" },
  { attr: "dateIssued", label: "Issued" },
];

const FILTERS: FilterDef[] = [
  { kind: "search", attrs: ["name", "description", "address"], label: "Search" },
  { kind: "select", attr: "category", label: "Category" },
  { kind: "select", attr: "subCategory", label: "Subcategory" },
  { kind: "dateRange", attr: "validTo", label: "Valid to" },
];

/**
 * Every alert the endpoint answers: filters, the table sorted by any column, an export of what is
 * shown, and a row opens the alert in the SDK's entity panel (SDK-40). Nothing here writes (owner
 * decision (a), T-3016), so the panel links to the alert in the Portal instead of offering Edit.
 */
export function AlertDesk() {
  const { rows, loading, error } = useEntities(ALERT);
  const { shown, bind, reset } = useFilters(rows, FILTERS);
  const { selected, select } = useEntitySelection();

  return (
    <Page label="Alerts">
      <FilterBar shown={shown.length} total={rows.length} onReset={reset}>
        <SearchBox binding={bind(0)} />
        <SelectFilter binding={bind(1)} />
        <SelectFilter binding={bind(2)} />
        <DateRangeFilter binding={bind(3)} />
      </FilterBar>
      <EntityTable
        rows={shown}
        columns={COLUMNS}
        loading={loading}
        error={error}
        selected={selected?.id ?? null}
        onSelect={(row) => select({ id: row.id, type: ALERT })}
        initialSort={{ attr: "validTo", dir: "desc" }}
        caption="Traffic alerts"
        empty={rows.length > 0 ? "No alert matches these filters." : "The endpoint holds no alerts right now."}
      />
      <ExportButton rows={shown} columns={COLUMNS.map((column) => column.attr)} filename="alerts" />
    </Page>
  );
}
