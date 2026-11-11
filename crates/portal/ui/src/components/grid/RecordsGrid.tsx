/**
 * Records the page already holds (a pipeline sample, an import preview, an answer's rows) on the
 * shared grid (T-6752): one column per field, searched, filtered and sorted by the text its cell
 * shows. The rows are what the read returned; the grid only narrows them.
 */
import type { JSX, ReactNode } from "react";
import clsx from "clsx";
import { AdminGrid } from "./AdminGrid";
import type { DataColumn } from "./AdminGrid";

/** A record with its place in what the read returned, which keys its row. */
export interface RecordRow<R> {
  at: number;
  record: R;
}

export function RecordsGrid<R>({
  caption,
  fields,
  records,
  text,
  label,
  lead = [],
  mono,
  storageKey,
  empty,
  stack,
}: {
  caption: string;
  fields: readonly string[];
  records: readonly R[];
  /** A field's value as the cell shows it. */
  text: (record: R, field: string) => string;
  /** A field's header, the field's own name when absent. */
  label?: (field: string) => string;
  /** Columns before the fields, such as the file's line number or the entity's id. */
  lead?: readonly DataColumn<RecordRow<R>>[];
  /** Values set in the code face, as a pipeline's raw fields are. */
  mono?: boolean;
  storageKey: string;
  empty: ReactNode;
  stack?: boolean;
}): JSX.Element {
  const rows = records.map((record, at) => ({ at, record }));
  const columns: DataColumn<RecordRow<R>>[] = [
    ...lead,
    ...fields.map((field) => ({
      key: `field:${field}`,
      label: label ? label(field) : field,
      text: (row: RecordRow<R>) => text(row.record, field),
      cell: (row: RecordRow<R>) => {
        const value = text(row.record, field);
        // A cell takes no max-width, so the fold is on a block inside it: a 4 000-character
        // value would push every other column off the screen (T-2460).
        return (
          <span className={clsx("block max-w-64 truncate", mono && "font-mono text-caption")} title={value}>
            {value}
          </span>
        );
      },
    })),
  ];
  return (
    <AdminGrid
      caption={caption}
      rows={rows}
      columns={columns}
      rowKey={(row) => String(row.at)}
      empty={empty}
      storageKey={storageKey}
      stack={stack}
    />
  );
}
