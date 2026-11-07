import React, { useCallback } from "react";
import { CellView, cellDisplayText, formatDate, kindOfCell, IdLabel, targetName, webHref } from "./cells";
import type { CellLabels, ColumnKind, ResolvedTargets } from "./cells";
import { EditableCell, EnumCell, coerce, inputKindOf } from "./GridCellEditors";
import { KIND_LABEL } from "./gridKinds";
import { cellText } from "./model";
import type { RichCell, RichRow } from "./model";
import { optionLabel } from "../enums";
import { objectsOf } from "../relations";
import type { TargetOption } from "../relations";
import { NGSI_LD_NULL, RelationPicker } from "./RelationPicker";
import type { ValueRule } from "./rules";
import type { EntityGrid as EntityGridApi, UseEntityGridOptions, VisibleColumn } from "./useEntityGrid";

const ISO_TIMESTAMP = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}/;

/** Whether a pending value is a geometry, so a geo cell stays a geo cell while it is changed. */
function isGeometry(value: unknown): boolean {
  return (
    typeof value === "object" &&
    value !== null &&
    typeof (value as { type?: unknown }).type === "string" &&
    (value as { coordinates?: unknown }).coordinates !== undefined
  );
}

/** The kind a model range asks a cell for, where a value of another kind is plainly wrong. */
function expectedKind(range: string | undefined): ColumnKind | undefined {
  if (range === "integer" || range === "number") return "number";
  if (range === "boolean") return "boolean";
  if (range === "datetime" || range === "date") return "datetime";
  return undefined;
}

/** What a cell's editor needs of the grid: the pending values, what is refused, and how targets open. */
interface CellContext {
  gridOptions: UseEntityGridOptions;
  labels: EntityGridApi["labels"];
  edits: EntityGridApi["state"]["edits"];
  setEdit: EntityGridApi["setEdit"];
  rules?: Record<string, ValueRule>;
  canOpen: boolean;
  openTarget: (urn: string, type?: string) => void;
  inverse: Record<string, { byId: Record<string, TargetOption[]>; full: boolean } | null>;
  searchOf: (target: string) => (text: string) => Promise<TargetOption[]>;
  pickerLabels: React.ComponentProps<typeof RelationPicker>["labels"];
  refusedCell: Map<string, string>;
  ruleProblems: Map<string, string>;
}

type End = NonNullable<UseEntityGridOptions["relations"]>[string];

/** A computed end: the entities pointing back, as links, never an input (DM-67). */
function inverseContent({ labels, inverse, canOpen, openTarget }: CellContext, row: RichRow, attr: string, end: End): React.ReactNode {
  const read = inverse[attr];
  if (read === undefined) return <>{labels.loading}</>;
  if (read === null) return <>{labels.error}</>;
  const pointing = read.byId[row.id] ?? [];
  return (
    <span className="jc-grid-rel-list">
      {pointing.map((one) =>
        canOpen ? (
          <button key={one.id} type="button" className="jc-grid-rel-btn" title={one.id} onClick={(e) => { e.stopPropagation(); openTarget(one.id, end.target); }}>
            {one.name ?? one.id}
          </button>
        ) : (
          <span key={one.id} title={one.id}>{one.name ?? one.id}</span>
        ),
      )}
      {read.full && <span className="jc-grid-rel-more">{labels.relationMore}</span>}
    </span>
  );
}

/** A stored end in edit mode: a picker of the target's entities (UI-84). */
function relationEditor({ labels, edits, setEdit, searchOf, pickerLabels, refusedCell }: CellContext, row: RichRow, column: VisibleColumn & { attr: string }, cell: RichCell | RichCell[] | undefined, end: End): React.ReactNode {
  const attr = column.attr;
  const pending = edits[row.id]?.[attr];
  const stored = objectsOf(cell);
  const now = pending === undefined ? stored : pending === NGSI_LD_NULL ? [] : Array.isArray(pending) ? (pending as string[]) : [String(pending)];
  return (
    <RelationPicker
      label={`${labels.edit} ${column.label}`}
      value={now}
      end={end}
      search={searchOf(end.target)}
      labels={pickerLabels}
      changed={pending !== undefined}
      invalid={refusedCell.get(`${row.id}\u0000${attr}`)}
      onChange={(next) => {
        const same = next.length === stored.length && next.every((id, at) => id === stored[at]);
        // An emptied end is written as NGSI-LD's null, so the attribute goes.
        const after = next.length === 0 ? NGSI_LD_NULL : end.many ? next : next[0];
        setEdit(row.id, attr, same ? undefined : after);
      }}
    />
  );
}

