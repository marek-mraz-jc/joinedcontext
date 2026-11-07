/**
 * The region's districts coloured by one indicator, on the Portal's basemap (T-2933, AP-67,
 * AP-123).
 *
 * The fill runs from the design tokens' `map.low` to `map.high` over the indicator's range, and a
 * district without a number keeps the map's point colour, which the legend names; no colour comes
 * from an entity. The map answers a pointer; the chart beside it is the same choice by keyboard and
 * the list a screen reader reads, so the map is never the only way to a district.
 */
import { GeoView, mapColors } from "@joinedcontext/sdk";
import type { GeoFeature } from "@joinedcontext/sdk";
import type { Strings } from "./locales";

export function DistrictMap({
  features,
  ramp,
  title,
  unit,
  marked,
  onMark,
  basemap,
  s,
}: {
  features: GeoFeature[];
  ramp: [number, number] | null;
  title: string;
  unit: string;
  marked: string | null;
  onMark: (territory: string) => void;
  basemap?: string;
  s: Strings;
}) {
  const colors = mapColors();
  const format = new Intl.NumberFormat(s.locale, { maximumFractionDigits: 2 });
  return (
    <figure className="district-map">
      <GeoView
        value={features}
        ramp={ramp ?? undefined}
        selectedId={marked}
        onSelect={onMark}
        basemap={basemap}
        label={`${s.map}: ${title}`}
      />
      <figcaption className="legend">
        {ramp && (
          <>
            <span className="legend-value">
              {format.format(ramp[0])} {unit}
            </span>
            <span
              className="legend-ramp"
              aria-hidden="true"
              style={{ background: `linear-gradient(to right, ${colors.low}, ${colors.high})` }}
            />
            <span className="legend-value">
              {format.format(ramp[1])} {unit}
            </span>
          </>
        )}
        <span className="legend-none">
          <span className="legend-swatch" aria-hidden="true" style={{ background: colors.point }} />
          {s.notMeasured}
        </span>
      </figcaption>
    </figure>
  );
}
