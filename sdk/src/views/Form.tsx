import { useEffect, useMemo, useRef, useState } from "react";
import type { Cell, RelationshipObject, Row } from "../ngsi";
import { columnKind, format, targetsOf } from "../ngsi";
import type { Field, Schema, TypeSchema, WriteResult } from "../write";
import { fieldOf } from "../write";
import { optionLabel } from "../enums";
import { unitSymbol } from "../sdk/units";
import { NGSI_LD_NULL, RelationPicker } from "../grid/RelationPicker";
import type { PickerLabels } from "../grid/RelationPicker";
import type { TargetOption } from "../relations";

const PICKER_LABELS: PickerLabels = {
  search: "Search",
  none: "Nothing found that you can read",
  remove: "Remove",
  loading: "Searching…",
  failed: "The search failed; try again",
};

/** Searches the entities of one target class the person can read. */
export type TargetSearch = (target: string, text: string) => Promise<TargetOption[]>;

/**
 * The selected entity as a window of inputs, or a new one (AP-61, AP-62). Each input is what
 * the endpoint's schema says the attribute is: a select over an enum, a number within its
 * bounds, a pattern, a required mark. A save writes through the endpoint; a refusal stays on
 * the form beside the inputs with the reason, and nothing reloads.
 */
export function Form({ row, rows, fields, title, schema, defs, creating, search, onSave, onClose }: { row: Row | null; rows: Row[]; fields: string[]; title?: string; schema?: TypeSchema; defs?: Schema; creating: boolean; search?: TargetSearch; onSave: (id: string | null, patch: Record<string, Cell | RelationshipObject>) => Promise<WriteResult>; onClose: () => void }) {
  const dialog = useRef<HTMLDialogElement>(null);
  const form = useRef<HTMLFormElement>(null);
  const [draft, setDraft] = useState<Record<string, string>>({});
  // A relationship end holds targets, not text: picked, and written as a Relationship (DM-64).
  // Only the ends the person changed are kept; every other end reads the row.
  const [picked, setPicked] = useState<Record<string, string[]>>({});
  const linksOf = (field: string): string[] => picked[field] ?? (row ? targetsOf(row[field]) : []);
  const [localId, setLocalId] = useState("");
  const [problem, setProblem] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const open = creating || row !== null;
  useEffect(() => {
    setDraft(row ? Object.fromEntries(fields.map((f) => [f, format(row[f])])) : {});
    setPicked({});
    setLocalId("");
    setProblem(null);
    const element = dialog.current;
    if (!element) return;
    if (open && !element.open) element.showModal();
    if (!open && element.open) element.close();
  }, [row, fields, open]);
  // The titles of an enum's values in the language the page is in (UI-86).
  const language = document.documentElement.lang || navigator.language;
  const specs: Record<string, Field> = Object.fromEntries(fields.map((f) => [f, fieldOf(f, schema, columnKind(rows, f), defs, language)]));
  // One search per target class, kept across renders: the picker reads again when it changes.
  const targets = [...new Set(Object.values(specs).flatMap((spec) => (spec.target ? [spec.target] : [])))];
  const targetKey = targets.join(",");
  const searches = useMemo(
    () =>
      Object.fromEntries(
        targetKey
          .split(",")
          .filter((target) => target !== "")
          .map((target) => [target, (text: string) => (search ? search(target, text) : Promise.resolve([]))]),
      ) as Record<string, (text: string) => Promise<TargetOption[]>>,
    [search, targetKey],
  );
  const idPrefix = rows[0]?.id.includes(":") ? rows[0].id.slice(0, rows[0].id.lastIndexOf(":") + 1) : "";

  // The app runs in a frame sandboxed without `allow-forms` (AP-63), where the browser never
  // starts a form submission, so no submit event ever reaches React: the Save button and the
  // Enter key call this themselves, after the inputs' own validity check.
  const submit = async () => {
    if (!form.current?.reportValidity()) return;
    const patch: Record<string, Cell | RelationshipObject> = {};
    for (const field of fields) {
      const spec = specs[field];
      if (spec.input === "relation") {
        const targets = linksOf(field);
        if (row && targets.join(", ") === targetsOf(row[field]).join(", ")) continue;
        if (targets.length === 0) {
          if (spec.required) {
            setProblem(`${field} needs a ${spec.target}.`);
            return;
          }
          if (!row) continue;
        }
        // A cleared optional end is written as the NGSI-LD null, so the attribute goes (CIM 009 §4.5.0).
        patch[field] = { object: targets.length === 0 ? NGSI_LD_NULL : spec.many ? targets : targets[0] };
        continue;
      }
      // Neither is written from a text box: a geometry is picked on the map, and a LanguageProperty
      // written as the one language a row shows would drop every other language.
      if (spec.input === "geo" || spec.input === "language") continue;
      const text = draft[field] ?? "";
      if (row && text === format(row[field])) continue;
      if (spec.input === "number") patch[field] = text.trim() === "" ? null : Number(text);
      else if (spec.input === "checkbox") patch[field] = text === "true";
      else patch[field] = text;
    }
    if (!row && !creating) return;
    setSaving(true);
    setProblem(null);
    const result = await onSave(row ? row.id : `${idPrefix}${localId.trim()}`, patch);
    setSaving(false);
    if (result.ok) {
      onClose();
    } else {
      setProblem(result.detail ?? `The endpoint answered ${result.status}.`);
    }
  };

  const input = (field: string) => {
    const spec = specs[field];
    const value = draft[field] ?? "";
    const set = (next: string) => setDraft((d) => ({ ...d, [field]: next }));
    switch (spec.input) {
      case "geo":
      case "language":
        return <input value={value} readOnly />;
      case "select": {
        // A stored value the enum does not list is shown as it is and marked, never swapped for
        // the first option: the save skips an unchanged field, so it stays until someone picks.
        const outside = value !== "" && !spec.options?.some((option) => option.value === value);
        return (
          <select value={value} required={spec.required} aria-invalid={outside || undefined} onChange={(e) => set(e.target.value)}>
            <option value="">—</option>
            {outside && <option value={value}>{`${value} (not in the list)`}</option>}
            {spec.options?.map((option) => (
              <option key={option.value} value={option.value} title={option.description}>{optionLabel(option)}</option>
            ))}
          </select>
        );
      }
      case "number":
        return <input type="number" step="any" min={spec.min} max={spec.max} required={spec.required} value={value} onChange={(e) => set(e.target.value)} />;
      case "checkbox":
        return <input type="checkbox" checked={value === "true"} onChange={(e) => set(e.target.checked ? "true" : "false")} />;
      case "date":
        return <input type="datetime-local" required={spec.required} value={value.replace(/Z$/, "").slice(0, 16)} onChange={(e) => set(e.target.value === "" ? "" : `${e.target.value}:00Z`)} />;
      default:
        return <input type="text" pattern={spec.pattern} required={spec.required} value={value} onChange={(e) => set(e.target.value)} />;
    }
  };

  return (
    <>
      <p className="empty">Pick a point on the map or a row in the table to open the form.</p>
      <dialog ref={dialog} className="form-window" onClose={onClose} aria-label={title ?? "Form"}>
        {open && (
          <form
            ref={form}
            onSubmit={(e) => e.preventDefault()}
            onKeyDown={(e) => {
              if (e.key === "Enter" && e.target instanceof HTMLInputElement) {
                e.preventDefault();
                void submit();
              }
            }}
          >
            <h2>{title ?? (row ? "Entity" : "New entity")}</h2>
            {row ? (
              <p className="mono">{row.id}</p>
            ) : (
              <label className="field">
                <span>id</span>
                <input type="text" aria-label="id" required pattern="[A-Za-z0-9._~-]+" placeholder={`${idPrefix}…`} value={localId} onChange={(e) => setLocalId(e.target.value)} />
              </label>
            )}
            {fields.map((field) =>
              specs[field].input === "relation" ? (
                // The picker is a group of its own (chips, a combobox, remove buttons), so it is
                // named by the group and not wrapped in one label.
                <div key={field} className="field">
                  <span aria-hidden="true">
                    {field}
                    {specs[field].required ? " *" : ""}
                  </span>
                  <RelationPicker
                    label={field}
                    value={linksOf(field)}
                    end={{ target: specs[field].target ?? "", many: specs[field].many ?? false, required: specs[field].required }}
                    search={searches[specs[field].target ?? ""] ?? (() => Promise.resolve([]))}
                    labels={PICKER_LABELS}
                    onChange={(next) => setPicked((p) => ({ ...p, [field]: next }))}
                  />
                </div>
              ) : (
              <label key={field} className="field">
                <span>
                  {field}
                  {unitSymbol(specs[field].unit) ? ` (${unitSymbol(specs[field].unit)})` : ""}
                  {specs[field].required ? " *" : ""}
                </span>
                {input(field)}
              </label>
              ),
            )}
            {problem && <p role="alert" className="error">{problem}</p>}
            <div className="form-actions">
              <button type="button" onClick={onClose}>Close</button>
              <button type="button" className="primary" disabled={saving} onClick={() => void submit()}>{saving ? "Saving…" : "Save"}</button>
            </div>
            <p className="form-note">Written through the endpoint with your own access; the Policy decides.</p>
          </form>
        )}
      </dialog>
    </>
  );
}
