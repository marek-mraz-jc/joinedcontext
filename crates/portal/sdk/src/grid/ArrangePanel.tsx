/**
 * The Arrange columns button and panel (T-6738), without knowing where the columns come from: the
 * entity grid passes its attributes and adds the metadata block (`children`), the array grid
 * (`DataGrid`, T-6739) its own columns. Which columns show, their order and a label of this view's
 * own: only the viewer's layout changes, never the data. A label is text: React renders it as
 * such, never as markup. On a phone the panel is a bottom sheet with 44 px targets (grid.css).
 */
import React, { useEffect, useId, useRef, useState } from "react";
import type { GridLabels } from "./useEntityGrid";

export interface ArrangeItem {
  key: string;
  /** What the column is called in this view. */
  label: string;
  /** What it is called when nobody renamed it: the placeholder, and what an empty label goes back to. */
  model: string;
  shown: boolean;
  /** Always shown and never moved (the row's name, a pinned column). */
  fixed: boolean;
  canUp: boolean;
  canDown: boolean;
}

export type ArrangeLabels = Pick<
  GridLabels,
  "arrangeColumns" | "arrangeDone" | "arrangeHint" | "alwaysShown" | "showColumn" | "renameColumn" | "renameHint" | "moveUp" | "moveDown" | "resetColumns" | "resetHint"
>;

export interface ArrangePanelProps {
  labels: ArrangeLabels;
  items: ArrangeItem[];
  onShow: (key: string, show: boolean) => void;
  onRename: (key: string, text: string | undefined) => void;
  onMove: (key: string, by: -1 | 1) => void;
  onReset: () => void;
  /** More settings under the list, such as the entity grid's metadata columns. */
  children?: React.ReactNode;
}

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

export function ArrangePanel({ labels, items, onShow, onRename, onMove, onReset, children }: ArrangePanelProps): React.JSX.Element {
  const [open, setOpen] = useState(false);
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
          <div className="jc-grid-arrange-body">
            <div className="jc-grid-arrange-cols">
              <ul className="jc-grid-arrange-list" aria-label={labels.arrangeColumns}>
                {items.map((item) => (
                  <li key={item.key} data-hidden={item.shown ? undefined : ""}>
                    <label className="jc-grid-arrange-show" title={item.fixed ? labels.alwaysShown : undefined}>
                      <input
                        type="checkbox"
                        checked={item.shown}
                        disabled={item.fixed}
                        aria-label={`${labels.showColumn}: ${item.label}`}
                        aria-description={item.fixed ? labels.alwaysShown : undefined}
                        onChange={() => onShow(item.key, !item.shown)}
                      />
                    </label>
                    <LabelField
                      value={item.label}
                      model={item.model}
                      name={`${labels.renameColumn}: ${item.model}`}
                      onCommit={(text) => onRename(item.key, text)}
                    />
                    <button
                      type="button"
                      className="jc-grid-arrange-move"
                      aria-label={`${labels.moveUp}: ${item.label}`}
                      title={labels.moveUp}
                      disabled={!item.canUp}
                      onClick={() => onMove(item.key, -1)}
                    >
                      ↑
                    </button>
                    <button
                      type="button"
                      className="jc-grid-arrange-move"
                      aria-label={`${labels.moveDown}: ${item.label}`}
                      title={labels.moveDown}
                      disabled={!item.canDown}
                      onClick={() => onMove(item.key, 1)}
                    >
                      ↓
                    </button>
                  </li>
                ))}
              </ul>
              <p className="jc-grid-arrange-hint">{labels.renameHint}</p>
            </div>
            {children}
          </div>
          <div className="jc-grid-arrange-foot">
            <button type="button" className="jc-grid-arrange-btn" onClick={onReset}>{labels.resetColumns}</button>
            <p className="jc-grid-arrange-hint">{labels.resetHint}</p>
          </div>
        </div>
      )}
    </div>
  );
}
