import { useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, unwrap } from "../../api/client";
import type { components } from "../../api/schema";
import {
  Alert,
  Badge,
  EmptyState,
  Field,
  InlineError,
  Select,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeaderCell,
  TableRow,
  TableSkeleton,
} from "../../components/ui";
import type { BadgeTone } from "../../components/ui";
import { fromIso, HISTORY_FOREVER, HISTORY_OFF } from "./retention";

type Audit = components["schemas"]["TemporalAudit"];
type Verdict = components["schemas"]["TemporalVerdict"];

const TONE: Record<Verdict, BadgeTone> = {
  ok: "success",
  "loses-history": "danger",
  "unbounded-growth": "warning",
  "duplicates-rows": "warning",
  "stamped-on-receive": "info",
};
const VERDICTS = Object.keys(TONE) as Verdict[];
const ALL = "";

/**
 * How every pipeline that writes the space turns its writes into history rows (T-4844, DM-104..
 * DM-109): per pipeline, entity type and attribute the recording, where the time of a row comes
 * from, its unit and dataset, the type's history, the pipeline's lifetime and a verdict in words.
 */
export function SpaceTemporalAudit({ project, space }: { project: string; space: string }): JSX.Element {
  const { t } = useTranslation();
  const [pipeline, setPipeline] = useState(ALL);
  const [verdict, setVerdict] = useState(ALL);
  const audit = useQuery({
    queryKey: ["projects", project, "spaces", space, "temporal-audit"],
    retry: false,
    queryFn: async (): Promise<Audit> =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/spaces/{space}/temporal-audit", {
          params: { path: { project, space } },
        }),
      ),
  });

  // A skeleton is a <tbody>: it goes inside a Table, never straight into the section.
  if (audit.isPending) {
    return (
      <Table caption={t("spaces.temporal.title")} status={t("app.loading")}>
        <TableSkeleton columns={4} rows={4} />
      </Table>
    );
  }
  if (audit.isError) {
    const error = audit.error;
    const detail = error instanceof ApiError ? (error.problem?.detail ?? error.message) : String(error);
    return <InlineError>{t("spaces.temporal.failed", { detail })}</InlineError>;
  }
  // An answer without a field reads as nothing of it, never as a crash of the whole tab.
  const rows = audit.data.rows ?? [];
  const unread = audit.data.unread ?? [];
  const verdicts: Partial<Record<string, number>> = audit.data.verdicts ?? {};
  if (rows.length === 0 && unread.length === 0) {
    return <EmptyState bare title={t("spaces.temporal.empty")} description={t("spaces.temporal.emptyHint")} />;
  }
  const pipelines = [...new Set(rows.map((row) => row.pipeline))].sort();
  const shown = rows.filter(
    (row) => (pipeline === ALL || row.pipeline === pipeline) && (verdict === ALL || row.verdict === verdict),
  );
  const list = (items: string[]) => (items.length === 0 ? t("spaces.temporal.none") : items.join(", "));
  // A duration in the Retention tab's words: "30 days", "Off: no history", "Forever".
  const duration = (iso: string) => {
    if (iso === HISTORY_OFF) return t("spaces.retention.offShown");
    if (iso === HISTORY_FOREVER) return t("spaces.retention.forever");
    const keep = fromIso(iso);
    return keep === undefined ? iso : t(`spaces.retention.amount.${keep.unit}`, { count: keep.amount });
  };
  const ends = (said: string) =>
    said === "expiresAt in mapping"
      ? t("spaces.temporal.expiresMapped")
      : said.startsWith("expiry ")
        ? t("spaces.temporal.expiry", { after: duration(said.slice(7)) })
        : duration(said);

  return (
    <div className="space-y-3">
      <p className="text-body text-fg-muted">{t("spaces.temporal.lead")}</p>
      <ul className="flex flex-wrap gap-2" aria-label={t("spaces.temporal.summary")}>
        {VERDICTS.map((v) => (
          <li key={v}>
            <Badge tone={TONE[v]}>{t("spaces.temporal.count", { verdict: t(`spaces.temporal.verdict.${v}`), n: verdicts[v] ?? 0 })}</Badge>
          </li>
        ))}
      </ul>
      {unread.length > 0 && (
        <Alert tone="warning">{t("spaces.temporal.unread", { pipelines: unread.join(", ") })}</Alert>
      )}
      <div className="flex flex-wrap gap-3">
        <Field id="temporal-pipeline" label={t("spaces.temporal.pipelineFilter")}>
          <Select id="temporal-pipeline" value={pipeline} onChange={(e) => setPipeline(e.target.value)}>
            <option value={ALL}>{t("spaces.temporal.allPipelines")}</option>
            {pipelines.map((name) => (
              <option key={name} value={name}>
                {name}
              </option>
            ))}
          </Select>
        </Field>
        <Field id="temporal-verdict" label={t("spaces.temporal.verdictFilter")}>
          <Select id="temporal-verdict" value={verdict} onChange={(e) => setVerdict(e.target.value)}>
            <option value={ALL}>{t("spaces.temporal.allVerdicts")}</option>
            {VERDICTS.map((v) => (
              <option key={v} value={v}>
                {t(`spaces.temporal.verdict.${v}`)}
              </option>
            ))}
          </Select>
        </Field>
      </div>
      {shown.length === 0 ? (
        <EmptyState bare title={t("spaces.temporal.noMatch")} description={t("spaces.temporal.noMatchHint")} />
      ) : (
        <Table caption={t("spaces.temporal.caption")}>
          <TableHead>
            <TableHeaderCell>{t("spaces.temporal.attribute")}</TableHeaderCell>
            <TableHeaderCell>{t("spaces.temporal.verdictColumn")}</TableHeaderCell>
            <TableHeaderCell>{t("spaces.temporal.rows")}</TableHeaderCell>
            <TableHeaderCell>{t("spaces.temporal.kept")}</TableHeaderCell>
          </TableHead>
          <TableBody>
            {shown.map((row) => (
              <TableRow key={`${row.pipeline}/${row.entityType}/${row.attribute}`}>
                <TableCell>
                  <span className="flex flex-col">
                    <span className="font-medium">{row.attribute}</span>
                    <span className="text-caption text-fg-muted">
                      {t("spaces.temporal.of", { type: row.entityType, pipeline: row.pipeline })}
                    </span>
                  </span>
                </TableCell>
                <TableCell>
                  <span className="flex flex-col items-start gap-1">
                    <Badge tone={TONE[row.verdict]}>{t(`spaces.temporal.verdict.${row.verdict}`)}</Badge>
                    {row.verdict !== "ok" && (
                      <span className="text-caption text-fg-muted">{t(`spaces.temporal.fix.${row.verdict}`)}</span>
                    )}
                    {/* Stamped on receipt but no measurement: say why it still matches (T-6089). */}
                    {row.verdict === "ok" && row.record !== "none" && row.observedAt.includes("received") && (
                      <span className="text-caption text-fg-muted">{t("spaces.temporal.receivedOk")}</span>
                    )}
                  </span>
                </TableCell>
                <TableCell>
                  <span className="flex flex-col">
                    <span>{t(`spaces.temporal.recordMode.${row.record}`, { defaultValue: row.record })}</span>
                    <span className="text-caption text-fg-muted">
                      {t("spaces.temporal.timeOf", { source: row.observedAt.map((s) => t(`spaces.temporal.source.${s}`)).join(", ") })}
                    </span>
                    <span className="text-caption text-fg-muted">
                      {t("spaces.temporal.unitOf", { unit: list(row.unitCode) })}
                      {row.datasetId.length > 0 && ` · ${t("spaces.temporal.datasets", { n: row.datasetId.length })}`}
                    </span>
                  </span>
                </TableCell>
                <TableCell>
                  <span className="flex flex-col">
                    <span>{t("spaces.temporal.history", { history: duration(row.history) })}</span>
                    <span className="text-caption text-fg-muted">
                      {t("spaces.temporal.lifetime", { lifetime: list(row.lifetime.map(ends)) })}
                    </span>
                  </span>
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      )}
    </div>
  );
}