/** An attribute's own editor, by what it holds: a geometry says so, an enum lists, the rest is typed. */
function valueEditor({ gridOptions, labels, edits, setEdit, rules, refusedCell, ruleProblems }: CellContext, row: RichRow, column: VisibleColumn & { attr: string }, cell: RichCell | RichCell[] | undefined): React.ReactNode {
  const pending = edits[row.id]?.[column.attr];
  const one = Array.isArray(cell) ? cell[0] : cell;
  // A geometry is not typed over. Its editor is the map panel, and a text box here would put
  // `[object Object]` in front of a person and write that string back as the value (T-1443).
  // The cell still says what it holds and marks a pending change, so the table stays the
  // record of what will be sent.
  if (one?.kind === "geo" || (pending !== undefined && isGeometry(pending))) {
    const geometry = (pending ?? one?.value) as { type?: unknown } | undefined;
    const kind = typeof geometry?.type === "string" ? geometry.type : labels.empty;
    return (
      <span className={`jc-grid-geo-cell${pending !== undefined ? " jc-grid-cell--changed" : ""}`}>
        {kind}
        {pending !== undefined ? " *" : ""}
      </span>
    );
  }
  // The value itself, not the text the cell shows: a person edits `5`, not `5 C62` — the unit
  // is what the value is measured in and is kept, never typed over.
  const own = one?.kind === "relationship" ? one.object : one?.value;
  const shown = own === undefined || own === null ? "" : String(own);
  const rule = rules?.[column.attr];
  const options =
    one?.kind === "relationship"
      ? undefined
      : (gridOptions.enums?.[column.attr] ??
        (rule?.kind === "boolean"
          ? [
              { value: "true", title: labels.yes },
              { value: "false", title: labels.no },
            ]
          : undefined));
  if (options && options.length > 0) {
    return (
      <EnumCell
        label={`${labels.edit} ${column.label}`}
        value={pending === undefined ? shown : String(pending)}
        options={options}
        changed={pending !== undefined}
        notInList={labels.notInList}
        onChange={(next) => setEdit(row.id, column.attr, next === shown ? undefined : coerce(next, shown, rule))}
      />
    );
  }
  const key = `${row.id}\u0000${column.attr}` as const;
  return (
    <EditableCell
      label={`${labels.edit} ${column.label}`}
      value={pending === undefined ? shown : String(pending)}
      changed={pending !== undefined}
      invalid={ruleProblems.get(key) ?? refusedCell.get(key)}
      kind={inputKindOf(rule, own)}
      onChange={(next) => {
        // Back to the endpoint's own value is not a change: it leaves the pending list.
        setEdit(row.id, column.attr, next === shown ? undefined : coerce(next, shown, rule));
      }}
    />
  );
}

