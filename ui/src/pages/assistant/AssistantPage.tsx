import { useMemo, useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, forPeople, unwrap, reasonOf } from "../../api/client";
import { rememberRun, requestOpen } from "../../assistant/state";
import {
  Badge,
  Button,
  Card,
  Checkbox,
  EmptyState,
  PageHeader,
  Select,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeaderCell,
  TableRow,
} from "../../components/ui";
import { ListFailed } from "../../components/forms/widgets/ListFailed";
import { usePermissions } from "../../api/permissions";
import { appDisplayName, useEndpointTitles } from "../apps/appTitle";
import { TERMINAL_STATES } from "../apps/useAgentRun";
import { AgentAccess } from "./AgentAccess";

interface RunRecord {
  id: string;
  project: string;
  appName?: string;
  title?: string | null;
  endpointName?: string;
  kind?: string;
  unattended?: boolean;
  continues?: string | null;
  /** `journey` for a run of the Portal's own live journeys (AG-93). */
  origin?: string;
  status: string;
  prompt: string;
  createdAt: string;
  firstFrameMs?: number | null;
  firstVersionMs?: number | null;
  error?: string;
}

/** An application reads by its title, or its name as words; a conversation by its first message. */
function formatTitle(run: RunRecord, endpointTitles: Map<string, string>): string {
  if (run.kind && run.kind !== "conversation" && (run.title || run.appName)) {
    return appDisplayName({
      title: run.title,
      appName: run.appName,
      endpointTitle: run.endpointName ? endpointTitles.get(run.endpointName) : undefined,
    });
  }
  return run.prompt.length > 80 ? `${run.prompt.slice(0, 80)}…` : run.prompt;
}

const RUNS_PER_PAGE = 20;

/** One App's runs, the newest first; a conversation stands alone (T-4994). */
interface RunGroup {
  key: string;
  runs: RunRecord[];
}

/** Groups `runs` (newest first) by the App they built, keeping the order of each group's newest run. */
export function groupRuns(runs: RunRecord[]): RunGroup[] {
  const groups = new Map<string, RunGroup>();
  for (const run of runs) {
    const app = run.kind && run.kind !== "conversation" ? (run.appName ?? run.title) : null;
    const key = app ? `${run.kind}:${app}` : `run:${run.id}`;
    const group = groups.get(key);
    if (group) group.runs.push(run);
    else groups.set(key, { key, runs: [run] });
  }
  return [...groups.values()];
}

/**
 * The central assistant page listing conversations and autonomous work runs (UI-54, AG-71).
 *
 * Displays all visible runs newest first with lifecycle state, performance timings, and continuation
 * links. Supports interactive filtering by kind, state, and ownership ("mine"), and opens active or
 * past runs into the assistant panel. Work starts from the assistant's paths, never from a form here
 * (UI-55, T-2745).
 */
/** Milliseconds as seconds with one decimal, in the person's language (T-3278). */
function seconds(ms: number, locale: string): string {
  return new Intl.NumberFormat(locale, { minimumFractionDigits: 1, maximumFractionDigits: 1 }).format(ms / 1000);
}

