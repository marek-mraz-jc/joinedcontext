/**
 * The region's districts as map shapes, joined to an indicator's bars (T-2933, AP-04, AP-123).
 *
 * The outlines are AdministrativeArea entities of the register space `bbsk-registre`, read through
 * its public endpoint; an indicator carries no location (PF-54) and names its district only in its
 * territory token, `okres-{name}` (`Development/10` §6). The join is that token, derived from the
 * district's Slovak name, so a district the register spells differently stays unjoined and uncoloured
 * rather than coloured with another district's number.
 */
import type { GeoFeature, RichCell, RichRow } from "@joinedcontext/sdk";

export interface District {
  territory: string;
  geometry: GeoFeature["geometry"];
}

/** "Okres Banská Bystrica" → `okres-banska-bystrica`; `null` for a name with nothing left. */
export function territoryOf(name: string): string | null {
  const slug = name
    .normalize("NFD")
    .replace(/\p{M}/gu, "")
    .toLowerCase()
    .replace(/^\s*okres\s+/, "")
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
  return slug ? `okres-${slug}` : null;
}

function one(cells: Record<string, RichCell | RichCell[]>, attr: string): RichCell | undefined {
  const cell = cells[attr];
  return Array.isArray(cell) ? cell[0] : cell;
}

const SHAPES = new Set(["Polygon", "MultiPolygon"]);

/**
 * The districts among register rows: a `district` with a Slovak (or its only) name and an area
 * outline. A municipality, a point or a nameless row is no district shape and is left out.
 */
export function districtsOf(rows: RichRow[]): District[] {
  const districts: District[] = [];
  for (const row of rows) {
    if (row.type !== "AdministrativeArea" || one(row.cells, "divisionLevel")?.value !== "district") continue;
    const name = one(row.cells, "name");
    const text = name?.languageMap?.sk ?? name?.value;
    const territory = typeof text === "string" ? territoryOf(text) : null;
    const geometry = one(row.cells, "location")?.value as GeoFeature["geometry"] | undefined;
    if (!territory || !geometry || !SHAPES.has(geometry.type)) continue;
    districts.push({ territory, geometry });
  }
  return districts;
}

/**
 * One feature per district, carrying the indicator's number as `value` where the district has one,
 * and the range the map's colour ramp spans; `null` when no district is measured.
 */
export function choropleth(
  districts: District[],
  bars: { territory: string; value: number }[],
): { features: GeoFeature[]; ramp: [number, number] | null } {
  const byTerritory = new Map(bars.map((bar) => [bar.territory, bar.value]));
  const features: GeoFeature[] = districts.map((district) => {
    const value = byTerritory.get(district.territory);
    return {
      type: "Feature",
      id: district.territory,
      geometry: district.geometry,
      properties: value === undefined ? {} : { value },
    };
  });
  const values = features.flatMap((feature) => {
    const value = feature.properties?.value;
    return typeof value === "number" ? [value] : [];
  });
  return { features, ramp: rangeOf(values) };
}

/** The lowest and highest of `values`; `null` for none. */
export function rangeOf(values: number[]): [number, number] | null {
  return values.length ? [Math.min(...values), Math.max(...values)] : null;
}

/**
 * A value's colour as the map fills its district (T-3005): straight from `low` to `high` in sRGB
 * over `ramp`, one value alone at the low end, so a bar and its district are one colour. The
 * tokens are any CSS colour, so the mix is CSS's own rather than parsed here.
 */
export function rampColor(value: number, ramp: [number, number], colors: { low: string; high: string }): string {
  const t = ramp[1] > ramp[0] ? Math.min(1, Math.max(0, (value - ramp[0]) / (ramp[1] - ramp[0]))) : 0;
  return `color-mix(in srgb, ${colors.low}, ${colors.high} ${Math.round(t * 1000) / 10}%)`;
}
