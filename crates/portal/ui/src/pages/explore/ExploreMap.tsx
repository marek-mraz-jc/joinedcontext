/**
 * The explorer's map (T-3256): every located entity of the view on one map, counted circles at low
 * zoom that open as one zooms in, a popup with the entity's main attributes, and a button to open
 * the entity itself. It reads the same view as the table, the page's filter and the grid's filter
 * row joined, through the person's endpoint with their session, so it shows what they may read.
 */
import { keepPreviousData, useQuery } from "@tanstack/react-query";
import { useCallback, useState } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { fetchEntities } from "../../components/entities/filters";
import type { EntityQuery } from "../../components/entities/filters";
import { MapLibreView } from "../../components/dashboards/MapLibreView";
import { ONE_VALUE_COLOUR, RAMP, unknownColour } from "../../components/dashboards/mapColours";
import { readable } from "../../components/entities/displayOrder";
import type { Bbox } from "../../components/dashboards/MapLibreView";
import { InlineError } from "../../components/ui/InlineError";
import { PageFailed, PageLoading } from "../../components/ui/PageState";
import { collect, geometryOf } from "./exportView";

/** The most entities the map draws at once; past it, the map reads the part in view. */
export const MAP_MAX = 5_000;

type Entity = Record<string, unknown>;

/** The located entities as one feature collection, each carrying its id and its shown attributes. */
export function mapFeatures(entities: Entity[], columns: string[]): { type: "FeatureCollection"; features: unknown[] } {
  const features = entities.flatMap((entity) => {
    const geometry = geometryOf(entity);
    if (!geometry || typeof entity.id !== "string") return [];
    const properties: Record<string, unknown> = { id: entity.id };
    for (const column of columns) {
      const value = entity[column];
      if (value !== undefined && value !== null && typeof value !== "object") properties[column] = value;
    }
    return [{ type: "Feature", geometry, properties }];
  });
  return { type: "FeatureCollection", features };
}

/** A coloured map's range and how many of its points hold no number to colour by. */
export function colourOf(features: unknown[], attr: string): { domain?: [number, number]; unknown: number } {
  const values = (features as { properties: Record<string, unknown> }[])
    .map((feature) => feature.properties[attr])
    .filter((value): value is number => typeof value === "number");
  return {
    domain: values.length > 0 ? [Math.min(...values), Math.max(...values)] : undefined,
    unknown: features.length - values.length,
  };
}

/** Whether a type has a place: a GeoProperty in its model, or the conventional `location`. */
export function isLocated(slots: { name: string; kind: string }[]): boolean {
  return slots.some((slot) => slot.kind === "GeoProperty" || slot.name === "location");
}

/** The coloured map's key. The words for a screen reader stand in the status line above it. */
function ColourKey({ attr, domain: [min, max], unknown }: { attr: string; domain: [number, number]; unknown: number }): JSX.Element {
  const { t, i18n } = useTranslation();
  const number = new Intl.NumberFormat(i18n.language);
  return (
    <div aria-hidden="true" className="flex flex-wrap items-center gap-x-2 gap-y-1 text-caption text-fg-muted">
      <span className="font-medium text-fg">{attr}</span>
      {min === max ? (
        <>
          <svg width="10" height="10">
            <circle cx="5" cy="5" r="5" fill={ONE_VALUE_COLOUR} />
          </svg>
          <span>{t("explore.map.keyAll", { value: number.format(min) })}</span>
        </>
      ) : (
        <>
          <span>{number.format(min)}</span>
          <svg width="128" height="8">
            <linearGradient id="explore-map-ramp">
              {RAMP.map((stop, index) => (
                <stop key={stop} offset={index / (RAMP.length - 1)} stopColor={stop} />
              ))}
            </linearGradient>
            <rect width="128" height="8" rx="4" fill="url(#explore-map-ramp)" />
          </svg>
          <span>{number.format(max)}</span>
        </>
      )}
      {unknown > 0 ? (
        <>
          <svg width="10" height="10" className="ml-2">
            <circle cx="5" cy="5" r="5" fill={unknownColour()} />
          </svg>
          <span>{t("explore.map.keyUnknown", { unknown: number.format(unknown) })}</span>
        </>
      ) : null}
    </div>
  );
}

