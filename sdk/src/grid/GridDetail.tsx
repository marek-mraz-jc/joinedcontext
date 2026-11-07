import React, { useCallback, useState } from "react";
import { AttributeMetadata } from "./AttributeMetadata";
import { CellInspector } from "./CellInspector";
import { EntityHistory } from "./EntityHistory";
import { GridPending } from "./GridReview";
import { noRow } from "./useGridAccess";
import { RowDetail } from "./RowDetail";
import type { RichRow } from "./model";
import type { UseEntityGridOptions } from "./useEntityGrid";
import type { GridView } from "./useGridView";

/** The open row by id, kept by the host or the grid, read by its id when no loaded page holds it (T-4828). */
export function useGridDetail(
  options: UseEntityGridOptions,
  { openRow, onOpenRow, inShell, select, rows, loading }: { openRow?: string | null; onOpenRow?: (id: string | null) => void; inShell: boolean; select: (entity: { id: string; type: string }) => void; rows: RichRow[]; loading: boolean },
) {
  // The row whose detail panel is open, by id, so a new page closes it rather than showing a row
  // that is no longer listed (T-3097).
  const [ownDetailId, setOwnDetailId] = useState<string | null>(null);
  const detailId = openRow === undefined ? ownDetailId : openRow;
  const setDetailId = useCallback(
    (id: string | null) => {
      setOwnDetailId(id);
      onOpenRow?.(id);
    },
    [onOpenRow],
  );
  // A cell clicked opens its row on that attribute: its readings over time, metadata and versions
  // (owner 2026-11-01); the "i" button keeps the access panel.
  const [cellPlace, setCellPlace] = useState<{ tab: string; attr: string } | null>(null);
  // Inside an App's shell a row opens in the shell's one entity panel (SDK-40), else in the grid's own.
  const openDetail = useCallback(
    (row: RichRow) => (inShell ? select({ id: row.id, type: row.type }) : setDetailId(row.id)),
    [inShell, select, setDetailId],
  );
  const listedRow = detailId === null ? undefined : rows.find((row) => row.id === detailId);
  // A deep link to a row the page has not loaded: read by its id, through the same source and Policy.
  const [linked, setLinked] = useState<RichRow | null>(null);
  const getRow = options.source.get;
  React.useEffect(() => {
    if (detailId === null || listedRow || loading) return;
    let live = true;
    // Gone, or not readable by this person: the same answer either way, and the link closes.
    const missing = () => live && setDetailId(null);
    getRow(detailId).then(
      (row) => (row ? live && setLinked(row) : missing()),
      missing,
    );
    return () => {
      live = false;
    };
  }, [detailId, listedRow, loading, getRow, setDetailId]);
  const detailRow = listedRow ?? (linked && linked.id === detailId ? linked : undefined);
  return { detailId, setDetailId, cellPlace, setCellPlace, openDetail, detailRow };
}

/** The open row as `RowDetail`, wired to the grid: its cells, history, metadata and changes. */
export function GridDetail({ view }: { view: GridView }): React.JSX.Element | null {
  const { props, options, labels, detailRow, rowName, grid, detailValue, historyOn, cellPlace, where, changesFeed, setDetailId, setCellPlace, tableRef } = view;
  const { columns } = grid;
  const { detailTabs, detailPlace, onDetailPlace, detailExtra } = props;
  if (!detailRow) return null;
  return (
    <RowDetail
      row={detailRow}
      name={rowName(detailRow)}
      columns={columns}
      renderValue={detailValue}
      labels={{
        rowDetail: labels.rowDetail,
        close: labels.close,
        rowFields: labels.rowFields,
        rowHistory: labels.history,
        historyAttr: labels.attribute,
        rowMetadata: labels.metadata,
        copyId: labels.rowCopyId,
        copied: labels.copied,
      }}
      history={
        historyOn
          ? (attr, label) => (
              <EntityHistory
                source={options.source}
                id={detailRow.id}
                attr={attr}
                heading={`${options.labels?.historyLabels?.title ?? labels.history}: ${label}`}
                maxPoints={options.config.history.maxPoints}
                labels={options.labels?.historyLabels}
                versions
                versionsFirst={cellPlace !== null}
              />
            )
          : undefined
      }
      // Created and changed come with the row; who wrote it needs the history the config allows.
      metadata={(attr) => (
        <AttributeMetadata
          source={historyOn ? options.source : {}}
          id={detailRow.id}
          attr={attr}
          cell={detailRow.cells[attr]}
          locale={where}
          labels={options.labels?.metadataLabels}
        />
      )}
      changes={changesFeed}
      tabs={detailTabs?.(detailRow)}
      place={cellPlace ?? detailPlace}
      onPlace={onDetailPlace}
      onClose={() => {
        setDetailId(null);
        setCellPlace(null);
        tableRef.current?.focus();
      }}
    >
      <GridPending view={view} inDialog />
      {detailExtra?.(detailRow)}
    </RowDetail>
  );
}

/** What opens from one cell: the access inspector, and an attribute's history under the grid. */
export function GridCellPanels({ view }: { view: GridView }): React.JSX.Element {
  const { props, options, inspected, setInspected, accessFailed, accessOf, accessRows, inspectorLabels, tableRef, history, setHistory, historyBox } = view;
  const { explainAccess, accessRequestHref } = props;
  return (
    <>
      {inspected && (
        <CellInspector
          attribute={inspected.attr}
          label={inspected.label}
          access={accessFailed === undefined ? accessOf(inspected.attr) ?? noRow(inspected.attr, accessRows) : undefined}
          failed={accessFailed}
          explain={explainAccess}
          askAccessHref={accessRequestHref}
          labels={inspectorLabels}
          onClose={() => {
            setInspected(null);
            // Back to the grid, whose active cell is still the one inspected.
            tableRef.current?.focus();
          }}
        />
      )}

      {history && (
        <div ref={historyBox}>
        <EntityHistory
          source={options.source}
          id={history.id}
          attr={history.attr}
          heading={history.heading}
          maxPoints={options.config.history.maxPoints}
          labels={options.labels?.historyLabels}
          onClose={() => setHistory(null)}
        />
        </div>
      )}
    </>
  );
}
