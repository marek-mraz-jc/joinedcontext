import { useMemo, useState } from "react";
import { Card, Page, Split, useEntities, useEntitySelection, useFilters } from "@joinedcontext/sdk";
import type { FilterDef } from "@joinedcontext/sdk";
import { EntityMap } from "../components/EntityMap";
import { EntityTable } from "../components/EntityTable";
import type { ColumnDef } from "../components/EntityTable";
import { FilterBar, SearchBox } from "../components/filters";
import { STATION, hasBikes } from "../stations";

const SEARCH: FilterDef[] = [{ kind: "search", attrs: ["name"], label: "Station" }];

/** The table's columns in words, the status as a coloured chip (T-2924). */
export const TABLE: ColumnDef[] = [
  { attr: "name", label: "Station" },
  { attr: "availableBikeNumber", label: "Bikes" },
  { attr: "freeSlotNumber", label: "Free slots" },
  { attr: "totalSlotNumber", label: "Capacity" },
  {
    attr: "status",
    label: "Status",
    render: (row) => {
      const status = typeof row.status === "string" && row.status !== "" ? row.status : "unknown";
      return (
        <span className="app-chip" data-status={status}>
          {status === "working" ? "Working" : status === "unknown" ? "Unknown" : status}
        </span>
      );
    },
  },
  { attr: "dateModified", label: "Updated" },
];

/**
 * Every station: a search by name, "only stations with bikes", the map coloured by bikes and the
 * table; a station chosen on either opens in the shell's entity panel (SDK-40), linked to the
 * Portal: a public App writes nothing.
 */
export function Stations() {
  const { rows, loading, error } = useEntities(STATION);
  const { shown: searched, bind, reset } = useFilters(rows, SEARCH);
  const [withBikes, setWithBikes] = useState(false);
  const shown = useMemo(() => (withBikes ? searched.filter(hasBikes) : searched), [searched, withBikes]);
  const { selected, select } = useEntitySelection();
  const selectedId = selected?.id ?? null;

  return (
    <Page label="Stations">
      <FilterBar
        shown={shown.length}
        total={rows.length}
        onReset={() => {
          reset();
          setWithBikes(false);
        }}
      >
        <SearchBox binding={bind(0)} />
        <label className="jc-filter">
          <input type="checkbox" checked={withBikes} onChange={(event) => setWithBikes(event.target.checked)} />
          <span>Only stations with bikes</span>
        </label>
      </FilterBar>
      <Split ratio="2:1">
        <EntityMap
          rows={shown}
          location="location"
          label="name"
          color="availableBikeNumber"
          selected={selectedId}
          onSelect={(row) => select({ id: row.id, type: STATION })}
        />
        <Card label="Station">
          <p>Choose a station on the map or in the table to see its bikes and free slots.</p>
        </Card>
      </Split>
      <EntityTable
        rows={shown}
        columns={TABLE}
        loading={loading}
        error={error}
        selected={selectedId}
        onSelect={(row) => select({ id: row.id, type: STATION })}
        initialSort={{ attr: "name", dir: "asc" }}
        caption="Stations"
        empty="No station matches."
      />
    </Page>
  );
}
