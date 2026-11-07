/**
 * The dataset page's map (EP-82, T-5628): the located entities of a public dataset, read
 * anonymously through its Endpoint by the Portal, each popup with the key attributes the
 * Endpoint's display, the model or jc-core's field order chooses, and the same places as a list
 * a keyboard and a screen reader reach. A dataset without locations shows nothing at all.
 */
import { Suspense, lazy, useCallback, useRef, useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, unwrap } from "../../api/client";
import type { components } from "../../api/schema";
import {
  Alert,
  Button,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeaderCell,
  TableRow,
} from "../../components/ui";
import { datasetKey } from "./queries";

// MapLibre (about 200 KB gzipped) loads only for a dataset that has a place to draw.
const MapLibreView = lazy(() =>
  import("../../components/dashboards/MapLibreView").then((module) => ({
    default: module.MapLibreView,
  })),
);

type CatalogueMap = components["schemas"]["CatalogueMap"];

/** The features as GeoJSON, each popup line keyed by its label: the popup shows the key as written. */
export function mapCollection(map: CatalogueMap): {
  type: "FeatureCollection";
  features: unknown[];
} {
  return {
    type: "FeatureCollection",
    features: map.features.map((feature) => ({
      type: "Feature",
      geometry: feature.geometry,
      properties: Object.fromEntries(
        map.fields.flatMap((field) => {
          const value = feature.properties[field.slot];
          return value === undefined ? [] : [[field.label, value]];
        }),
      ),
    })),
  };
}

/**
 * A ref for a box and whether it has come into the viewport yet. The map below the fold of a phone
 * draws only when scrolled to: MapLibre's first draw blocked the page's main thread for ~0.8 s of
 * a phone's load (web-performance TBT, T-6600). A browser without IntersectionObserver draws at once.
 */
function useInSight(): [(box: Element | null) => void, boolean] {
  const [seen, setSeen] = useState(() => typeof IntersectionObserver === "undefined");
  const watching = useRef<IntersectionObserver | null>(null);
  const ref = useCallback(
    (box: Element | null) => {
      watching.current?.disconnect();
      watching.current = null;
      if (!box || seen) return;
      const observer = new IntersectionObserver((entries) => {
        if (entries.some((entry) => entry.isIntersecting)) setSeen(true);
      });
      observer.observe(box);
      watching.current = observer;
    },
    [seen],
  );
  return [ref, seen];
}

export function DatasetMap({ name }: { name: string }): JSX.Element | null {
  const { t, i18n } = useTranslation();
  const [frame, inSight] = useInSight();
  const lang = (i18n.resolvedLanguage ?? i18n.language ?? "en").slice(0, 2);
  const map = useQuery({
    queryKey: [...datasetKey(name), "map", lang],
    retry: false,
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/catalogue/datasets/{name}/map", {
          params: { path: { name }, query: { lang } },
        }),
      ),
  });
  // Nothing to say before the answer, for a dataset no Endpoint serves, or one without places:
  // the page shows no empty box.
  if (map.isPending) return null;
  if (map.isError) {
    if (map.error instanceof ApiError && map.error.status === 404) return null;
    return (
      <Section>
        <Alert
          tone="danger"
          role="alert"
          actions={
            <Button size="sm" onClick={() => void map.refetch()}>
              {t("app.error.retry")}
            </Button>
          }
        >
          {/* The server's reason is one sentence of its own; the generic line only stands in for it. */}
          {(map.error instanceof ApiError && map.error.problem?.detail) || t("catalogue.dataset.mapFailed")}
        </Alert>
      </Section>
    );
  }
  const { data } = map;
  if (!Array.isArray(data.features) || data.features.length === 0) return null;
  const number = new Intl.NumberFormat(i18n.language);
  const shown = data.features.length;
  // No total is a full page whose count took too long (T-6541): a short page is its own count.
  const total = data.total ?? undefined;
  const fields =
    data.fields.length > 0
      ? data.fields
      : [{ slot: "", label: t("catalogue.dataset.mapPlace") }];
  const loading = <p className="p-3 text-body text-fg-muted">{t("catalogue.dataset.mapLoading")}</p>;
  return (
    <Section>
      <p role="status" className="text-caption text-fg-muted">
        {total === undefined
          ? t("catalogue.dataset.mapCountFirst", { shown: number.format(shown) })
          : total > shown
            ? t("catalogue.dataset.mapCountOf", {
                shown: number.format(shown),
                total: number.format(total),
              })
            : t("catalogue.dataset.mapCount", { count: shown })}
      </p>
      <div ref={frame} className="h-72 overflow-hidden rounded-lg border border-border sm:h-96">
        {inSight ? (
          <Suspense fallback={loading}>
            <MapLibreView
              fill
              project={data.basemap ?? undefined}
              label={t("catalogue.dataset.mapLabel", { type: data.type })}
              layers={[
                {
                  name: "dataset-features",
                  url: "",
                  data: mapCollection(data),
                  style: "circle",
                  // MapLibre clusters points alone.
                  cluster: data.features.every(
                    (feature) =>
                      (feature.geometry as { type?: unknown }).type === "Point",
                  ),
                  popupProperties: data.fields.map((field) => field.label),
                },
              ]}
            />
          </Suspense>
        ) : (
          loading
        )}
      </div>
      <details className="text-body">
        <summary className="focus-ring w-fit cursor-pointer rounded-sm text-primary-soft-fg">
          {t("catalogue.dataset.mapList", { count: shown })}
        </summary>
        <Table
          caption={t("catalogue.dataset.mapLabel", { type: data.type })}
          maxHeight="24rem"
          className="mt-2"
        >
          <TableHead>
            {fields.map((field) => (
              <TableHeaderCell key={field.slot}>{field.label}</TableHeaderCell>
            ))}
          </TableHead>
          <TableBody>
            {data.features.map((feature) => (
              <TableRow key={feature.id}>
                {fields.map((field, index) => (
                  <TableCell key={field.slot} primary={index === 0}>
                    {field.slot === ""
                      ? feature.id
                      : (feature.properties[field.slot] ?? "—")}
                  </TableCell>
                ))}
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </details>
    </Section>
  );
}

function Section({ children }: { children: React.ReactNode }): JSX.Element {
  const { t } = useTranslation();
  return (
    <section aria-labelledby="catalogue-map" className="flex flex-col gap-3">
      <h2 id="catalogue-map" className="font-heading text-title font-semibold">
        {t("catalogue.dataset.map")}
      </h2>
      {children}
    </section>
  );
}
