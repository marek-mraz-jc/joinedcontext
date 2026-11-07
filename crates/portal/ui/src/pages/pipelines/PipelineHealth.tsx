import { PageFailed } from "../../components/ui/PageState";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, unwrap } from "../../api/client";
import type { components } from "../../api/schema";
import type { BadgeTone } from "../../components/ui/Badge";
import {
  Alert,
  Badge,
  Button,
  EmptyState,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeaderCell,
  TableRow,
  TableSkeleton,
} from "../../components/ui";

type Health = components["schemas"]["PipelineHealth"];
type HealthState = components["schemas"]["PipelineHealthState"];
type Links = components["schemas"]["PipelineLinks"];

/** The share of asked links that answered, in whole percent; T-5081 asks for 95 % or more. */
export function answeredShare(links: Links): number | null {
  return links.checked === 0 ? null : Math.floor((links.answered / links.checked) * 100);
}

/** The weekly link check of one source (T-5133): its share, its dead links and how to fix them. */
function LinksCell({ links, when }: { links: Links | null | undefined; when: (at: string) => string }): JSX.Element {
  const { t } = useTranslation();
  if (links == null) {
    return <span className="block max-w-xs text-caption text-fg-muted">{t("pipelines.health.linksNotChecked")}</span>;
  }
  const share = answeredShare(links);
  return (
    <span className="flex min-w-48 max-w-60 flex-col items-start gap-1 text-caption">
      {share == null ? null : (
        <span className={share < 95 ? "tabular-nums text-danger" : "tabular-nums"}>
          {t("pipelines.health.linksAnswer", { answered: links.answered, checked: links.checked, share })}
        </span>
      )}
      <span className="text-fg-muted">
        <time dateTime={links.checkedAt}>{t("pipelines.health.linksChecked", { at: when(links.checkedAt), skipped: links.skipped })}</time>
      </span>
      {links.deadCount > 0 ? (
        <>
          <span className="font-medium text-danger">{t("pipelines.health.linksDead", { count: links.deadCount })}</span>
          <ul className="flex flex-col gap-0.5">
            {links.dead.map((dead) => (
              <li key={dead.url}>
                <span className="break-all" title={dead.url}>
                  {dead.url.replace(/^https?:\/\//, "")}
                </span>{" "}
                <span className="whitespace-nowrap text-fg-muted">
                  ({dead.status == null ? t("pipelines.health.linksNoAnswer") : t("pipelines.health.linksStatus", { status: dead.status })})
                </span>
              </li>
            ))}
          </ul>
          {links.deadCount > links.dead.length ? (
            <span className="text-fg-muted">{t("pipelines.health.linksMore", { count: links.deadCount - links.dead.length })}</span>
          ) : null}
          <span>{t("pipelines.health.linksFix")}</span>
        </>
      ) : null}
    </span>
  );
}

const TONE: Record<HealthState, BadgeTone> = {
  failing: "danger",
  drift: "danger",
  empty: "warning",
  never: "neutral",
  ok: "success",
};

/** The order a steward reads them in: what needs a hand first. */
const RANK: Record<HealthState, number> = { failing: 0, drift: 1, empty: 2, never: 3, ok: 4 };

/** Alerted first, then by state, then by name. */
export function byUrgency(items: Health[]): Health[] {
  return [...items].sort(
    (a, b) =>
      Number(b.alert) - Number(a.alert) ||
      RANK[a.state] - RANK[b.state] ||
      a.pipeline.localeCompare(b.pipeline),
  );
}

/**
 * Each source's health from its kept runs (T-5048): when it last delivered, what its newest run
 * sent against the week's median, and the last error of its log. Stewards only: the server
 * answers 403 to anyone who approves no pipeline, and the page shows the tab only to them.
 */
export function PipelineHealthView({
  project,
  onOpenRuns,
}: {
  project: string;
  onOpenRuns: (name: string) => void;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "sk";
  const time = new Intl.DateTimeFormat(locale, { dateStyle: "short", timeStyle: "short" });
  const health = useQuery({
    queryKey: ["projects", project, "pipeline-health"],
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/pipeline-health", {
          params: { path: { project } },
        }),
      ),
    refetchInterval: 60_000,
  });
  const when = (at: string | null | undefined) => (at == null ? t("pipelines.health.never") : time.format(new Date(at)));

  if (health.isError) {
    return <PageFailed error={health.error} onRetry={() => void health.refetch()} />;
  }
  if (health.isPending) {
    return (
      <Table caption={t("pipelines.health.caption")} status={t("app.loading")}>
        <TableSkeleton columns={7} />
      </Table>
    );
  }
  const items = byUrgency(health.data.items);
  if (items.length === 0) {
    return (
      <EmptyState
        icon="pipelines"
        title={t("pipelines.health.empty")}
        description={t("pipelines.health.emptyHint")}
      />
    );
  }
  const alerted = items.filter((item) => item.alert).length;
  const broken = items.filter((item) => item.state === "failing" || item.state === "drift").length;
  return (
    <div className="flex flex-col gap-4">
      <p className="text-body text-fg-muted">{t("pipelines.health.lead")}</p>
      {broken > 0 ? (
        <Alert tone={alerted > 0 ? "danger" : "warning"} role="status">
          {t("pipelines.health.summary", { broken, alerted, total: items.length })}
        </Alert>
      ) : (
        <Alert tone="success" role="status">
          {t("pipelines.health.allWell", { total: items.length })}
        </Alert>
      )}
      <Table caption={t("pipelines.health.caption")}>
        <TableHead>
          <TableHeaderCell>{t("pipelines.health.source")}</TableHeaderCell>
          <TableHeaderCell>{t("pipelines.health.state")}</TableHeaderCell>
          <TableHeaderCell>{t("pipelines.health.lastSuccess")}</TableHeaderCell>
          <TableHeaderCell>{t("pipelines.health.lastSent")}</TableHeaderCell>
          <TableHeaderCell>{t("pipelines.health.change")}</TableHeaderCell>
          <TableHeaderCell>{t("pipelines.health.lastLogLine")}</TableHeaderCell>
          <TableHeaderCell>{t("pipelines.health.links")}</TableHeaderCell>
          <TableHeaderCell>
            <span className="sr-only">{t("pipelines.runs.log")}</span>
          </TableHeaderCell>
        </TableHead>
        <TableBody>
          {items.map((item) => (
            <TableRow key={item.pipeline}>
              <TableCell>
                <span className="font-medium">{item.pipeline}</span>
              </TableCell>
              <TableCell>
                <span className="flex flex-col items-start gap-1">
                  <Badge tone={TONE[item.state]}>{t(`pipelines.health.states.${item.state}`)}</Badge>
                  {item.failedRuns > 1 ? (
                    <span className="text-caption text-fg-muted">
                      {t("pipelines.health.failedRuns", { count: item.failedRuns })}
                    </span>
                  ) : null}
                  {item.errors != null && item.errors.errors > 0 ? (
                    <span
                      className={
                        item.state === "failing"
                          ? "text-caption text-danger tabular-nums"
                          : "text-caption text-fg-muted tabular-nums"
                      }
                    >
                      {t("pipelines.health.errorRatio", {
                        percent: item.errors.percent.toLocaleString(locale),
                        errors: item.errors.errors.toLocaleString(locale),
                        records: item.errors.records.toLocaleString(locale),
                      })}
                    </span>
                  ) : null}
                  {item.alert ? (
                    <span className="text-caption text-danger">{t("pipelines.health.alerted")}</span>
                  ) : null}
                </span>
              </TableCell>
              <TableCell>
                {item.lastSuccessAt == null ? (
                  t("pipelines.health.never")
                ) : (
                  <time dateTime={item.lastSuccessAt} className="whitespace-nowrap tabular-nums">
                    {when(item.lastSuccessAt)}
                  </time>
                )}
              </TableCell>
              <TableCell>
                <span className="tabular-nums">
                  {item.lastSent == null ? "—" : item.lastSent.toLocaleString(locale)}
                </span>
              </TableCell>
              <TableCell>
                {item.changePercent == null ? (
                  "—"
                ) : (
                  <span
                    className={item.changePercent <= -80 ? "tabular-nums text-danger" : "tabular-nums"}
                  >
                    {item.changePercent > 0 ? "+" : item.changePercent < 0 ? "−" : ""}
                    {Math.abs(item.changePercent)} %
                    <span className="block text-caption text-fg-muted">
                      {t("pipelines.health.median", {
                        median: (item.medianSent ?? 0).toLocaleString(locale),
                      })}
                    </span>
                  </span>
                )}
              </TableCell>
              <TableCell>
                {item.lastError == null ? (
                  "—"
                ) : (
                  <span className="block max-w-md break-words text-caption">
                    {item.lastError}
                    {item.lastErrorAt != null ? (
                      <time dateTime={item.lastErrorAt} className="block text-fg-muted tabular-nums">
                        {when(item.lastErrorAt)}
                      </time>
                    ) : null}
                  </span>
                )}
              </TableCell>
              <TableCell>
                <LinksCell links={item.links} when={(at) => when(at)} />
              </TableCell>
              <TableCell>
                <Button
                  size="sm"
                  className="whitespace-nowrap"
                  aria-label={t("pipelines.health.openRuns", { name: item.pipeline })}
                  onClick={() => onOpenRuns(item.pipeline)}
                >
                  {t("pipelines.runs.open")}
                </Button>
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </div>
  );
}
