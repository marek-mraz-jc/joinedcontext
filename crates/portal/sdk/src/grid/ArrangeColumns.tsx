/**
 * The grid toolbar's Arrange columns panel (T-6738): which columns show, their order, a label of
 * this view's own and the metadata columns beside one attribute, each with a line that says what it
 * does. What it changes is the viewer's layout (`onLayout`, a saved view): the Endpoint and its data
 * are never written. A label is text: React renders it as such, never as markup. On a phone the
 * panel is a bottom sheet with 44 px targets (grid.css).
 */
import React, { useEffect, useId, useRef, useState } from "react";
import { readableName } from "./readableName";
import { NAME_COLUMN } from "./useEntityGrid";
import type { GridLabels, MetaKey } from "./useEntityGrid";
import type { GridView } from "./useGridView";
import { moveTo } from "./GridHead";

const META: { key: MetaKey; label: keyof GridLabels; hint: keyof GridLabels }[] = [
  { key: "observedAt", label: "observedAt", hint: "observedAtHint" },
  { key: "unit", label: "unit", hint: "unitHint" },
  { key: "datasetId", label: "datasetId", hint: "datasetIdHint" },
  { key: "createdAt", label: "createdAt", hint: "createdAtHint" },
  { key: "modifiedAt", label: "modifiedAt", hint: "modifiedAtHint" },
];

/** A label typed for one column: kept on blur or Enter, an empty one goes back to the model's. */
function LabelField({ value, model, name, onCommit }: { value: string; model: string; name: string; onCommit: (text: string | undefined) => void }) {
  const [draft, setDraft] = useState(value);
  useEffect(() => setDraft(value), [value]);
  const commit = () => {
    const text = draft.trim();
    if (text === value) return;
    onCommit(text === "" || text === model ? undefined : text);
  };
  return (
    <input
      className="jc-grid-arrange-input"
      aria-label={name}
      value={draft}
      placeholder={model}
      maxLength={80}
      onChange={(e) => setDraft(e.target.value)}
      onBlur={commit}
      onKeyDown={(e) => {
        if (e.key === "Enter") commit();
      }}
    />
  );
}

