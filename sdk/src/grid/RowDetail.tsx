import React, { useEffect, useId, useRef, useState } from "react";
import { targetName } from "./cells";
import type { RichRow } from "./model";
import type { VisibleColumn } from "./useEntityGrid";
import { useRowChanges } from "./EntityChanges";
import type { RowChanges } from "./EntityChanges";

export interface RowDetailLabels {
  /** The dialog's own name, before the row's: "Details". */
  rowDetail: string;
  close: string;
  /** The tab with the row's values: "Fields". */
  rowFields: string;
  /** The tab with one attribute's history at a time: "History", and its attribute picker. */
  rowHistory: string;
  historyAttr: string;
  /** The tab with who changed one attribute, when and through what (T-5529): "Metadata". */
  rowMetadata?: string;
  /** The button that copies the row's identifier, and what it says for a moment after (T-5599). */
  copyId?: string;
  copied?: string;
}

/** A tab the host adds after Fields and History, such as the row's comments (T-4828). */
export interface RowDetailTab {
  id: string;
  label: string;
  content: React.ReactNode;
}

export interface RowDetailProps {
  row: RichRow;
  /** What the row is called: its primary field, else its name, else its id. */
  name: string;
  /** The grid's columns: their attributes come first, in the grid's order and with its labels. */
  columns: VisibleColumn[];
  /** How one attribute's value is drawn, the grid's own cell renderer, editors included. */
  renderValue: (row: RichRow, column: VisibleColumn) => React.ReactNode;
  labels: RowDetailLabels;
  onClose: () => void;
  /** One attribute's history, where the source reads history and the config allows it. */
  history?: (attr: string, label: string) => React.ReactNode;
  /** One attribute's created and changed times and its last writer, with the History tab's picker. */
  metadata?: (attr: string) => React.ReactNode;
  /**
   * Who changed what and when (T-6333), where the source reads kept versions: one read per row,
   * a 403 (no `readVersions`) hides the tab and the marks beside changed fields.
   */
  changes?: RowChanges;
  /** The host's own tabs after Fields and History. */
  tabs?: RowDetailTab[];
  /** What the host adds under the attributes, in the Fields tab. */
  children?: React.ReactNode;
  /** The tab and attribute to open on, such as a host's address names them (T-5602); unknown ones read as the defaults. */
  place?: { tab?: string | null; attr?: string | null };
  /** Each change of tab or attribute, for a host that keeps them in the address. */
  onPlace?: (place: { tab: string; attr: string }) => void;
}

/**
 * The attribute History opens on (T-5602): the first one that holds a number or carries an
 * `observedAt`, what a person reads history for, else the first one. A name does not change.
 */
export function measuredAttr(row: RichRow, attrs: string[]): string {
  const measured = attrs.find((attr) => {
    const cell = row.cells[attr];
    const one = Array.isArray(cell) ? cell[0] : cell;
    return one !== undefined && one.kind === "property" && (typeof one.value === "number" || one.observedAt !== undefined);
  });
  return measured ?? attrs[0] ?? "";
}

/** One attribute on its own: its value, then its metadata and its readings over time where the host has them. */
export function AttributeView({ label, value, metadata, history }: { label: string; value: React.ReactNode; metadata?: React.ReactNode; history?: React.ReactNode }): React.JSX.Element {
  return (
    <div className="jc-grid-attr-view">
      <p className="jc-grid-attr-value" aria-label={label}>
        {value}
      </p>
      {metadata ? <section className="jc-grid-detail-section">{metadata}</section> : null}
      {history ? <section className="jc-grid-detail-section">{history}</section> : null}
    </div>
  );
}

/**
 * One row whole, as a modal dialog (T-3097, T-4828): every attribute the entity carries in the
 * Fields tab, the grid's columns first and then the rest, each drawn by the grid's own cell
 * renderer, so an editable attribute is edited here exactly as in its cell and joins the same
 * pending list; History reads one attribute's temporal values; the host adds its own tabs. The
 * browser's modal dialog keeps focus inside and the page behind inert; the heading takes focus when
 * it opens and Escape outside a text box closes it.
 */
