import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
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
import type { BadgeTone } from "../../components/ui/Badge";

type Report = components["schemas"]["SpaceQuality"];
type FreshState = components["schemas"]["FreshState"];

const TONE: Record<FreshState, BadgeTone> = {
  fresh: "success",
  stale: "danger",
  empty: "warning",
  untargeted: "neutral",
};

/** A whole report, as opposed to the `{}` the API answers before the first run. */
function isReport(value: unknown): value is Report {
  return typeof value === "object" && value !== null && "observedAt" in value;
}

/**
 * Data quality of one space (DM-74, T-2796): the last daily run's share of valid entities, the
 * rules they break with example ids (for whoever reads the entities), and how fresh the data of
 * each pipeline writing into the space is. Before the first run it says so, never "all valid".
 */
export function SpaceQuality({ project, space }: { project: string; space: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "en";
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
    return (
      <PageFailed
        error={quality.error}
        onRetry={() => {
          void quality.refetch();
        }}
      />
    );
  }
  const report = quality.data;
  if (!isReport(report)) {
    return <p className="text-body text-fg-muted">{t("spaces.quality.notYet")}</p>;
  }

  const when = new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" });
  const percent = new Intl.NumberFormat(locale, { style: "percent", maximumFractionDigits: 1 });
  const stale = report.freshness.filter((row) => row.state === "stale" || row.state === "empty");
  const parts = [
    report.checked === 0
      ? t("spaces.quality.nothingToCheck")
      : t("spaces.quality.valid", { share: percent.format((report.checked - report.invalid) / report.checked) }),
    t("spaces.quality.rulesFailing", { count: report.rules.length }),
    ...(report.freshness.length === 0
      ? []
      : [stale.length === 0 ? t("spaces.quality.allFresh") : t("spaces.quality.stale", { count: stale.length })]),
  ];
  const duration = (seconds: number) =>
    seconds < 7200
      ? t("spaces.quality.minutes", { count: Math.round(seconds / 60) })
      : t("spaces.quality.hours", { count: Math.round(seconds / 3600) });

  return (
    <div className="space-y-3">
      <p className="text-body text-fg">{parts.join(" · ")}</p>
      <p className="text-caption text-fg-muted">
        {t("spaces.quality.checkedAt", { when: when.format(new Date(report.observedAt)), count: report.checked })}
        {report.truncated ? ` ${t("spaces.quality.truncated")}` : ""}
      </p>
      {report.rules.length > 0 ? (
        <Table caption={t("spaces.quality.rulesCaption")}>
          <TableHead>
            <TableHeaderCell>{t("spaces.quality.rule")}</TableHeaderCell>
            <TableHeaderCell>{t("spaces.quality.attribute")}</TableHeaderCell>
            <TableHeaderCell align="right">{t("spaces.quality.entities")}</TableHeaderCell>
            <TableHeaderCell>{t("spaces.quality.examples")}</TableHeaderCell>
          </TableHead>
          <TableBody>
            {report.rules.map((rule) => (
              <TableRow key={`${rule.rule} ${rule.path}`}>
                <TableCell>
                  <code>{rule.rule}</code>
                </TableCell>
                <TableCell>{rule.path ? <code>{rule.path}</code> : t("spaces.quality.wholeEntity")}</TableCell>
                <TableCell align="right">{rule.count.toLocaleString(locale)}</TableCell>
                <TableCell>
                  {rule.examples.length > 0 ? (
                    <ul className="space-y-0.5">
                      {rule.examples.map((id) => (
                        <li key={id} className="font-mono text-caption [overflow-wrap:anywhere]">
                          {id}
                        </li>
                      ))}
                    </ul>
                  ) : (
                    <span className="text-fg-muted">{t("spaces.quality.examplesHidden")}</span>
                  )}
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      ) : null}
      {report.freshness.length > 0 ? (
        <Table caption={t("spaces.quality.freshnessCaption")}>
          <TableHead>
            <TableHeaderCell>{t("spaces.quality.pipeline")}</TableHeaderCell>
            <TableHeaderCell>{t("spaces.quality.type")}</TableHeaderCell>
            <TableHeaderCell>{t("spaces.quality.newest")}</TableHeaderCell>
            <TableHeaderCell>{t("spaces.quality.target")}</TableHeaderCell>
            <TableHeaderCell>{t("spaces.quality.state")}</TableHeaderCell>
          </TableHead>
          <TableBody>
            {report.freshness.map((row) => (
              <TableRow key={row.pipeline}>
                <TableCell primary>{row.pipeline}</TableCell>
                <TableCell>{row.type || t("spaces.quality.anyType")}</TableCell>
                <TableCell>{row.newest ? when.format(new Date(row.newest)) : "—"}</TableCell>
                <TableCell>{row.targetSeconds == null ? "—" : duration(row.targetSeconds)}</TableCell>
                <TableCell>
                  <span className="inline-flex flex-wrap items-center gap-1">
                    <Badge tone={TONE[row.state]}>{t(`spaces.quality.states.${row.state}`)}</Badge>
                    {row.paused ? <Badge tone="warning">{t("spaces.quality.paused")}</Badge> : null}
                  </span>
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      ) : null}
    </div>
  );
}
