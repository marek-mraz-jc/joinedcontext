import { useMemo, useState } from "react";
import { displayName, format, Header, Page, Split, useAccess, useEntities, useFilters, useSave } from "@joinedcontext/sdk";
import type { FilterDef, Row } from "@joinedcontext/sdk";
import { EntityDetail } from "./components/EntityDetail";
import { EntityTable, type ColumnDef } from "./components/EntityTable";
import { FilterBar, SearchBox, SelectFilter } from "./components/filters";
import { Problem } from "./components/states";

const TYPE = "Alert";
const FILTERS: FilterDef[] = [
  { kind: "search", attrs: ["name", "description"], label: "Search" },
  { kind: "select", attr: "severity", label: "Severity" },
  { kind: "select", attr: "status", label: "Status" },
  { kind: "select", attr: "district", label: "District" },
];

/** Most urgent first; an unknown severity sorts last rather than first. */
const RANK: Record<string, number> = { critical: 0, high: 1, medium: 2, low: 3 };
const SEVERITY_WORD: Record<string, string> = { critical: "Critical", high: "High", medium: "Medium", low: "Low" };
const STATUS_WORD: Record<string, string> = { open: "Open", acknowledged: "Acknowledged", resolved: "Resolved" };

function rank(row: Row): number {
  return RANK[String(row.severity)] ?? 4;
}

/** Severity then newest first: the first row is the item to look at next. */
export function queueOrder(rows: Row[]): Row[] {
  return [...rows].sort((a, b) => rank(a) - rank(b) || String(b.dateIssued ?? "").localeCompare(String(a.dateIssued ?? "")));
}

function Severity({ value }: { value: unknown }): React.JSX.Element {
  const key = String(value ?? "");
  return (
    <span className="app-severity" data-severity={key}>
      {SEVERITY_WORD[key] ?? (key || "—")}
    </span>
  );
}

interface Outcome {
  done: number;
  failed: string[];
}

/** The queue of alerts, the chosen one beside it, and bulk status changes for editors. */
export default function App(): React.JSX.Element {
  const { rows, loading, error, reload } = useEntities(TYPE);
  const ordered = useMemo(() => queueOrder(rows), [rows]);
  const { shown, bind, reset } = useFilters(ordered, FILTERS);
  const { can } = useAccess();
  const save = useSave();
  const [picked, setPicked] = useState<ReadonlySet<string>>(new Set());
  const [openId, setOpenId] = useState<string | null>(null);
  const [outcome, setOutcome] = useState<Outcome | null>(null);
  const [working, setWorking] = useState(false);

  const allowed = can("updateAttrs", TYPE, "status");
  const chosen = ordered.filter((row) => picked.has(row.id));
  const open = ordered.find((row) => row.id === openId) ?? null;

  const toggle = (id: string) =>
    setPicked((before) => {
      const next = new Set(before);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });

  const move = async (status: "acknowledged" | "resolved") => {
    setWorking(true);
    setOutcome(null);
    const failed: string[] = [];
    let done = 0;
    for (const row of chosen) {
      if (await save.update(row.id, { status })) done += 1;
      else failed.push(displayName(row));
    }
    setWorking(false);
    setOutcome({ done, failed });
    setPicked(new Set());
    reload();
  };

  const columns: ColumnDef[] = [
    {
      attr: "select",
      label: "Select",
      render: (row) => (
        <input
          type="checkbox"
          aria-label={`Select ${displayName(row)}`}
          checked={picked.has(row.id)}
          onClick={(event) => event.stopPropagation()}
          onChange={() => toggle(row.id)}
        />
      ),
    },
    { attr: "name", label: "Alert", render: (row) => displayName(row) },
    { attr: "severity", label: "Severity", render: (row) => <Severity value={row.severity} /> },
    { attr: "status", label: "Status", render: (row) => STATUS_WORD[String(row.status)] ?? "—" },
    { attr: "district", label: "District" },
    { attr: "dateIssued", label: "Issued", render: (row) => format(row.dateIssued, "date") || "—" },
  ];

  const disabledWhy = !allowed.ok ? allowed.reason : chosen.length === 0 ? "Select one or more alerts first." : undefined;
  const actions = (
    <>
      <button type="button" disabled={disabledWhy !== undefined || working} title={disabledWhy} onClick={() => void move("acknowledged")}>
        Acknowledge {chosen.length > 0 ? chosen.length : ""}
      </button>
      <button type="button" disabled={disabledWhy !== undefined || working} title={disabledWhy} onClick={() => void move("resolved")}>
        Resolve {chosen.length > 0 ? chosen.length : ""}
      </button>
    </>
  );

  return (
    <div className="jc-shell">
      <main className="jc-main">
        <Page label="Alerts desk">
          <Header level={1} title="Alerts desk" subtitle="Open alerts, most urgent first" actions={actions} />
          {!allowed.ok && (
            <p className="app-note" role="note">
              {allowed.reason}
            </p>
          )}
          {outcome && (
            <p className="app-note" role="status">
              {outcome.done === 1 ? "1 alert changed." : `${outcome.done} alerts changed.`}
              {outcome.failed.length > 0 && ` Not changed: ${outcome.failed.join(", ")}.`}
            </p>
          )}
          <Problem error={save.problem} />
          <FilterBar
            shown={shown.length}
            total={rows.length}
            onReset={() => {
              reset();
              setPicked(new Set());
            }}
          >
            <SearchBox binding={bind(0)} placeholder="Name or text" />
            <SelectFilter binding={bind(1)} />
            <SelectFilter binding={bind(2)} />
            <SelectFilter binding={bind(3)} />
          </FilterBar>
          <Split ratio="2:1">
            <EntityTable
              rows={shown}
              columns={columns}
              pageSize={25}
              loading={loading}
              error={error}
              selected={openId}
              onSelect={(row) => setOpenId(row.id)}
              caption="Alerts"
              empty="No alert matches these filters."
            />
            {open ? (
              <EntityDetail
                row={open}
                title={displayName(open)}
                attrs={["severity", "status", "category", "district", "dateIssued", "description"]}
                onClose={() => setOpenId(null)}
              />
            ) : (
              <p className="app-hint">Choose an alert to read it here.</p>
            )}
          </Split>
        </Page>
      </main>
    </div>
  );
}
