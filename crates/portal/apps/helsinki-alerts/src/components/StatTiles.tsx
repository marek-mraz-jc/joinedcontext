import { aggregate } from "@joinedcontext/sdk";
import type { Agg, Row } from "@joinedcontext/sdk";

export interface StatTile {
  label: string;
  agg?: Agg;
  attr?: string;
  unit?: string;
  digits?: number;
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
        let value: string;
        if (loading) {
          value = "…";
        } else {
          const raw = aggregate(rows, tile.agg ?? "count", tile.attr);
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
          </div>
        );
      })}
    </section>
  );
}
