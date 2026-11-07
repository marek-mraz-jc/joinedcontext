import { useState } from "react";
import type { JSX } from "react";
import {
  useInfiniteQuery,
  useMutation,
  useQueryClient,
} from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, unwrap } from "../../api/client";
import i18n from "../../i18n";
import type { components } from "../../api/schema";
import {
  Alert,
  Badge,
  Button,
  Card,
  ConfirmDialog,
  EmptyState,
  Field,
  Input,
  Select,
  TableSkeleton,
} from "../../components/ui";
import { PageFailed } from "../../components/ui/PageState";
import { AlertDialog } from "../../components/AlertDialog";

type Activity = components["schemas"]["AgentActivity"];
type ActivityRun = components["schemas"]["ActivityRun"];
type ActivityStep = components["schemas"]["ActivityStep"];
type AgentAlert = components["schemas"]["AgentAlert"];

/** Which runs the tab is about: an agent profile's, or one run. */
export interface ActivityScope {
  project?: string;
  profile?: string;
  run?: string;
}

interface Filters {
  person: string;
  space: string;
  verdict: "" | "ok" | "refused";
  since: string;
}

const NO_FILTERS: Filters = { person: "", space: "", verdict: "", since: "" };

function when(value: string | null | undefined): string {
  return value
    ? new Date(value).toLocaleString(i18n.resolvedLanguage ?? "en", {
        dateStyle: "medium",
        timeStyle: "short",
      })
    : "";
}

function seconds(ms: number | null | undefined): string | null {
  return typeof ms === "number"
    ? `${Math.max(0, Math.round(ms / 1000))} s`
    : null;
}

/**
 * An agent's Activity tab (AG-134…AG-136, T-4851): its runs newest first with each tool call,
 * its verdict and the problem text of a refusal, the writes with their reasons, tokens and cost;
 * the alerts above them; Stop on a run in flight. Only runs the person may see arrive.
 */
/** "Alert me…" for one agent profile once its activity loaded: greyed with the reason when the person approves in no project. */
export function AgentAlertControl({
  profile,
  ready,
  projects = [],
}: {
  profile: string | undefined;
  ready: boolean;
  projects: string[] | undefined;
}) {
  const { t } = useTranslation();
  const [alerting, setAlerting] = useState(false);
  if (profile === undefined || !ready) return null;
  const none = projects.length === 0;
  return (
    <>
      <div className="flex flex-wrap items-center gap-2">
        <Button
          variant="secondary"
          disabled={none}
          disabledReason={none ? t("organization.agents.activity.alertMeNone") : undefined}
          onClick={() => setAlerting(true)}
        >
          {t("organization.agents.activity.alertMe")}
        </Button>
        <p className="text-caption text-fg-muted">
          {none
            ? t("organization.agents.activity.alertMeNone")
            : t("organization.agents.activity.alertMeHint")}
        </p>
      </div>
      {alerting && projects[0] ? (
        <AlertDialog
          project={projects[0]}
          projects={projects}
          scope="agent"
          target={profile}
          label={profile}
          onClose={() => setAlerting(false)}
        />
      ) : null}
    </>
  );
}

