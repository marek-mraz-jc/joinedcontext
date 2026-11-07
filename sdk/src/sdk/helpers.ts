import type { Geo, Row } from "../ngsi";
import { aggregate, columnKind, displayName, distinct, extent, format, pointOf } from "../ngsi";
import type { Agg } from "../spec";

export { aggregate, columnKind, displayName, distinct, extent, format, pointOf };
export type { Agg };

export interface Group {
  key: string;
  value: number;
}

function isGeo(val: unknown): val is Geo {
  return (
    typeof val === "object" &&
    val !== null &&
    "type" in val &&
    "coordinates" in val &&
    typeof (val as { type: unknown }).type === "string"
  );
}

export function groupBy(
  rows: Row[],
  attr: string,
  agg: Agg = "count",
  valueAttr?: string,
  top?: number,
): Group[] {
  const map = new Map<string, Row[]>();
  for (const row of rows) {
    const raw = format(row[attr]);
    const key = raw.trim() === "" ? "(none)" : raw;
    let group = map.get(key);
    if (!group) {
      group = [];
      map.set(key, group);
    }
    group.push(row);
  }

  const groups: Group[] = [];
  for (const [key, groupRows] of map.entries()) {
    const value = aggregate(groupRows, agg, valueAttr);
    if (value !== null) {
      groups.push({ key, value });
    }
  }

  groups.sort((a, b) => {
    if (b.value !== a.value) {
      return b.value - a.value;
    }
    return a.key.localeCompare(b.key);
  });

  if (top !== undefined && top > 0) {
    return groups.slice(0, top);
  }
  return groups;
}

export function toFeatureCollection(
  rows: Row[],
  location = "location",
  properties?: string[],
): {
  type: "FeatureCollection";
  features: Array<{
    type: "Feature";
    id: string;
    geometry: Geo;
    properties: Record<string, string | number | boolean | null>;
  }>;
} {
  const features: Array<{
    type: "Feature";
    id: string;
    geometry: Geo;
    properties: Record<string, string | number | boolean | null>;
  }> = [];

  for (const row of rows) {
    const geo = row[location];
    if (!isGeo(geo)) {
      continue;
    }

    const props: Record<string, string | number | boolean | null> = {
      id: row.id,
      type: row.type,
    };

    if (properties !== undefined) {
      for (const attr of properties) {
        if (attr === "id" || attr === "type") continue;
        const val = row[attr];
        if (!isGeo(val)) {
          if (typeof val === "string" || typeof val === "number" || typeof val === "boolean" || val === null) {
            props[attr] = val;
          } else if (val === undefined) {
            props[attr] = null;
          }
        }
      }
    } else {
      for (const [key, val] of Object.entries(row)) {
        if (key === "id" || key === "type" || key === "@context") continue;
        if (!isGeo(val)) {
          if (typeof val === "string" || typeof val === "number" || typeof val === "boolean" || val === null) {
            props[key] = val;
          }
        }
      }
    }

    features.push({
      type: "Feature",
      id: row.id,
      geometry: geo,
      properties: props,
    });
  }

  return {
    type: "FeatureCollection",
    features,
  };
}