export function ExploreMap({
  project,
  slug,
  query,
  columns,
  geo,
  colorBy,
  onOpen,
}: {
  project: string;
  slug: string;
  /** The view: the type and both filters joined. */
  query: EntityQuery;
  /** The attributes a popup shows, the main ones first. */
  columns: string[];
  /** The type's geometry attributes, `location` first. */
  geo: string[];
  /** The number attribute the points take their colour from; a point without it stays grey. */
  colorBy?: string;
  onOpen: (id: string) => void;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const shown = [...new Set([...(colorBy ? [colorBy] : []), ...columns.slice(0, 4)])];
  const attrs = [...new Set([...geo, ...shown])];
  const key = [slug, query.type, query.q, query.scopeQ, query.idPattern, attrs.join(",")];
  // keyValues and only what the popup shows, beside the geometry: a map of thousands of points
  // does not need their whole entities.
  const read = (within?: Bbox) =>
    collect(
      async (offset, limit) => {
        const page = await fetchEntities(slug, { ...query, attrs }, { limit, offset, count: offset === 0, within, geoproperty: geo[0] });
        return { rows: page.rows as Entity[], count: page.count };
      },
      { max: MAP_MAX },
    );
  const located = useQuery({
    queryKey: ["explore-map", ...key],
    enabled: Boolean(query.type),
    retry: false,
    queryFn: () => read(),
  });
  // Past MAP_MAX the map reads the part in view after every pan or zoom (T-5061), as the
  // catalogue's map and the dashboards do, so a view of 10 000 entities stays whole where one looks.
  const [view, setView] = useState<Bbox | null>(null);
  const onMoveEnd = useCallback((box: Bbox) => setView(box), []);
  const paged = located.data?.truncated === true;
  const inView = useQuery({
    queryKey: ["explore-map-view", ...key, view?.join(",")],
    enabled: paged && view !== null,
    retry: false,
    placeholderData: keepPreviousData,
    queryFn: () => read(view ?? undefined),
  });

  if (located.isPending) return <PageLoading label={t("explore.map.loading")} lines={4} />;
  if (located.isError) return <PageFailed error={located.error} onRetry={() => void located.refetch()} />;
  const drawn = paged && inView.data ? inView.data : located.data;
  const collection = mapFeatures(drawn.entities, shown);
  const number = new Intl.NumberFormat(i18n.language);
  const total = number.format(located.data.total ?? located.data.entities.length);
  let status: string;
  if (collection.features.length === 0) {
    status = t(paged && inView.data ? "explore.map.noneInView" : "explore.map.none");
  } else {
    status = t(paged && inView.data ? "explore.map.inView" : "explore.map.count", {
      count: collection.features.length,
      shown: number.format(collection.features.length),
      total,
    });
  }
  const coloured = colorBy ? colourOf(collection.features, colorBy) : undefined;
  if (paged) {
    status += `${status.endsWith(".") ? "" : "."} ${t(inView.data?.truncated ? "explore.map.truncated" : "explore.map.paged", { max: number.format(MAP_MAX) })}`;
  }
  // Said once, under the key the eye reads: on a phone the sentence would wrap around the map's top.
  let colourWords = "";
  if (colorBy && coloured?.domain) {
    const [min, max] = coloured.domain;
    colourWords = `${status.endsWith(".") ? "" : "."} ${t(min === max ? "explore.map.colouredOne" : "explore.map.colouredBy", {
      attr: readable(colorBy),
      min: number.format(min),
      max: number.format(max),
      unknown: number.format(coloured.unknown),
    })}`;
  }
  return (
    <div className="space-y-2">
      <p role="status" className="text-caption text-fg-muted">
        {status}
        {inView.isFetching ? ` ${t("explore.map.reading")}` : ""}
        {colourWords ? <span className="sr-only">{colourWords}</span> : null}
      </p>
      {colorBy && coloured?.domain ? <ColourKey attr={readable(colorBy)} domain={coloured.domain} unknown={coloured.unknown} /> : null}
      {inView.isError ? (
        <InlineError>
          {t("explore.map.viewFailed", { reason: inView.error instanceof Error ? inView.error.message : String(inView.error) })}
        </InlineError>
      ) : null}
      {collection.features.length === 0 && !paged ? null : (
      <div className="h-preview min-h-preview-min overflow-hidden rounded-lg border border-border">
        <MapLibreView
          project={project}
          fill
          label={t("explore.map.label", { type: query.type ?? "" })}
          onMoveEnd={paged ? onMoveEnd : undefined}
          layers={[
            {
              name: "explore-entities",
              url: "",
              data: collection,
              style: "circle",
              // A cluster has no one value, so a coloured map draws every point.
              cluster: !colorBy,
              colorBy: colorBy && coloured?.domain ? { property: colorBy, domain: coloured.domain } : undefined,
              popupProperties: shown,
              open: { label: t("explore.map.open"), onOpen },
            },
          ]}
        />
      </div>
      )}
    </div>
  );
}
