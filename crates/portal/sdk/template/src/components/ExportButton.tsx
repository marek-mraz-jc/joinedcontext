import { columnKind, download, format, pointOf, toCsv, toGeoJson, toPdf, toPng, useClient } from "@joinedcontext/sdk";
import type { Row } from "@joinedcontext/sdk";
import { t } from "../i18n";

export type ExportFormat = "csv" | "geojson" | "pdf" | "png";

/** The attributes an export carries by default: the first `max` the rows hold, no geometry. */
export function defaultColumns(rows: Row[], max = 8): string[] {
  if (max <= 0) return [];
  const sample = rows.slice(0, 50);
  const seen = new Set<string>();
  const cols: string[] = [];

  for (const row of sample) {
    for (const key of Object.keys(row)) {
      if (key === "id" || key === "type" || key === "@context") continue;
      if (seen.has(key)) continue;
      seen.add(key);
      if (columnKind(sample, key) !== "geo") {
        cols.push(key);
        if (cols.length >= max) return cols;
      }
    }
  }

  return cols;
}

export function ExportButton({
  rows,
  columns,
  filename = "export",
  formats = ["csv", "geojson"],
  location = "location",
  title = filename,
  canvas,
}: {
  rows: Row[];
  columns?: string[];
  filename?: string;
  formats?: ExportFormat[];
  location?: string;
  title?: string;
  canvas?: () => HTMLCanvasElement | null;
}): React.JSX.Element {
  const client = useClient();
  const cols = columns ?? defaultColumns(rows);
  const empty = rows.length === 0;

  const hasGeo = rows.some((r) => pointOf(r[location]) !== null);

  const handleCsv = () => {
    const blob = toCsv(["id", ...cols], rows);
    download(blob, `${filename}.csv`);
  };

  const handleGeoJson = () => {
    const blob = new Blob([JSON.stringify(toGeoJson(rows, location))], {
      type: "application/geo+json",
    });
    download(blob, `${filename}.geojson`);
  };

  const handlePdf = () => {
    const lines = rows
      .slice(0, 500)
      .map((r) => [r.id, ...cols.map((c) => format(r[c]))].join(" · "));
    const blob = toPdf({
      title,
      endpoint: client.config.endpointName ?? "",
      filters: "",
      takenAt: new Date().toISOString(),
      lines,
    });
    download(blob, `${filename}.pdf`);
  };

  const handlePng = () => {
    if (!canvas) return;
    const el = canvas();
    if (el) {
      void toPng(el).then((blob) => download(blob, `${filename}.png`));
    }
  };

  return (
    <div className="jc-export" role="group" aria-label={t("export.label")}>
      {formats.map((fmt) => {
        if (fmt === "csv") {
          return (
            <button key="csv" type="button" disabled={empty} onClick={handleCsv}>
              CSV
            </button>
          );
        }
        if (fmt === "geojson") {
          const disabled = empty || !hasGeo;
          const notice = !empty && !hasGeo ? t("export.noGeometry") : undefined;
          return (
            <button
              key="geojson"
              type="button"
              disabled={disabled}
              title={notice}
              onClick={handleGeoJson}
            >
              GeoJSON
            </button>
          );
        }
        if (fmt === "pdf") {
          return (
            <button key="pdf" type="button" disabled={empty} onClick={handlePdf}>
              PDF
            </button>
          );
        }
        if (fmt === "png") {
          return (
            <button key="png" type="button" disabled={empty || !canvas} onClick={handlePng}>
              PNG
            </button>
          );
        }
        return null;
      })}
    </div>
  );
}