export function RowDetail({ row, name, columns, renderValue, labels, onClose, history, metadata, changes, tabs = [], children, place, onPlace }: RowDetailProps): React.JSX.Element {
  const headingId = useId();
  const tabsId = useId();
  const heading = useRef<HTMLHeadingElement>(null);
  const dialog = useRef<HTMLDialogElement>(null);
  const [tab, setOwnTab] = useState(place?.tab ?? "fields");
  // "Copied" for two seconds after the identifier reached the clipboard.
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!copied) return;
    const timer = setTimeout(() => setCopied(false), 2000);
    return () => clearTimeout(timer);
  }, [copied]);
  useEffect(() => {
    const element = dialog.current;
    if (element && !element.open) element.showModal();
    return () => element?.close();
  }, []);
  useEffect(() => {
    heading.current?.focus();
  }, [row.id]);

  // Unpinned copies: in the dialog the primary attribute is a value or an editor like the rest, not
  // the button that opened it.
  // The id is behind the Copy identifier button, never text on screen (T-5599); a model's `id`
  // slot is not a field to read either.
  const shown = columns
    .filter((column) => column.attr !== null && column.attr !== "id" && column.meta === null)
    .map((column) => ({ ...column, pinned: false }));
  const named = new Set(shown.map((column) => column.attr));
  const rest: VisibleColumn[] = Object.keys(row.cells)
    .filter((attr) => !named.has(attr))
    .sort()
    .map((attr) => ({ key: attr, attr, meta: null, label: attr, pinned: false }));
  const all = [...shown, ...rest];

  const attrs = all.map((column) => column.attr ?? "");
  const [attr, setOwnAttr] = useState<string>(() =>
    place?.attr && attrs.includes(place.attr) ? place.attr : measuredAttr(row, attrs),
  );
  const setTab = (next: string) => {
    setOwnTab(next);
    onPlace?.({ tab: next, attr });
  };
  const setAttr = (next: string) => {
    setOwnAttr(next);
    onPlace?.({ tab, attr: next });
  };
  // An attribute's own view reads that attribute only (owner 2026-11-01): no whole-row versions read.
  const rowChanges = useRowChanges(tab === "attribute" ? undefined : changes, row.id, all.map((column) => ({ attr: column.attr ?? "", label: column.label })), () => setTab("changes"));
  const historyTab = history && all.length > 0 ? [{ id: "history", label: labels.rowHistory }] : [];
  const metadataTab = metadata && all.length > 0 ? [{ id: "metadata", label: labels.rowMetadata ?? "Metadata" }] : [];
  // A clicked cell opens on its attribute: value, metadata, readings over time and changes in one
  // view (owner 2026-11-01), named by the attribute; the row's other tabs stay beside it.
  const attributeTab = place?.tab === "attribute" && all.length > 0 ? [{ id: "attribute", label: all.find((column) => column.attr === attr)?.label ?? attr }] : [];
  const list = [...attributeTab, { id: "fields", label: labels.rowFields }, ...historyTab, ...metadataTab, ...rowChanges.tabs, ...tabs.map(({ id, label }) => ({ id, label }))];
  const current = list.some((one) => one.id === tab) ? tab : "fields";
  const tabId = (id: string) => `${tabsId}-tab-${id}`;
  const panelId = (id: string) => `${tabsId}-panel-${id}`;
  // Arrow keys move between tabs, Home and End to the ends (WAI-ARIA tabs, automatic activation).
  const onTabKey = (e: React.KeyboardEvent, index: number) => {
    const to = { ArrowRight: index + 1, ArrowLeft: index - 1, Home: 0, End: list.length - 1 }[e.key];
    if (to === undefined) return;
    e.preventDefault();
    const next = list[(to + list.length) % list.length];
    setTab(next.id);
    document.getElementById(tabId(next.id))?.focus();
  };
  const label = all.find((column) => column.attr === attr)?.label ?? attr;
  // Out of the modal state first: while it is modal the page behind is inert, so the host's focus
  // back to the grid went nowhere and the focus fell to the body once the dialog left (T-5619).
  const close = () => {
    dialog.current?.close();
    onClose();
  };

  return (
    <dialog
      ref={dialog}
      className="jc-grid-detail"
      aria-labelledby={headingId}
      // The browser's Escape would close the dialog from inside a cell editor too, where Escape
      // puts the value back: the dialog closes on Escape only outside a text box.
      onCancel={(e) => e.preventDefault()}
      onKeyDown={(e) => {
        const typing = /^(INPUT|SELECT|TEXTAREA)$/.test((e.target as HTMLElement).tagName);
        if (e.key === "Escape" && !typing) {
          e.preventDefault();
          close();
        }
      }}
    >
      <div className="jc-grid-detail-head">
        <h3 id={headingId} ref={heading} tabIndex={-1}>
          {/* A row named by nothing but its id reads as the id's last part, as its chips do. */}
          {`${labels.rowDetail}: ${name === row.id ? targetName(row.id).name : name}`}
        </h3>
        <span className="jc-grid-detail-actions">
          <button
            type="button"
            onClick={() => {
              void navigator.clipboard?.writeText(row.id).then(() => setCopied(true), () => setCopied(false));
            }}
          >
            {copied ? (labels.copied ?? "Copied") : (labels.copyId ?? "Copy identifier")}
          </button>
          <button type="button" className="jc-grid-detail-close" onClick={close} aria-label={labels.close} title={labels.close}>
            <svg viewBox="0 0 20 20" width="18" height="18" aria-hidden="true" fill="none" stroke="currentColor" strokeWidth="1.75" strokeLinecap="round">
              <path d="M5 5l10 10M15 5L5 15" />
            </svg>
          </button>
        </span>
        <span className="jc-grid-sr-only" role="status">
          {copied ? (labels.copied ?? "Copied") : ""}
        </span>
      </div>
      <div role="tablist" aria-labelledby={headingId} className="jc-grid-detail-tabs">
        {list.map((one, index) => (
          <button
            key={one.id}
            id={tabId(one.id)}
            type="button"
            role="tab"
            aria-selected={current === one.id}
            aria-controls={panelId(one.id)}
            tabIndex={current === one.id ? 0 : -1}
            onClick={() => setTab(one.id)}
            onKeyDown={(e) => onTabKey(e, index)}
          >
            {one.label}
          </button>
        ))}
      </div>
      <div role="tabpanel" id={panelId(current)} aria-labelledby={tabId(current)} className="jc-grid-detail-panel">
        {current === "fields" ? (
          <>
            <dl className="jc-grid-detail-list">
              {all.map((column) => (
                <div key={column.key} className="jc-grid-detail-item">
                  <dt>
                    {column.label}
                    {rowChanges.mark(column.attr, column.label)}
                  </dt>
                  <dd>{renderValue(row, column)}</dd>
                </div>
              ))}
            </dl>
            {children}
          </>
        ) : null}
        {(current === "history" && history) || (current === "metadata" && metadata) ? (
          <>
            <label className="jc-grid-detail-attr">
              {labels.historyAttr}
              <select value={attr} onChange={(e) => setAttr(e.target.value)}>
                {all.map((column) => (
                  <option key={column.key} value={column.attr ?? ""}>
                    {column.label}
                  </option>
                ))}
              </select>
            </label>
            {/* A new attribute is a new read: nothing of the last one's window or answer carries over. */}
            <React.Fragment key={attr}>{current === "history" ? history?.(attr, label) : metadata?.(attr)}</React.Fragment>
          </>
        ) : null}
        {current === "attribute" ? (
          <>
            <label className="jc-grid-detail-attr">
              {labels.historyAttr}
              <select value={attr} onChange={(e) => setAttr(e.target.value)}>
                {all.map((column) => (
                  <option key={column.key} value={column.attr ?? ""}>
                    {column.label}
                  </option>
                ))}
              </select>
            </label>
            <AttributeView
              key={attr}
              label={label}
              value={renderValue(row, all.find((column) => column.attr === attr) ?? all[0])}
              metadata={metadata?.(attr)}
              history={history?.(attr, label)}
            />
          </>
        ) : null}
        {current === "changes" ? rowChanges.panel : null}
        {tabs.find((one) => one.id === current)?.content}
      </div>
    </dialog>
  );
}