export function AssistantPage({ project }: { project: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const mayAsk = usePermissions(project).can("*", "propose");
  const endpointTitles = useEndpointTitles(project);
  const queryClient = useQueryClient();

  const [kindFilter, setKindFilter] = useState<string>("");
  const [statusFilter, setStatusFilter] = useState<string>("all");
  // One page of runs at a time (T-2760): every conversation ever held, listed at once, made the
  // page ten thousand pixels tall.
  const [shown, setShown] = useState(RUNS_PER_PAGE);
  const [mineFilter, setMineFilter] = useState<boolean>(false);
  // The live journeys' test runs are left out unless asked for (AG-93, T-2816): they crowded the
  // history people read. Nothing is deleted; this only asks the server for them too.
  const [testRuns, setTestRuns] = useState<boolean>(false);

  const runsKey = ["projects", project, "agent-runs", { kind: kindFilter, mine: mineFilter, testRuns }];

  const runsQuery = useQuery({
    queryKey: runsKey,
    queryFn: async () => {
      const result = await api.GET("/api/v1/projects/{project}/agent-runs", {
        params: {
          path: { project },
          query: {
            limit: 100,
            kind: kindFilter || undefined,
            mine: mineFilter || undefined,
            origin: testRuns ? "all" : undefined,
          },
        },
      });
      return unwrap(result);
    },
    refetchInterval: (query) => {
      const items = (query.state.data?.items ?? []) as RunRecord[];
      const anyLive = items.some((item) => !TERMINAL_STATES.includes(item.status));
      return anyLive ? 5000 : false;
    },
  });

  const filteredRuns = useMemo(() => {
    const runs = (runsQuery.data?.items ?? []) as RunRecord[];
    return runs.filter((run) => {
      if (statusFilter === "live") {
        return !TERMINAL_STATES.includes(run.status);
      }
      if (statusFilter === "ended") {
        return TERMINAL_STATES.includes(run.status);
      }
      return true;
    });
  }, [runsQuery.data, statusFilter]);

  const groups = useMemo(() => groupRuns(filteredRuns), [filteredRuns]);
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const toggle = (key: string) =>
    setExpanded((now) => {
      const next = new Set(now);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });

  const continueMutation = useMutation({
    mutationFn: async (priorRunId: string) => {
      const result = await api.POST("/api/v1/projects/{project}/assistant/conversations", {
        params: { path: { project } },
        body: { message: t("assistantPage.continueMessage"), continues: priorRunId },
      });
      return unwrap(result);
    },
    onSuccess: (created) => {
      rememberRun({ project, runId: created.id });
      requestOpen();
      void queryClient.invalidateQueries({ queryKey: ["projects", project, "agent-runs"] });
    },
  });

  const secs = (ms: number) => t("agentRun.timing.seconds", { seconds: seconds(ms, i18n.language) });
  /** "First frame 38.8 s · First version 44.0 s" on one line, one figure when both are the same. */
  const timing = (frame: number | null | undefined, version: number | null | undefined): string => {
    if (frame != null && version != null && seconds(frame, i18n.language) === seconds(version, i18n.language)) {
      return `${t("assistantPage.timing.both")} ${secs(frame)}`;
    }
    return [
      frame != null ? `${t("agentRun.timing.firstFrame")} ${secs(frame)}` : null,
      version != null ? `${t("agentRun.timing.firstVersion")} ${secs(version)}` : null,
    ]
      .filter(Boolean)
      .join(" · ");
  };

  const handleOpen = (run: RunRecord) => {
    rememberRun({ project, runId: run.id });
    requestOpen();
  };

  return (
    <div className="space-y-6">
      <PageHeader
        title={t("assistantPage.title")}
        description={t("assistantPage.lead")}
      />

      <Card padding="md" className="flex flex-wrap items-center justify-between gap-4">
        <div className="flex flex-wrap items-center gap-4">
          <div className="flex items-center gap-2">
            <label htmlFor="filter-kind" className="text-caption font-medium text-fg-muted">
              {t("assistantPage.filters.kind")}
            </label>
            <Select
              id="filter-kind"
              aria-label={t("assistantPage.filters.kind")}
              value={kindFilter}
              onChange={(e) => setKindFilter(e.target.value)}
              className="w-40"
            >
              <option value="">{t("assistantPage.filters.all")}</option>
              <option value="conversation">{t("assistantPage.kinds.conversation")}</option>
              <option value="application">{t("assistantPage.kinds.application")}</option>
              <option value="dashboard">{t("assistantPage.kinds.dashboard")}</option>
              <option value="analysis">{t("assistantPage.kinds.analysis")}</option>
            </Select>
          </div>

          <div className="flex items-center gap-2">
            <label htmlFor="filter-status" className="text-caption font-medium text-fg-muted">
              {t("assistantPage.filters.status")}
            </label>
            <Select
              id="filter-status"
              aria-label={t("assistantPage.filters.status")}
              value={statusFilter}
              onChange={(e) => setStatusFilter(e.target.value)}
              className="w-32"
            >
              <option value="all">{t("assistantPage.filters.all")}</option>
              <option value="live">{t("assistantPage.filters.live")}</option>
              <option value="ended">{t("assistantPage.filters.ended")}</option>
            </Select>
          </div>

          {/* Was a bare input reimplementing the size, the tick colour and the focus treatment
              by hand, with `focus:ring-*` rather than the shared `focus-ring` utility — so this
              one filter had a different keyboard focus from every control beside it. */}
          <Checkbox
            checked={mineFilter}
            onChange={(e) => setMineFilter(e.target.checked)}
            label={t("assistantPage.filters.mine")}
          />
          <Checkbox
            checked={testRuns}
            onChange={(e) => setTestRuns(e.target.checked)}
            label={t("assistantPage.filters.testRuns")}
          />
        </div>
      </Card>

      {runsQuery.isPending ? (
        <p role="status" className="text-body text-fg-muted">
          {t("app.loading")}
        </p>
      ) : runsQuery.isError ? (
        // `isError` was never checked, so a failed request fell through to the empty state and
        // told the person, confidently and wrongly, that their conversations and unattended runs
        // were gone — with no reason and nothing to press.
        <ListFailed
          what={t("assistantPage.title")}
          reason={reasonOf(runsQuery.error, t("app.error.generic"))}
          onRetry={() => void runsQuery.refetch()}
        />
      ) : filteredRuns.length === 0 ? (
        // The way to the first run is the assistant itself (T-2745 took the New work form away).
        <EmptyState
          icon="chat"
          title={t("assistantPage.empty")}
          action={<Button onClick={() => requestOpen()}>{t("assistant.open")}</Button>}
        />
      ) : (
        <>
        <Table caption={t("assistantPage.title")}>
          <TableHead>
            <TableHeaderCell>{t("assistantPage.table.name")}</TableHeaderCell>
            <TableHeaderCell>{t("assistantPage.filters.status")}</TableHeaderCell>
            <TableHeaderCell className="hidden md:table-cell">
              {t("assistantPage.timings")}
            </TableHeaderCell>
            <TableHeaderCell className="hidden sm:table-cell">
              {t("assistantPage.started")}
            </TableHeaderCell>
            <TableHeaderCell align="right">{t("assistantPage.actions")}</TableHeaderCell>
          </TableHead>
          <TableBody>
            {groups.slice(0, shown).flatMap((group) =>
              (expanded.has(group.key) ? group.runs : group.runs.slice(0, 1)).map((run, index) => ({ group, run, index })),
            ).map(({ group, run, index }) => {
              const ended = TERMINAL_STATES.includes(run.status);
              const isEndedConversation = run.kind === "conversation" && ended;
              const title = formatTitle(run, endpointTitles);
              return (
                <TableRow key={run.id}>
                  <TableCell className="max-w-[28rem]">
                    <div className={`flex min-w-0 flex-col gap-0.5${index > 0 ? " border-l-2 border-border pl-3" : ""}`}>
                      <span className="truncate font-semibold text-fg" title={title}>
                        {title}
                      </span>
                      <span className="text-caption text-fg-muted">
                        <span>
                          {t(`assistantPage.kinds.${run.kind ?? "conversation"}`, {
                            defaultValue: run.kind,
                          })}
                        </span>
                        {run.continues ? (
                          <> · <span className="italic">{t("assistantPage.continues")}</span></>
                        ) : null}
                        {run.origin === "journey" ? (
                          <>
                            {" · "}
                            <Badge>{t("assistantPage.testRun")}</Badge>
                          </>
                        ) : null}
                      </span>
                      {index === 0 && group.runs.length > 1 ? (
                        <Button
                          variant="ghost"
                          size="sm"
                          className="self-start px-0 underline"
                          aria-expanded={expanded.has(group.key)}
                          onClick={() => toggle(group.key)}
                        >
                          {expanded.has(group.key)
                            ? t("assistantPage.group.latestOnly")
                            : t("assistantPage.group.all", { count: group.runs.length })}
                        </Button>
                      ) : null}
                    </div>
                  </TableCell>
                  <TableCell>
                    <Badge
                      tone={run.status === "failed" ? "danger" : ended ? "neutral" : "primary"}
                    >
                      {ended ? null : (
                        <span aria-hidden className="size-1.5 rounded-full bg-current" />
                      )}
                      {t(`assistantPage.states.${run.status}`, {
                        defaultValue: t(`agentRun.states.${run.status}`, {
                          defaultValue: run.status,
                        }),
                      })}
                    </Badge>
                    {/* `error` is declared on every run record and was rendered nowhere: the
                        only signal of a failed run was the Badge's colour, so someone whose
                        unattended build failed could see THAT it failed and nothing about why,
                        on the page whose job is to report it. */}
                    {run.status === "failed" && run.error ? (
                      <p className="mt-1 max-w-prose text-caption text-danger">{forPeople(run.error)}</p>
                    ) : null}
                  </TableCell>
                  <TableCell className="hidden whitespace-nowrap text-caption text-fg-muted md:table-cell">
                    {run.firstFrameMs != null || run.firstVersionMs != null ? (
                      <span className="tabular-nums">{timing(run.firstFrameMs, run.firstVersionMs)}</span>
                    ) : (
                      "—"
                    )}
                  </TableCell>
                  <TableCell className="hidden whitespace-nowrap text-caption text-fg-muted sm:table-cell">
                    {new Date(run.createdAt).toLocaleString(i18n.language, {
                      dateStyle: "medium",
                      timeStyle: "short",
                    })}
                  </TableCell>
                  <TableCell align="right">
                    <div className="flex items-center justify-end gap-2">
                      {isEndedConversation ? (
                        // Continuing a conversation starts another run: propose on any kind (AG-67, UI-44).
                        <Button
                          size="sm"
                          variant="secondary"
                          loading={
                            continueMutation.isPending && continueMutation.variables === run.id
                          }
                          disabled={continueMutation.isPending || !mayAsk}
                          disabledReason={mayAsk ? t("app.busyReason") : t("assistant.readOnly")}
                          onClick={() => continueMutation.mutate(run.id)}
                        >
                          {t("assistantPage.continue")}
                        </Button>
                      ) : null}
                      <Button size="sm" variant="secondary" onClick={() => handleOpen(run)}>
                        {t("assistantPage.open")}
                      </Button>
                    </div>
                  </TableCell>
                </TableRow>
              );
            })}
          </TableBody>
        </Table>
        {groups.length > shown ? (
          <div className="mt-3 flex justify-center">
            <Button onClick={() => setShown((count) => count + RUNS_PER_PAGE)}>
              {t("assistantPage.showMore", { count: Math.min(RUNS_PER_PAGE, groups.length - shown), left: groups.length - shown })}
            </Button>
          </div>
        ) : null}
        </>
      )}

      <AgentAccess project={project} />
    </div>
  );
}