export function AgentActivity({
  scope,
}: {
  scope: ActivityScope;
}): JSX.Element {
  const { t } = useTranslation();
  const [draft, setDraft] = useState<Filters>(NO_FILTERS);
  const [filters, setFilters] = useState<Filters>(NO_FILTERS);
  const activity = useInfiniteQuery({
    queryKey: ["agentActivity", scope, filters],
    retry: false,
    initialPageParam: undefined as string | undefined,
    getNextPageParam: (last: Activity) => last.nextCursor ?? undefined,
    queryFn: async ({ pageParam }): Promise<Activity> =>
      unwrap(
        await api.GET("/api/v1/organization/agents/activity", {
          params: {
            query: {
              project: scope.project,
              profile: scope.profile,
              run: scope.run,
              person: filters.person || undefined,
              space: filters.space || undefined,
              verdict: filters.verdict || undefined,
              since: filters.since
                ? new Date(`${filters.since}T00:00:00`).toISOString()
                : undefined,
              cursor: pageParam,
            },
          },
        }),
      ),
  });
  const runs = activity.data?.pages.flatMap((page) => page.runs) ?? [];
  const alerts = activity.data?.pages[0]?.alerts ?? [];
  const choices = activity.data?.pages[0]?.choices ?? { people: [], spaces: [], alertProjects: [] };
  const denied =
    activity.error instanceof ApiError && activity.error.status === 403
      ? (activity.error.problem?.detail ?? activity.error.message)
      : null;
  const filtered = Object.values(filters).some((value) => value !== "");
  return (
    <section className="space-y-4" aria-labelledby="agent-activity-heading">
      <h2
        id="agent-activity-heading"
        className="text-title font-semibold text-fg"
      >
        {t("organization.agents.activity.title")}
      </h2>
      <AgentAlertControl
        profile={scope.profile}
        ready={activity.isSuccess}
        projects={choices.alertProjects}
      />
      {scope.run === undefined ? (
        <form
          className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-4"
          aria-label={t("organization.agents.activity.filters")}
          onSubmit={(event) => {
            event.preventDefault();
            setFilters(draft);
          }}
        >
          <Field
            id="activity-person"
            label={t("organization.agents.activity.person")}
            help={t("organization.agents.activity.personHelp")}
          >
            <Select
              id="activity-person"
              value={draft.person}
              onChange={(e) => setDraft({ ...draft, person: e.target.value })}
            >
              <option value="">
                {t("organization.agents.activity.personAny")}
              </option>
              {choices.people.map((value) => (
                <option key={value} value={value}>
                  {value}
                </option>
              ))}
            </Select>
          </Field>
          <Field
            id="activity-space"
            label={t("organization.agents.activity.space")}
            help={t("organization.agents.activity.spaceHelp")}
          >
            <Select
              id="activity-space"
              value={draft.space}
              onChange={(e) => setDraft({ ...draft, space: e.target.value })}
            >
              <option value="">
                {t("organization.agents.activity.spaceAny")}
              </option>
              {choices.spaces.map((value) => (
                <option key={value} value={value}>
                  {value}
                </option>
              ))}
            </Select>
          </Field>
          <Field
            id="activity-verdict"
            label={t("organization.agents.activity.verdict")}
            help={t("organization.agents.activity.verdictHelp")}
          >
            <Select
              id="activity-verdict"
              value={draft.verdict}
              onChange={(e) =>
                setDraft({
                  ...draft,
                  verdict: e.target.value as Filters["verdict"],
                })
              }
            >
              <option value="">
                {t("organization.agents.activity.verdictAny")}
              </option>
              <option value="refused">
                {t("organization.agents.activity.verdictRefused")}
              </option>
              <option value="ok">
                {t("organization.agents.activity.verdictOk")}
              </option>
            </Select>
          </Field>
          <Field
            id="activity-since"
            label={t("organization.agents.activity.since")}
            help={t("organization.agents.activity.sinceHelp")}
          >
            <Input
              id="activity-since"
              type="date"
              value={draft.since}
              onChange={(e) => setDraft({ ...draft, since: e.target.value })}
            />
          </Field>
          <div className="flex gap-2 sm:col-span-2 lg:col-span-4">
            <Button type="submit">
              {t("organization.agents.activity.apply")}
            </Button>
            {filtered ? (
              <Button
                type="button"
                variant="secondary"
                onClick={() => {
                  setDraft(NO_FILTERS);
                  setFilters(NO_FILTERS);
                }}
              >
                {t("organization.agents.activity.clear")}
              </Button>
            ) : null}
          </div>
        </form>
      ) : null}
      {alerts.length > 0 ? (
        <ul
          className="space-y-2"
          aria-label={t("organization.agents.activity.alerts")}
        >
          {alerts.map((alert) => (
            <li
              key={`${alert.kind}-${alert.project}-${alert.profile}-${alert.runId ?? ""}`}
            >
              <Alert tone="warning">{alertText(t, alert)}</Alert>
            </li>
          ))}
        </ul>
      ) : null}
      {denied !== null ? (
        <Alert tone="info">{denied}</Alert>
      ) : activity.isError ? (
        <PageFailed
          error={activity.error}
          onRetry={() => void activity.refetch()}
        />
      ) : activity.isPending ? (
        <TableSkeleton rows={3} columns={3} />
      ) : runs.length === 0 ? (
        <EmptyState
          icon="search"
          title={t(
            filtered
              ? "organization.agents.activity.noneFiltered"
              : "organization.agents.activity.none",
          )}
          description={t("organization.agents.activity.noneHint")}
        />
      ) : (
        <ol
          className="space-y-3"
          aria-label={t("organization.agents.activity.runs")}
        >
          {runs.map((run) => (
            <li key={run.id}>
              <RunCard run={run} />
            </li>
          ))}
        </ol>
      )}
      {activity.hasNextPage ? (
        <Button
          variant="secondary"
          aria-busy={activity.isFetchingNextPage}
          onClick={() => {
            if (!activity.isFetchingNextPage) void activity.fetchNextPage();
          }}
        >
          {t("organization.agents.activity.more")}
        </Button>
      ) : null}
    </section>
  );
}

type Translate = ReturnType<typeof useTranslation>["t"];

function alertText(t: Translate, alert: AgentAlert): string {
  return t(`organization.agents.activity.alert.${alert.kind}`, {
    count: alert.count,
    profile: alert.profile,
    project: alert.project,
    run: alert.runTitle ?? alert.runId ?? "",
    when: when(alert.at),
  });
}

const STATUS_TONE: Record<string, "success" | "warning" | "danger" | "info"> = {
  published: "success",
  failed: "danger",
  cancelled: "warning",
  expired: "warning",
};

