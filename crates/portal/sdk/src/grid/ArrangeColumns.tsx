/**
 * The grid toolbar's Arrange columns panel (T-6738): which columns show, their order, a label of
 * this view's own and the metadata columns beside one attribute, each with a line that says what it
 * does. What it changes is the viewer's layout (`onLayout`, a saved view): the Endpoint and its data
 * are never written. A label is text: React renders it as such, never as markup. On a phone the
 * panel is a bottom sheet with 44 px targets (grid.css).
 */
import React, { useState } from "react";
import { ArrangePanel } from "./ArrangePanel";
import type { ArrangeItem } from "./ArrangePanel";
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

export function ArrangeColumns({ view }: { view: GridView }): React.JSX.Element | null {
  const { options, grid, labels, columns, order, firstMovable, layout, setLayout, ownHidden, hideColumn, showColumns, renameColumn, baseLabel, handleMetaCheck } = view;
  const [metaFor, setMetaFor] = useState<string>("");
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
  const items: ArrangeItem[] = rows.map((attr) => {
    const shown = order.includes(attr);
    const at = order.indexOf(attr);
    const movable = shown && !fixed(attr);
    return { key: attr, label: shownLabel(attr), model: modelLabel(attr), shown, fixed: fixed(attr), canUp: movable && at > firstMovable, canDown: movable && at < order.length - 1 };
  });
  return (
    <ArrangePanel
      labels={labels}
      items={items}
      onShow={(attr, show) => (show ? showColumns([attr]) : hideColumn(attr))}
      onRename={renameColumn}
      onMove={(attr, by) => setLayout({ ...layout, order: moveTo(order, attr, order.indexOf(attr) + by) })}
      onReset={() => {
        setLayout({});
        if (ownHidden.length > 0) showColumns(ownHidden);
      }}
    >
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
    </ArrangePanel>
  );
}
