import { useEffect, useId, useState } from "react";
import { Card, displayName, Page, Problem, Split, useAccess, useEntities, useEntitySelection, useFilters, useSave } from "@joinedcontext/sdk";
import type { FilterDef } from "@joinedcontext/sdk";
import { EntityForm } from "../components/EntityForm";
import { EntityMap } from "../components/EntityMap";
import { EntityTable } from "../components/EntityTable";
import { FilterBar, SearchBox, SelectFilter } from "../components/filters";
import { ALERT, COLUMNS, READ_ONLY, WRITABLE, isOwn } from "../alerts";

const FILTERS: FilterDef[] = [
  { kind: "search", attrs: ["name", "address"], label: "Search" },
  { kind: "select", attr: "category", label: "Category" },
  { kind: "select", attr: "subCategory", label: "Subcategory" },
];

function ReadOnlyNote() {
  return (
    <details className="app-muted">
      <summary>Fields this form does not write</summary>
      <ul>
        {READ_ONLY.map(([attr, why]) => (
          <li key={attr}>
            {attr}: {why}
          </li>
        ))}
      </ul>
    </details>
  );
}

/**
 * Every alert: filters, the map and the table. An alert on either opens in the shell's entity panel
 * (SDK-40), where a steward corrects its plain fields. A steward also gets the record form: a new alert, and the names language by language and the place of the chosen one, which the
 * panel leaves to the Portal; on an alert a steward added, Delete. Those sit in a toolbar above the
 * map, for the alert last opened, so the panel never covers them. The controls follow the
 * endpoint's answer (`can`), and the gateway refuses the same write for anyone else (AP-09, AP-96).
 */
export function Alerts() {
  const { rows, loading, error, reload } = useEntities(ALERT);
  const { shown, bind, reset } = useFilters(rows, FILTERS);
  const { select: choose, clear, saved } = useEntitySelection();
  // A change saved in the panel shows in the table and on the map.
  useEffect(() => {
    if (saved > 0) reload();
  }, [saved, reload]);
  const [mode, setMode] = useState<"view" | "edit" | "new">("view");
  // The alert last opened: the one the steward's own actions work on, still there once the panel closes.
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const { can } = useAccess();
  const save = useSave();
  const [removing, setRemoving] = useState(false);
  const [why, setWhy] = useState("");
  const [tried, setTried] = useState(false);
  const missing = tried && why.trim() === "";
  const nameId = useId();
  const selected = shown.find((row) => row.id === selectedId) ?? null;
  const open = (id: string) => {
    setSelectedId(id);
    choose({ id, type: ALERT });
    setMode("view");
  };
  const done = (id: string) => {
    open(id);
    reload();
  };
  // A form opens below the table: the panel steps aside so it never covers it.
  const startForm = (next: "edit" | "new") => {
    clear();
    setMode(next);
  };

  const mayCreate = can("createEntity", ALERT).ok;
  const mayEdit = can("updateAttrs", ALERT).ok;
  const mayDelete = can("deleteEntity", ALERT).ok;
  const mayRemove = selected !== null && mayDelete && isOwn(selected);

  return (
    <Page label="Alerts">
      <FilterBar shown={shown.length} total={rows.length} onReset={reset}>
        <SearchBox binding={bind(0)} />
        <SelectFilter binding={bind(1)} />
        <SelectFilter binding={bind(2)} />
      </FilterBar>
      {(mayCreate || (selected && (mayEdit || mayRemove))) && (
        <div className="app-actions" role="toolbar" aria-label="Alert actions">
          {mayCreate && mode !== "new" && (
            <button type="button" onClick={() => startForm("new")}>
              New alert
            </button>
          )}
          {selected && mode === "view" && (mayEdit || mayRemove) && (
            <>
              <span id={nameId}>{displayName(selected)}</span>
              {mayEdit && (
                <button type="button" aria-describedby={nameId} onClick={() => startForm("edit")}>
                  Correct names and place
                </button>
              )}
              {mayRemove && !removing && (
                <button
                  type="button"
                  aria-describedby={nameId}
                  disabled={save.saving}
                  onClick={() => {
                    setWhy("");
                    setTried(false);
                    save.clear();
                    setRemoving(true);
                  }}
                >
                  Delete
                </button>
              )}
            </>
          )}
          {mayRemove && removing && (
            // The confirm step asks why, as the Portal's own delete does (DM-113): the reason goes
            // with the delete, and nothing is sent without one.
            <section className="app-confirm" aria-label={`Delete ${displayName(selected)}`}>
              <p>
                <strong>{`Delete ${displayName(selected)}?`}</strong> This cannot be undone.
              </p>
              <label htmlFor={`${nameId}-why`}>Reason</label>
              <input
                id={`${nameId}-why`}
                required
                maxLength={500}
                autoComplete="off"
                aria-describedby={`${nameId}-hint${missing ? ` ${nameId}-missing` : ""}`}
                aria-invalid={missing}
                value={why}
                onChange={(event) => setWhy(event.target.value)}
              />
              <p id={`${nameId}-hint`} className="app-confirm-hint">Why you delete it, recorded with the deletion, for example: Duplicate of the Senaatintori closure.</p>
              {missing && (
                <p id={`${nameId}-missing`} className="app-confirm-error" role="alert">
                  Enter why you delete it.
                </p>
              )}
              <button
                type="button"
                disabled={save.saving}
                onClick={() => {
                  setTried(true);
                  if (why.trim() === "") return;
                  void save.remove(selected.id, why).then((ok) => {
                    if (ok) {
                      setRemoving(false);
                      setSelectedId(null);
                      clear();
                      reload();
                    }
                  });
                }}
              >
                Confirm delete
              </button>
              <button type="button" disabled={save.saving} onClick={() => setRemoving(false)}>
                Cancel
              </button>
            </section>
          )}
          <Problem error={save.problem} />
        </div>
      )}
      <Split ratio="2:1">
        <EntityMap rows={shown} location="location" label="name" selected={selectedId} onSelect={(row) => open(row.id)} />
        <Card label="Alert">
          <p>Choose an alert on the map or in the table to read it.</p>
        </Card>
      </Split>
      <EntityTable
        rows={shown}
        columns={COLUMNS}
        loading={loading}
        error={error}
        selected={selectedId}
        onSelect={(row) => open(row.id)}
        initialSort={{ attr: "validFrom", dir: "desc" }}
        caption="Alerts"
        empty="No alert matches."
      />
      {mode === "new" && (
        <Split ratio="2:1">
          <EntityForm type={ALERT} fields={WRITABLE} rows={rows} title="New alert" onSaved={done} onCancel={() => setMode("view")} />
          <ReadOnlyNote />
        </Split>
      )}
      {selected && mode === "edit" && (
        <Split ratio="2:1">
          <EntityForm type={ALERT} row={selected} fields={WRITABLE} rows={rows} onSaved={done} onCancel={() => setMode("view")} />
          <ReadOnlyNote />
        </Split>
      )}
    </Page>
  );
}
