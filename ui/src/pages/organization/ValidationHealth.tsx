import { PageFailed } from "../../components/ui/PageState";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, unwrap } from "../../api/client";
import { ORG_NAMESPACE } from "../../api/manifest";
import { administersOrganization, usePermissions } from "../../api/permissions";
import type { components } from "../../api/schema";
import {
  Alert,
  Badge,
  EmptyState,
  Table,
  TableBody,
  TableCell,
  TableEmpty,
  TableHead,
  TableHeaderCell,
  TableRow,
  TableSkeleton,
} from "../../components/ui";
import type { BadgeTone } from "../../components/ui/Badge";

type CheckHealth = components["schemas"]["CheckHealth"];
type CheckState = components["schemas"]["CheckState"];
type Point = components["schemas"]["Point"];

const TONE: Record<CheckState, BadgeTone> = {
  green: "success",
  red: "danger",
  stale: "warning",
  unreadable: "neutral",
};

/**
 * The share of results that passed in each earlier run, as a line: a red run dips it. The
 * picture is decoration; its label says the same in words for a screen reader.
 */
function Trend({ history, label }: { history: Point[]; label: string }): JSX.Element | null {
  if (history.length === 0) return null;
  const width = 96;
  const height = 24;
  const share = history.map(({ pass, fail, error }) => {
    const counted = pass + fail + error;
    return counted === 0 ? 1 : pass / counted;
  });
  const step = share.length > 1 ? width / (share.length - 1) : 0;
  const points = share.map((value, index) => `${index * step},${(1 - value) * (height - 2) + 1}`).join(" ");
  return (
    <svg role="img" aria-label={label} width={width} height={height} viewBox={`0 0 ${width} ${height}`}>
      <polyline points={points} fill="none" stroke="currentColor" strokeWidth={1.5} className="text-primary-soft-fg" />
    </svg>
  );
}

/**
 * Organization → Health (OPS-53, T-2803): the last result every validation check published,
 * with its state, its seven-day trend and the failures with the tasks they filed. Only an
 * administrator of the organization (approve and delete on RoleBinding, PF-03) reads it; anyone
 * else is told so and nothing is fetched.
 */
export function ValidationHealth(): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "en";
  const permissions = usePermissions(ORG_NAMESPACE);
  const administers = administersOrganization(permissions.data);
  const health = useQuery({
    queryKey: ["validationHealth"],
    enabled: administers,
    queryFn: async () => unwrap(await api.GET("/api/v1/organization/health")),
  });

  if (permissions.isLoading) {
    return (
      <p role="status" className="text-body text-fg-muted">
        {t("app.loading")}
      </p>
    );
  }

  const checks: CheckHealth[] = health.data?.checks ?? [];
  const count = (state: CheckState) => checks.filter((check) => check.state === state).length;
  const troubles = (["red", "stale", "unreadable"] as const)
    .filter((state) => count(state) > 0)
    .map((state) => t(`organization.health.summary.${state}`, { count: count(state) }));
  const when = (at: string) =>
    new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }).format(new Date(at));

  return (
    <section className="space-y-4" aria-labelledby="organization-health-heading">
      <div>
        <h2 id="organization-health-heading" className="text-title font-semibold text-fg">
          {t("organization.health.title")}
        </h2>
        <p className="text-body text-fg-muted">{t("organization.health.lead")}</p>
      </div>
      {!administers ? (
        <Alert tone="info">{t("organization.health.hidden")}</Alert>
      ) : health.isError ? (
        <PageFailed error={health.error} onRetry={() => void health.refetch()} />
      ) : (
        <>
          {health.isSuccess && checks.length > 0 ? (
            <Alert tone={troubles.length === 0 ? "success" : "danger"} role="status">
              {troubles.length === 0 ? t("organization.health.summary.green") : troubles.join(", ")}
            </Alert>
          ) : null}
          <Table
            caption={t("organization.health.caption")}
            status={health.isPending ? t("app.loading") : undefined}
          >
            <TableHead>
              <TableHeaderCell>{t("organization.health.check")}</TableHeaderCell>
              <TableHeaderCell>{t("organization.health.state")}</TableHeaderCell>
              <TableHeaderCell>{t("organization.health.lastRun")}</TableHeaderCell>
              <TableHeaderCell>{t("organization.health.results")}</TableHeaderCell>
              <TableHeaderCell>{t("organization.health.trend")}</TableHeaderCell>
              <TableHeaderCell>{t("organization.health.failures")}</TableHeaderCell>
            </TableHead>
            {health.isPending ? (
              <TableSkeleton columns={6} />
            ) : (
              <TableBody>
                {checks.length === 0 ? (
                  <TableEmpty columns={6}>
                    <EmptyState
                      bare
                      title={t("organization.health.empty")}
                      description={t("organization.health.emptyHint")}
                    />
                  </TableEmpty>
                ) : (
                  checks.map(({ check, state, result }) => (
                    <TableRow key={check}>
                      <TableCell primary>
                        <span className="font-mono">{check}</span>
                      </TableCell>
                      <TableCell>
                        <Badge tone={TONE[state]}>{t(`organization.health.states.${state}`)}</Badge>
                      </TableCell>
                      <TableCell>
                        {result ? (
                          <>
                            <time dateTime={result.at}>{when(result.at)}</time>
                            {result.run ? <span className="block text-caption text-fg-muted">{result.run}</span> : null}
                          </>
                        ) : (
                          t("organization.health.unreadableHint")
                        )}
                      </TableCell>
                      <TableCell>{result ? t("organization.health.counts", { ...result.counts }) : "—"}</TableCell>
                      <TableCell>
                        {result ? (
                          <Trend
                            history={result.history ?? []}
                            label={t("organization.health.trendLabel", {
                              runs: (result.history ?? []).length,
                              red: (result.history ?? []).filter((point) => point.fail + point.error > 0).length,
                            })}
                          />
                        ) : null}
                      </TableCell>
                      <TableCell>
                        {result && (result.failures ?? []).length > 0 ? (
                          <details>
                            <summary className="focus-ring cursor-pointer rounded-sm">
                              {t("organization.health.showFailures", { count: (result.failures ?? []).length })}
                            </summary>
                            <ul className="mt-2 space-y-1">
                              {(result.failures ?? []).map((failure) => (
                                <li key={failure.key} className="text-caption">
                                  <span className="font-mono">{failure.key}</span>
                                  {`: ${failure.title}`}
                                  {failure.task ? (
                                    <span className="ml-1 text-fg-muted">
                                      {t("organization.health.task", { task: failure.task })}
                                    </span>
                                  ) : null}
                                </li>
                              ))}
                            </ul>
                          </details>
                        ) : (
                          "—"
                        )}
                      </TableCell>
                    </TableRow>
                  ))
                )}
              </TableBody>
            )}
          </Table>
        </>
      )}
    </section>
  );
}

