/**
 * How the table draws a record's cells (T-2966): the indicator and the territory by the statistics
 * office's names with the code beside them, the number with the publisher's own unit text, the day
 * as a date, and the URN behind a disclosure. The codes stay on screen because a column filter is
 * the endpoint's own `q`, which asks about the codes.
 */
import type { ReactNode } from "react";
import type { RichCell, RichRow } from "@joinedcontext/sdk";
import { AREAS, CODES } from "./labels";
import { named } from "./series";

type Renderer = (cell: RichCell | RichCell[] | undefined, row: RichRow) => ReactNode;

function valueOf(cell: RichCell | RichCell[] | undefined): unknown {
  return Array.isArray(cell) ? cell[0]?.value : cell?.value;
}

function text(row: RichRow, attr: string): string | undefined {
  const held = row.raw[attr];
  const value = typeof held === "object" && held !== null && "value" in held ? (held as { value: unknown }).value : held;
  return typeof value === "string" ? value : undefined;
}

function coded(name: string, code: string): ReactNode {
  return name === code ? (
    <span>{code}</span>
  ) : (
    <span className="coded">
      <span>{name}</span> <span className="code">{code}</span>
    </span>
  );
}

export function recordRenderers(
  language: "sk" | "en",
  words: { locale: string; showId: string; openRecord: string },
  open: (row: RichRow) => void,
): Record<string, Renderer> {
  const number = new Intl.NumberFormat(words.locale, { maximumFractionDigits: 2 });
  const date = new Intl.DateTimeFormat(words.locale, { dateStyle: "medium", timeZone: "UTC" });
  return {
    indicator: (cell, row) => {
      const code = String(valueOf(cell) ?? "");
      return coded(named(CODES[text(row, "dataSet") ?? ""]?.[code], language, code), code);
    },
    refArea: (cell) => {
      const code = String(valueOf(cell) ?? "");
      return coded(named(AREAS[code], language, code), code);
    },
    kpiValue: (cell, row) => {
      const value = valueOf(cell);
      if (typeof value !== "number") return <span>{String(value ?? "")}</span>;
      const unit = text(row, "unitText");
      return (
        <span className="figure">
          {number.format(value)}
          {unit ? <span className="unit"> {unit}</span> : null}
        </span>
      );
    },
    dateObserved: (cell) => {
      const value = valueOf(cell);
      const at = typeof value === "string" ? new Date(value) : null;
      return at && !Number.isNaN(at.getTime()) ? <time dateTime={value as string}>{date.format(at)}</time> : <span>{String(value ?? "")}</span>;
    },
    // The record opens in the shell's entity panel (SDK-40); its URN stays behind a disclosure.
    id: (_cell, row) => (
      <>
        <button type="button" className="row-open" onClick={() => open(row)}>
          {words.openRecord}
        </button>
        <details className="urn">
          <summary>{words.showId}</summary>
          <code>{row.id}</code>
        </details>
      </>
    ),
  };
}