export function ArrangeColumns({ view }: { view: GridView }): React.JSX.Element | null {
  const { options, grid, labels, columns, order, firstMovable, layout, setLayout, ownHidden, hideColumn, showColumns, renameColumn, baseLabel, handleMetaCheck } = view;
  const [open, setOpen] = useState(false);
  const [metaFor, setMetaFor] = useState<string>("");
  const button = useRef<HTMLButtonElement>(null);
  const panel = useRef<HTMLDivElement>(null);
  const id = useId();
  const close = () => {
    setOpen(false);
    button.current?.focus();
  };
  useEffect(() => {
    if (!open) return;
    panel.current?.querySelector<HTMLElement>("h2")?.focus();
    const outside = (e: MouseEvent) => {
      if (!panel.current?.contains(e.target as Node) && !button.current?.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener("mousedown", outside);
    return () => document.removeEventListener("mousedown", outside);
  }, [open]);
  if (order.length === 0) return null;
  const modelLabel = (attr: string): string => options.config.columns.find((column) => column.attr === attr)?.label ?? readableName(attr);
  const shownLabel = (attr: string): string => {
    const col = columns.find((each) => each.attr === attr && each.meta === null);
    return layout.labels?.[attr] ?? (col ? baseLabel(col) : modelLabel(attr));
  };
  const pinned = new Set(columns.filter((col) => col.pinned && col.attr !== null).map((col) => col.attr!));
  const fixed = (attr: string) => attr === NAME_COLUMN || pinned.has(attr);
  const rows = [...order, ...ownHidden.filter((attr) => !order.includes(attr))];
  const chosen = order.includes(metaFor) ? metaFor : order[0];
  const shownMeta = grid.state.shown[chosen] ?? [];
  const reset = () => {
    setLayout({});
    if (ownHidden.length > 0) showColumns(ownHidden);
  };
  return (
    <div className="jc-grid-arrange">
      <button
        ref={button}
        type="button"
        className="jc-grid-arrange-btn jc-grid-arrange-toggle"
        aria-expanded={open}
        aria-controls={open ? id : undefined}
        onClick={() => (open ? close() : setOpen(true))}
      >
        <span className="jc-grid-menu-icon" aria-hidden="true">☰</span>
        {labels.arrangeColumns}
      </button>
      {open && (
        <div
          ref={panel}
          id={id}
          className="jc-grid-arrange-panel"
          role="dialog"
          aria-labelledby={`${id}-title`}
          aria-describedby={`${id}-hint`}
          onKeyDown={(e) => {
            // The grid's own arrow and Enter keys do not apply inside the panel.
            e.stopPropagation();
            if (e.key === "Escape") close();
          }}
        >
          <div className="jc-grid-arrange-head">
            <h2 id={`${id}-title`} tabIndex={-1}>{labels.arrangeColumns}</h2>
            <button type="button" className="jc-grid-arrange-done" onClick={close}>{labels.arrangeDone}</button>
          </div>
          <p id={`${id}-hint`} className="jc-grid-arrange-hint">{labels.arrangeHint}</p>
          <ul className="jc-grid-arrange-list" aria-label={labels.arrangeColumns}>
            {rows.map((attr) => {
              const shown = order.includes(attr);
              const at = order.indexOf(attr);
              const movable = shown && !fixed(attr);
              const label = shownLabel(attr);
              return (
                <li key={attr} data-hidden={shown ? undefined : ""}>
                  <label className="jc-grid-arrange-show">
                    <input
                      type="checkbox"
                      checked={shown}
                      disabled={fixed(attr)}
                      aria-label={`${labels.showColumn}: ${label}`}
                      onChange={() => (shown ? hideColumn(attr) : showColumns([attr]))}
                    />
                  </label>
                  <LabelField
                    value={label}
                    model={modelLabel(attr)}
                    name={`${labels.renameColumn}: ${modelLabel(attr)}`}
                    onCommit={(text) => renameColumn(attr, text)}
                  />
                  <button
                    type="button"
                    className="jc-grid-arrange-move"
                    aria-label={`${labels.moveUp}: ${label}`}
                    title={labels.moveUp}
                    disabled={!movable || at <= firstMovable}
                    onClick={() => setLayout({ ...layout, order: moveTo(order, attr, at - 1) })}
                  >
                    ↑
                  </button>
                  <button
                    type="button"
                    className="jc-grid-arrange-move"
                    aria-label={`${labels.moveDown}: ${label}`}
                    title={labels.moveDown}
                    disabled={!movable || at >= order.length - 1}
                    onClick={() => setLayout({ ...layout, order: moveTo(order, attr, at + 1) })}
                  >
                    ↓
                  </button>
                </li>
              );
            })}
          </ul>
          <p className="jc-grid-arrange-hint">{labels.renameHint}</p>
          <fieldset className="jc-grid-arrange-meta">
            <legend>{labels.metaHeading}</legend>
            <p className="jc-grid-arrange-hint">{labels.metaHint}</p>
            <label className="jc-grid-arrange-for">
              {labels.metaFor}
              <select value={chosen} onChange={(e) => setMetaFor(e.target.value)}>
                {order.map((attr) => (
                  <option key={attr} value={attr}>{shownLabel(attr)}</option>
                ))}
              </select>
            </label>
            {META.map((meta) => (
              <label key={meta.key} className="jc-grid-arrange-check">
                <input type="checkbox" checked={shownMeta.includes(meta.key)} onChange={() => handleMetaCheck(chosen, meta.key)} />
                <span>
                  <span className="jc-grid-arrange-check-label">{labels[meta.label] as string}</span>
                  <span className="jc-grid-arrange-hint">{labels[meta.hint] as string}</span>
                </span>
              </label>
            ))}
          </fieldset>
          <div className="jc-grid-arrange-foot">
            <button type="button" className="jc-grid-arrange-btn" onClick={reset}>{labels.resetColumns}</button>
            <p className="jc-grid-arrange-hint">{labels.resetHint}</p>
          </div>
        </div>
      )}
    </div>
  );
}
