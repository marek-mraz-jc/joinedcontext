/**
 * One indicator's districts as bars, largest first (T-2922, AP-123).
 *
 * Plain HTML rather than a drawing: the bars are a list a screen reader reads as "district, value,
 * unit", and the labels stay at reading size at 375 px, where a scaled drawing shrinks its text
 * with it. The width of a bar is computed from the data, and so, beside a map, is its colour: the
 * map's scale over `ramp` from the design tokens (T-3005), since the body's own colour is the one
 * the map's legend names "not measured". Without a map the bars keep the body's colour. No colour
 * is a value an entity carries.
 */
import type { CSSProperties } from "react";
import { mapColors } from "@joinedcontext/sdk";
import { rampColor } from "./districts";
import type { Strings } from "./locales";

export interface Bar {
  territory: string;
  value: number;
}

export function DistrictChart({
  id,
  title,
  unit,
  bars,
  s,
  marked = null,
  onMark,
  ramp = null,
}: {
  id: string;
  title: string;
  unit: string;
  bars: Bar[];
  s: Strings;
  /** The district picked on the map, drawn marked. */
  marked?: string | null;
  /** A district's name is a button: it opens the district's indicator, and marks it on a map. */
  onMark: (territory: string) => void;
  /** The range the map's colour scale spans for these bars; `null` where no map is drawn. */
  ramp?: [number, number] | null;
}) {
  const colors = mapColors();
  const max = Math.max(0, ...bars.map((bar) => bar.value));
  const format = new Intl.NumberFormat(s.locale, { maximumFractionDigits: 2 });
  const captionId = `chart-${id}`;
  return (
    <figure className="district-chart" aria-labelledby={captionId}>
      <figcaption id={captionId}>
        {s.districtsCompared}: {title}
      </figcaption>
      <ol className="bars">
        {bars.map((bar) => {
          const name = s.territory[bar.territory] ?? bar.territory;
          const share = max > 0 ? Math.max(0, bar.value / max) : 0;
          return (
            <li key={bar.territory} className={bar.territory === marked ? "bar-row marked" : "bar-row"}>
              <button type="button" className="bar-label" aria-pressed={bar.territory === marked} onClick={() => onMark(bar.territory)}>
                {name}
              </button>
              <span className="bar-track" aria-hidden="true">
                <span
                  className="bar"
                  style={
                    {
                      "--share": share,
                      ...(ramp && { "--bar": rampColor(bar.value, ramp, colors) }),
                    } as CSSProperties
                  }
                />
              </span>
              <span className="bar-value">
                {format.format(bar.value)} {unit}
              </span>
            </li>
          );
        })}
      </ol>
    </figure>
  );
}
