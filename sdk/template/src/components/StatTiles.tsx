import { aggregate } from "@joinedcontext/sdk";
import type { Agg, Row } from "@joinedcontext/sdk";
import * as i18n from "../i18n";

export interface StatTile {
  label: string;
  agg?: Agg;
  attr?: string;
  unit?: string;
  digits?: number;
  /** Counts the rows whose `attr` holds exactly this: "empty" is `equals: 0`. */
  equals?: number;
}

/** A row without `attr` is unknown, never 0: it counts in no tile, and the tile says how many. */
function reporting(rows: Row[], attr?: string): Row[] {
  return attr ? rows.filter((row) => row[attr] !== undefined && row[attr] !== null) : rows;
}

function figure(known: Row[], tile: StatTile): number | null {
  if (tile.equals === undefined) return aggregate(known, tile.agg ?? "count", tile.attr);
  return known.filter((row) => row[tile.attr ?? ""] === tile.equals).length;
}

export function StatTiles({
  rows,
  tiles,
  loading,
}: {
  rows: Row[];
  tiles: StatTile[];
  loading?: boolean;
}): React.JSX.Element {
  return (
    <section className="jc-tiles">
      {tiles.map((tile, idx) => {
        const known = reporting(rows, tile.attr);
        const unknown = rows.length - known.length;
        let value: string;
        if (loading) {
          value = "…";
        } else {
          const raw = figure(known, tile);
          if (raw === null) {
            value = "–";
          } else {
            value = new Intl.NumberFormat(undefined, {
              maximumFractionDigits: tile.digits ?? 1,
            }).format(raw);
          }
        }
        return (
          <div className="jc-tile" key={tile.label || String(idx)}>
            <span className="jc-tile-label">{tile.label}</span>
            <strong className="jc-tile-value">
              {value}
              {tile.unit && <small> {tile.unit}</small>}
            </strong>
            {!loading && unknown > 0 ? (
              <span className="jc-tile-unknown">{i18n.t("tiles.unknown", { count: String(unknown) })}</span>
            ) : null}
          </div>
        );
      })}
    </section>
  );
}