/** How one cell reads and opens: the grid's body and the row's detail draw every value through this. */
export function useCellContent(
  gridOptions: UseEntityGridOptions,
  grid: EntityGridApi,
  {
    where,
    cellLabels,
    renderers,
    termHref,
    rules,
    editable,
    isPrimary,
    rowName,
    openDetail,
    relations,
    inverse,
    targets,
    canOpen,
    openTarget,
    searchOf,
    pickerLabels,
    refusedCell,
    ruleProblems,
  }: Pick<CellContext, "rules" | "inverse" | "canOpen" | "openTarget" | "searchOf" | "pickerLabels" | "refusedCell" | "ruleProblems"> & {
    where: string;
    cellLabels: CellLabels;
    renderers?: Record<string, (cell: RichCell | RichCell[] | undefined, row: RichRow) => React.ReactNode>;
    termHref?: (meaning: string) => string | undefined;
    editable: (column: VisibleColumn) => boolean;
    isPrimary: (column: VisibleColumn) => boolean;
    rowName: (row: RichRow) => string;
    openDetail: (row: RichRow) => void;
    relations: UseEntityGridOptions["relations"];
    targets: ResolvedTargets;
  },
) {
  const { rows, labels, state, cellOf, setEdit } = grid;
  // A value as it reads, without an editor: the grid's cells and a row opened by its id alone (T-5600).
  const readValue = useCallback((row: RichRow, column: VisibleColumn): React.ReactNode => {
    const { text, cell } = cellOf(row, column);
    // An enum's value reads as its label, the one its filter and its editor already offer (T-4200).
    const options = column.attr ? gridOptions.enums?.[column.attr] : undefined;
    const one = Array.isArray(cell) ? undefined : cell;
    const option = options && one?.kind !== "relationship" ? options.find((o) => o.value === String(one?.value)) : undefined;
    if (option) {
      // The model's term, where the host says where terms read and it is a web address (G-R2GRID2-019).
      const href = option.meaning && termHref ? webHref(termHref(option.meaning)) : undefined;
      if (!href) return <>{optionLabel(option)}</>;
      return (
        <a className="jc-cell jc-cell--link" href={href} target="_blank" rel="noopener noreferrer" title={option.meaning} onClick={(e) => e.stopPropagation()}>
          {optionLabel(option)}
        </a>
      );
    }
    if (column.attr !== null && column.meta === null) {
      // Absent or null: an empty cell, which a screen reader hears as not set (G-R2GRID2-020).
      if (cell === undefined) {
        return (
          <span className="jc-cell jc-cell--empty">
            <span className="jc-grid-sr-only">{`${column.label}: ${labels.rowNotSet}`}</span>
          </span>
        );
      }
      // A value the model's list does not hold: as stored, said so, never a label it does not have.
      if (options && options.length > 0 && one && one.kind !== "relationship" && one.value !== undefined && one.value !== null) {
        return <span className="jc-cell jc-cell--invalid">{`${String(one.value)} (${labels.notInList})`}</span>;
      }
      // A value of another kind than the model's range: the raw text and why, never coerced.
      const expected = expectedKind(rules?.[column.attr]?.kind);
      if (expected && one?.kind === "property" && kindOfCell(one) !== expected) {
        const reason = labels.wrongKind.replace("{kind}", labels[KIND_LABEL[expected]] as string);
        return (
          <span className="jc-cell jc-cell--invalid" title={reason}>{`${cellText(one)} (${reason})`}</span>
        );
      }
      // The value by what it holds: a number with its unit, a local date, a point, a chip (T-4827).
      return <CellView cell={cell} locale={where} labels={cellLabels} onOpen={canOpen ? openTarget : undefined} targets={targets} />;
    }
    // A timestamp column (observed, created, modified) in the reader's own time.
    return <>{ISO_TIMESTAMP.test(text) ? formatDate(text, where) : text}</>;
  }, [where, cellLabels, cellOf, canOpen, openTarget, targets, termHref, rules, gridOptions.enums, labels]);
  const renderCellContent = useCallback((row: RichRow, column: VisibleColumn): React.ReactNode => {
    const { text, cell } = cellOf(row, column);

    // Custom renderer by attribute name first, then by cell kind
    if (column.attr && renderers?.[column.attr]) {
      return renderers[column.attr](cell, row);
    }
    // The identifier column carries no attribute, so a host addresses it by its key.
    if (column.attr === null && renderers?.[column.key]) {
      return renderers[column.key](cell, row);
    }
    // The primary field, or the identifier where the config names none, opens the row's detail
    // panel (T-3097): the person reads the row by its name and opens it from there. Inside an
    // App's shell that is the shell's one entity panel (SDK-40, T-3396), else the grid's own.
    if (column.key === "id" || isPrimary(column)) {
      // A row named by nothing but its id is called by the id's last part, as its detail heading
      // and its chips are: every id of a type starts `urn:ngsi-ld:Type:…` (T-4265, T-5619).
      const named = rowName(row);
      const name = named === row.id ? targetName(row.id).name : named;
      return (
        <button
          type="button"
          className="jc-grid-open"
          aria-label={`${labels.openRow}: ${name}`}
          title={row.id}
          onClick={(e) => {
            e.stopPropagation();
            openDetail(row);
          }}
        >
          {column.key === "id" ? <IdLabel urn={text} /> : text || row.id}
        </button>
      );
    }
    if (cell && !Array.isArray(cell) && renderers?.[cell.kind]) {
      return renderers[cell.kind](cell, row);
    }

    const context: CellContext = { gridOptions, labels, edits: state.edits, setEdit, rules, canOpen, openTarget, inverse, searchOf, pickerLabels, refusedCell, ruleProblems };
    const end = column.attr && column.meta === null ? relations?.[column.attr] : undefined;
    if (end?.inverseOf !== undefined && column.attr) return inverseContent(context, row, column.attr, end);
    if (end && column.attr && editable(column)) return relationEditor(context, row, { ...column, attr: column.attr }, cell, end);

    // A relationship is a chip naming its target, which opens it (T-4827).
    if (column.attr && column.meta === null && cell && !Array.isArray(cell) && cell.kind === "relationship" && canOpen) {
      return <CellView cell={cell} locale={where} labels={cellLabels} onOpen={openTarget} targets={targets} />;
    }

    if (column.attr && editable(column)) return valueEditor(context, row, { ...column, attr: column.attr }, cell);

    return readValue(row, column);
  }, [readValue, where, cellLabels, cellOf, renderers, canOpen, openTarget, targets, editable, state.edits, labels.edit, labels.empty, labels.notInList, labels.loading, labels.error, labels.relationMore, labels.openRow, isPrimary, rowName, openDetail, gridOptions.enums, setEdit, relations, inverse, searchOf, pickerLabels, refusedCell, ruleProblems, rules, labels.yes, labels.no]);

  // One attribute in the row's detail (T-5599): an editor where the row is on the page and the
  // grant writes it, else the value as a reader sees it, an enum by its label, and an attribute
  // with no value says "not set" instead of nothing. An edit is written against the row as the page
  // loaded it, so a row read on its own shows its values without editors.
  const detailValue = useCallback(
    (row: RichRow, column: VisibleColumn): React.ReactNode => {
      const listed = rows.some((one) => one.id === row.id);
      if (listed && editable(column)) return renderCellContent(row, column);
      const cell = column.attr ? row.cells[column.attr] : undefined;
      if (cellDisplayText(cell, where, cellLabels).trim() === "") return <span className="jc-grid-detail-empty">{labels.rowNotSet}</span>;
      // Every instance by its datasetId, where the cell shows one with "+N" (G-R2GRID2-020); an edit
      // still reaches only the shown one, through the cell.
      if (Array.isArray(cell) && cell.length > 1 && !cell.every((one) => one.kind === "relationship")) {
        return (
          <ul className="jc-grid-instances">
            {cell.map((one, at) => (
              <li key={`${one.datasetId ?? ""}-${at}`}>
                <span className="jc-grid-instance-id" title={one.datasetId}>{one.datasetId ? targetName(one.datasetId).name : labels.defaultInstance}</span>{" "}
                <CellView cell={one} locale={where} labels={cellLabels} onOpen={canOpen ? openTarget : undefined} targets={targets} />
              </li>
            ))}
          </ul>
        );
      }
      return listed ? renderCellContent(row, column) : readValue(row, column);
    },
    [rows, editable, renderCellContent, readValue, where, cellLabels, labels.rowNotSet, labels.defaultInstance, canOpen, openTarget, targets],
  );
  return { readValue, renderCellContent, detailValue };
}