function RunCard({ run }: { run: ActivityRun }): JSX.Element {
  const { t } = useTranslation();
  const duration = seconds(run.durationMs);
  const facts: [string, string][] = [
    [t("organization.agents.activity.startedBy"), run.startedBy],
    [t("organization.agents.activity.started"), when(run.createdAt)],
    ...(duration
      ? [
          [t("organization.agents.activity.duration"), duration] as [
            string,
            string,
          ],
        ]
      : []),
    [
      t("organization.agents.activity.tokens"),
      run.tokens.toLocaleString(i18n.resolvedLanguage ?? "en"),
    ],
    [
      t("organization.agents.activity.cost"),
      typeof run.costUsd === "number"
        ? `$${run.costUsd.toFixed(4)}`
        : t("organization.agents.activity.costUnreported"),
    ],
    [
      t("organization.agents.activity.spaces"),
      run.spaces.length > 0 ? run.spaces.join(", ") : "–",
    ],
  ];
  return (
    <Card className="space-y-3">
      <div className="flex flex-wrap items-start justify-between gap-2">
        <div className="min-w-0">
          <h3 className="text-body font-semibold text-fg break-words">
            {run.title}
          </h3>
          <p className="text-caption text-fg-muted">
            {run.project} · {run.profile}
          </p>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <Badge tone={STATUS_TONE[run.status] ?? "info"}>{run.status}</Badge>
          {run.refusals > 0 ? (
            <Badge tone="danger">
              {t("organization.agents.activity.refusals", {
                count: run.refusals,
              })}
            </Badge>
          ) : null}
          {run.writes > 0 ? (
            <Badge tone="accent">
              {t("organization.agents.activity.writes", { count: run.writes })}
            </Badge>
          ) : null}
          {run.mayStop ? <StopRun run={run} /> : null}
        </div>
      </div>
      <dl className="grid grid-cols-2 gap-x-6 gap-y-1 sm:grid-cols-3">
        {facts.map(([label, value]) => (
          <div key={label}>
            <dt className="text-caption text-fg-muted">{label}</dt>
            <dd className="text-body text-fg">{value}</dd>
          </div>
        ))}
      </dl>
      <details>
        <summary className="cursor-pointer text-body text-fg">
          {t("organization.agents.activity.steps", { count: run.steps.length })}
        </summary>
        {run.steps.length === 0 ? (
          <p className="mt-2 text-body text-fg-muted">
            {t("organization.agents.activity.noSteps")}
          </p>
        ) : (
          <ol className="mt-2 space-y-2">
            {run.steps.map((step, index) => (
              <li key={`${step.at}-${index}`}>
                <Step step={step} />
              </li>
            ))}
          </ol>
        )}
      </details>
    </Card>
  );
}

function Step({ step }: { step: ActivityStep }): JSX.Element {
  const { t } = useTranslation();
  const refused = step.verdict === "refused";
  return (
    <div className="rounded border border-border p-2">
      <div className="flex flex-wrap items-center gap-2">
        <Badge tone={refused ? "danger" : "success"}>
          {t(`organization.agents.activity.verdictOf.${step.verdict}`)}
        </Badge>
        {step.write ? (
          <Badge tone="accent">{t("organization.agents.activity.write")}</Badge>
        ) : null}
        <code className="text-caption break-all">{step.tool}</code>
        {step.target ? (
          <span className="text-caption text-fg-muted break-all">
            → {step.target}
          </span>
        ) : null}
        <time className="text-caption text-fg-muted" dateTime={step.at}>
          {when(step.at)}
        </time>
      </div>
      {step.reason ? (
        <p className="mt-1 text-body text-fg break-words">
          {t(
            refused
              ? "organization.agents.activity.refusedBecause"
              : "organization.agents.activity.reason",
            { text: step.reason },
          )}
        </p>
      ) : null}
    </div>
  );
}

/** Stop: ends a run in flight; the run's stream records who stopped it (AG-135). */
function StopRun({ run }: { run: ActivityRun }): JSX.Element {
  const { t } = useTranslation();
  const client = useQueryClient();
  const [open, setOpen] = useState(false);
  const stop = useMutation({
    mutationFn: async () =>
      unwrap(
        await api.POST("/api/v1/projects/{project}/agent-runs/{id}/cancel", {
          params: { path: { project: run.project, id: run.id } },
        }),
      ),
    onSuccess: async () => {
      setOpen(false);
      await Promise.all([
        client.invalidateQueries({ queryKey: ["agentActivity"] }),
        client.invalidateQueries({ queryKey: ["organizationAgents"] }),
      ]);
    },
  });
  return (
    <>
      <Button variant="danger" size="sm" onClick={() => setOpen(true)}>
        {t("organization.agents.activity.stop")}
      </Button>
      <ConfirmDialog
        open={open}
        onOpenChange={setOpen}
        title={t("organization.agents.activity.stopTitle", {
          title: run.title,
        })}
        description={t("organization.agents.activity.stopHint")}
        confirmLabel={t("organization.agents.activity.stopConfirm")}
        tone="danger"
        pending={stop.isPending}
        onConfirm={() => stop.mutate()}
      >
        {stop.isError ? (
          <Alert tone="danger">
            {stop.error instanceof ApiError
              ? (stop.error.problem?.detail ?? stop.error.message)
              : t("organization.agents.activity.stopFailed")}
          </Alert>
        ) : null}
      </ConfirmDialog>
    </>
  );
}
