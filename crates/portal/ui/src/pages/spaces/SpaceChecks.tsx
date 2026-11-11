import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, unwrap } from "../../api/client";
import type { components } from "../../api/schema";
import { AdminGrid } from "../../components/grid/AdminGrid";
import type { DataColumn } from "../../components/grid/AdminGrid";

type Checks = components["schemas"]["SpaceChecks"];
type Counts = components["schemas"]["Counts"];
type SpaceCheckRow = Checks["rules"][number];

/**
 * The space's quality rules as the gateway checked writes against them (DM-94, API/01 §27.1):
 * per type and rule the pass rate over 24 hours and 7 days, what was rejected and quarantined,
 * and when a write was last rejected. Names and counts only, never a value. Before the Portal
 * has read the gateway's counters it says the checks are unknown, never "all pass".
 */
export function SpaceChecks({
  project,
  space,
}: {
  project: string;
  space: string;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "en";
  const checks = useQuery({
    queryKey: ["projects", project, "spaces", space, "quality", "checks"],
    queryFn: async (): Promise<Checks> =>
      unwrap(
        await api.GET(
          "/api/v1/projects/{project}/spaces/{space}/quality/checks",
          {
            params: { path: { project, space } },
          },
        ),
      ),
  });

  if (checks.isPending) {
    return (
      <p role="status" className="text-body text-fg-muted">
        {t("app.loading")}
      </p>
    );
  }
  const data = checks.data;
  if (checks.isError || !data?.at || !data.windows) {
    return (
      <p className="text-body text-fg-muted">
        {t("spaces.quality.checks.unknown")}
      </p>
    );
  }
  if (data.rules.length === 0) {
    return (
      <p className="text-body text-fg-muted">
        {t("spaces.quality.checks.noRules")}
      </p>
    );
  }

  const when = new Intl.DateTimeFormat(locale, {
    dateStyle: "medium",
    timeStyle: "short",
  });
  const percent = new Intl.NumberFormat(locale, {
    style: "percent",
    maximumFractionDigits: 1,
  });
  const rate = (counts: Counts) =>
    counts.passRate == null ? "—" : percent.format(counts.passRate);
  const columns: DataColumn<SpaceCheckRow>[] = [
    { key: "type", label: t("spaces.quality.type"), text: (row) => row.type },
    {
      key: "rule",
      label: t("spaces.quality.rule"),
      text: (row) => row.rule,
      cell: (row) => <code>{row.rule}</code>,
    },
    {
      key: "attribute",
      label: t("spaces.quality.attribute"),
      text: (row) => row.attribute ?? "",
      cell: (row) => (row.attribute ? <code>{row.attribute}</code> : "—"),
    },
    {
      key: "passDay",
      label: t("spaces.quality.checks.passDay"),
      kind: "number",
      align: "right",
      text: (row) => String(row["24h"].passRate ?? ""),
      cell: (row) => rate(row["24h"]),
    },
    {
      key: "passWeek",
      label: t("spaces.quality.checks.passWeek"),
      kind: "number",
      align: "right",
      text: (row) => String(row["7d"].passRate ?? ""),
      cell: (row) => rate(row["7d"]),
    },
    {
      key: "rejected",
      label: t("spaces.quality.checks.rejected"),
      kind: "number",
      align: "right",
      text: (row) => String(row["7d"].reject),
      cell: (row) => row["7d"].reject.toLocaleString(locale),
    },
    {
      key: "quarantined",
      label: t("spaces.quality.checks.quarantined"),
      kind: "number",
      align: "right",
      text: (row) => String(row["7d"].quarantine),
      cell: (row) => row["7d"].quarantine.toLocaleString(locale),
    },
    {
      key: "lastRejected",
      label: t("spaces.quality.checks.lastRejected"),
      kind: "date",
      text: (row) => row.lastRejected ?? "",
      cell: (row) => (row.lastRejected ? when.format(new Date(row.lastRejected)) : "—"),
    },
  ];

  return (
    <div className="space-y-2">
      <AdminGrid<SpaceCheckRow>
        caption={t("spaces.quality.checks.caption")}
        storageKey={`${project}:${space}:quality-checks`}
        stack
        rows={data.rules}
        rowKey={(row) => `${row.type} ${row.rule}`}
        columns={columns}
        empty={<p className="text-body text-fg-muted">{t("spaces.quality.checks.noRules")}</p>}
      />
      <p className="text-caption text-fg-muted">
        {/* Both windows start at the same instant when the counters began after the 24 h mark
            (a gateway restart): one date, not the same date twice. */}
        {data.windows["24h"] === data.windows["7d"]
          ? t("spaces.quality.checks.sinceOne", { at: when.format(new Date(data.windows["24h"])) })
          : t("spaces.quality.checks.since", {
              day: when.format(new Date(data.windows["24h"])),
              week: when.format(new Date(data.windows["7d"])),
            })}
      </p>
    </div>
  );
}
