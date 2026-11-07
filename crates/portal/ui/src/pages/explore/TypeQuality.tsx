/**
 * The Quality tab of one entity type in Explore (T-3252, DM-74): the last daily run's count of
 * the type, how complete each attribute is, the newest change, numeric ranges and the values far
 * outside the rest, and how fresh each pipeline writing the type is. Example ids and ranges come
 * only to whoever reads the space's entities; the API leaves them out for anyone else.
 */
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Link } from "@tanstack/react-router";
import { api, unwrap } from "../../api/client";
import type { components } from "../../api/schema";
import {
  Badge,
  PageFailed,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeaderCell,
  TableRow,
} from "../../components/ui";

type Report = components["schemas"]["SpaceQuality"];

function isReport(value: unknown): value is Report {
  return typeof value === "object" && value !== null && "observedAt" in value;
}

export function TypeQuality({ project, space, type }: { project: string; space: string; type: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "en";
  // The same query key as the space page's report: one read serves both.
  const quality = useQuery({
    queryKey: ["projects", project, "spaces", space, "quality"],
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/spaces/{space}/quality", {
          params: { path: { project, space } },
        }),
      ) as unknown,
  });

  if (quality.isPending) {
    return (
      <p role="status" className="text-body text-fg-muted">
        {t("app.loading")}
      </p>
    );
  }
  if (quality.isError) {
    return <PageFailed error={quality.error} onRetry={() => void quality.refetch()} />;
  }
  const report = quality.data;
  if (!isReport(report)) {
    return <p className="text-body text-fg-muted">{t("spaces.quality.notYet")}</p>;
  }
  const own = (report.types ?? []).find((each) => each.type === type);
  const pipelines = report.freshness.filter((row) => row.type === type);
  const percent = new Intl.NumberFormat(locale, { style: "percent", maximumFractionDigits: 0 });
  const number = new Intl.NumberFormat(locale, { maximumFractionDigits: 3 });
  const when = (at: string | null | undefined) =>
    at ? new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }).format(new Date(at)) : "—";

  return (
    <div className="flex flex-col gap-3" data-testid="type-quality">
      <p className="text-body text-fg-muted">
        {t("explore.quality.observed", { at: when(report.observedAt) })}
        {report.truncated ? ` ${t("explore.quality.truncated")}` : ""}
      </p>
      {!own || own.count === 0 ? (
        <p className="text-body">{t("explore.quality.none", { type })}</p>
      ) : (
        <>
          <p className="text-body">{t("explore.quality.summary", { count: own.count, newest: when(own.newest) })}</p>
          <Table caption={t("explore.quality.attributes", { type })}>
            <TableHead>
              <TableHeaderCell>{t("explore.quality.attribute")}</TableHeaderCell>
              <TableHeaderCell>{t("explore.quality.completeness")}</TableHeaderCell>
              <TableHeaderCell>{t("explore.quality.range")}</TableHeaderCell>
              <TableHeaderCell>{t("explore.quality.outliers")}</TableHeaderCell>
            </TableHead>
            <TableBody>
              {own.attributes.map((attribute) => {
                const share = attribute.present / own.count;
                return (
                  <TableRow key={attribute.name}>
                    <TableCell className="font-mono">{attribute.name}</TableCell>
                    <TableCell>
                      <Badge tone={share >= 0.99 ? "success" : share >= 0.8 ? "warning" : "danger"}>
                        {percent.format(share)}
                      </Badge>{" "}
                      <span className="text-caption text-fg-muted">
                        {t("explore.quality.present", { present: attribute.present, count: own.count })}
                      </span>
                    </TableCell>
                    <TableCell>
                      {attribute.min !== undefined && attribute.min !== null && attribute.max !== undefined && attribute.max !== null
                        ? `${number.format(attribute.min)} – ${number.format(attribute.max)}`
                        : "—"}
                    </TableCell>
                    <TableCell>
                      {attribute.outliers === 0 ? (
                        "—"
                      ) : (
                        <span className="flex flex-col gap-0.5">
                          <span>{t("explore.quality.outlierCount", { count: attribute.outliers })}</span>
                          {attribute.outlierExamples.map((id) => (
                            <span key={id} className="break-all font-mono text-caption text-fg-muted">
                              {id}
                            </span>
                          ))}
                        </span>
                      )}
                    </TableCell>
                  </TableRow>
                );
              })}
            </TableBody>
          </Table>
        </>
      )}
      {pipelines.length > 0 ? (
        <section aria-labelledby="type-quality-pipelines" className="flex flex-col gap-1">
          <h3 id="type-quality-pipelines" className="text-body font-semibold text-fg">
            {t("explore.quality.pipelines")}
          </h3>
          <ul className="flex flex-col gap-1">
            {pipelines.map((row) => (
              <li key={row.pipeline} className="flex flex-wrap items-center gap-2 text-body">
                <span className="font-mono">{row.pipeline}</span>
                <Badge tone={row.state === "fresh" ? "success" : row.state === "stale" ? "danger" : "neutral"}>
                  {t(`spaces.quality.states.${row.state}`)}
                </Badge>
                <Link
                  to="/projects/$project/$plural"
                  params={{ project, plural: "pipelines" }}
                  className="text-caption underline"
                >
                  {t("explore.quality.rejected", { pipeline: row.pipeline })}
                </Link>
              </li>
            ))}
          </ul>
        </section>
      ) : null}
    </div>
  );
}
