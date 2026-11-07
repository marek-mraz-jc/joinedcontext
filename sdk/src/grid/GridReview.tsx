import React from "react";
import { MAX_ENTITIES, MAX_REASON, reasonOk } from "./apply";
import type { AttributeChange } from "./apply";
import { CellView, targetName } from "./cells";
import type { CellLabels } from "./cells";
import type { RichCell } from "./model";
import { NGSI_LD_NULL } from "./RelationPicker";
import type { GridView } from "./useGridView";

function reviewValue(change: AttributeChange, value: unknown, locale: string, labels: CellLabels): React.ReactNode {
  if (value === undefined || value === null || value === NGSI_LD_NULL) return "—";
  const cell: RichCell =
    change.kind === "relationship"
      ? { kind: "relationship", value: undefined, object: value as string | string[] }
      : { kind: change.kind === "geo" ? "geo" : "property", value, unitCode: change.unitCode };
  return <CellView cell={cell} locale={locale} labels={labels} />;
}

/** The page buttons a pager shows: 1, the pages beside the current one and the last, `null` for a gap. */
/**
 * An attribute the decisions do not list once they are read: nothing is allowed (deny by default).
 * `undefined` while they are still being read.

/** What is typed and not applied yet, with Review and Discard; in the row dialog Review closes it first (T-4828). */
export function GridPending({ view, inDialog = false }: { view: GridView; inDialog?: boolean }): React.JSX.Element | null {
  const { editing, grid, labels, ruleProblems, setDetailId, setOpen, setRefused } = view;
  const { pendingChanges, clearEdits } = grid;
  if (!editing || pendingChanges.length === 0) return null;
  return (
    <div className="jc-grid-pending" role="status">
      <span>{`${pendingChanges.length} ${labels.pending}`}</span>
      {ruleProblems.size > 0 && <span className="jc-grid-to-correct">{`${ruleProblems.size} ${labels.toCorrect}`}</span>}
      <button
        type="button"
        disabled={ruleProblems.size > 0}
        onClick={() => {
          if (inDialog) setDetailId(null);
          setOpen(true);
        }}
      >
        {labels.review}
      </button>
      <button type="button" onClick={() => { clearEdits(); setRefused([]); }}>
        {labels.discard}
      </button>
    </div>
  );
}

/** The review before an apply: each change as the grid draws it, what happens to `observedAt`, and why (UI-67). */
export function GridReview({ view }: { view: GridView }): React.JSX.Element | null {
  const { open, labels, grid, rowName, columns, where, cellLabels, observed, setObserved, refused, reasonText, setReason, applying, apply } = view;
  const { pendingChanges, rows } = grid;
  const reasonHintId = React.useId();
  if (!open) return null;
  return (
    <section className="jc-grid-review" aria-label={labels.review}>
      {/* Scrolls sideways on a phone, so the keyboard can reach it as well (WCAG 2.1.1). */}
      <div className="jc-grid-review-rows" tabIndex={0} role="group" aria-label={labels.review}>
        <table>
          <thead>
            <tr>
              <th>{labels.entity}</th>
              <th>{labels.attribute}</th>
              <th>{labels.before}</th>
              <th>{labels.after}</th>
            </tr>
          </thead>
          <tbody>
            {/* What a person reads, drawn as the grid draws the cell: the row's name, the column's
                label and each value formatted; the URN only behind its copy button (T-5530). */}
            {pendingChanges.flatMap((entity) => {
              const row = rows.find((one) => one.id === entity.id);
              const name = row ? rowName(row) : targetName(entity.id).name;
              return entity.changes.map((change) => (
                <tr key={`${entity.id}-${change.attribute}`}>
                  <td>
                    {name}{" "}
                    <button type="button" className="jc-grid-copy" aria-label={`${labels.copyId}: ${name}`} title={labels.copyId} onClick={() => void navigator.clipboard?.writeText(entity.id)}>
                      ⧉
                    </button>
                  </td>
                  <td>{columns.find((col) => col.attr === change.attribute && col.meta === null)?.label ?? change.attribute}</td>
                  <td>{reviewValue(change, change.before, where, cellLabels)}</td>
                  <td>{reviewValue(change, change.after, where, cellLabels)}</td>
                </tr>
              ));
            })}
          </tbody>
        </table>
      </div>
      {/* What happens to `observedAt`, chosen once for the batch: a corrected value that keeps
          the old timestamp claims to have been observed then (UI-67). */}
      <fieldset>
        <label>
          <input
            type="radio"
            name="jc-grid-observed"
            checked={observed === "keep"}
            onChange={() => setObserved("keep")}
          />
          {labels.observedKeep}
        </label>
        <label>
          <input
            type="radio"
            name="jc-grid-observed"
            checked={observed === "now"}
            onChange={() => setObserved("now")}
          />
          {labels.observedNow}
        </label>
      </fieldset>
      {pendingChanges.length > MAX_ENTITIES && (
        <p className="jc-grid-error">{`${labels.error}: ${MAX_ENTITIES}`}</p>
      )}
      {refused.map((one) => (
        <p key={one.id} className="jc-grid-error">{`${one.id}: ${one.detail}`}</p>
      ))}
      <label className="jc-grid-reason">
        <span>{labels.reason}</span>
        <input
          required
          maxLength={MAX_REASON}
          value={reasonText}
          aria-invalid={reasonOk(reasonText) ? undefined : true}
          aria-describedby={reasonOk(reasonText) ? undefined : reasonHintId}
          onChange={(e) => setReason(e.target.value)}
        />
      </label>
      {!reasonOk(reasonText) && (
        <p id={reasonHintId} className="jc-grid-error">
          {labels.reasonNeeded}
        </p>
      )}
      <button
        type="button"
        disabled={applying || pendingChanges.length > MAX_ENTITIES || !reasonOk(reasonText)}
        onClick={() => void apply()}
      >
        {applying ? labels.applying : labels.apply}
      </button>
    </section>
  );
}
