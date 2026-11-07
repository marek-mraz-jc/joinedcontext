import type { JSX, ReactNode } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, forPeople, queryKeys, unwrap } from "../../api/client";
import { approvalStanding, changedKind } from "../../api/approval";
import { usePermissions } from "../../api/permissions";
import { useIdentity } from "../../auth/AuthProvider";
import { PlanDiffViewer } from "../../components/diff/PlanDiffViewer";
import { Button, Card, InlineError } from "../../components/ui";
import { Icon } from "../../components/ui/icons";
import { PortalLink } from "./Prose";
import type { OpenLink } from "./Prose";
import { isRecord, QueryAnswer, viewOf } from "./QueryResultCard";

/**
 * A Change the assistant proposed, followed in the conversation to a result that works (T-2774,
 * AG-87): proposed, approved (by whom, and here, beside its field changes, when the person may), live, and for an endpoint
 * tested by reading a few of its rows with the person's own session. The assistant never
 * approves (AG-11); the button is the person's.
 */
export interface FollowedChange {
  changeId: string;
  kind: string;
  name: string;
}

/** The Change a successful step proposed, or none: only an id the Portal mints is followed. */
export function followedChangeOf(payload: Record<string, unknown>): FollowedChange | null {
  const output = payload.output;
  if (payload.status !== "ok" || !isRecord(output) || typeof output.changeId !== "string") {
    return null;
  }
  if (!/^chg-[0-9a-z]{1,32}$/.test(output.changeId)) {
    return null;
  }
  const change = isRecord(output.change) ? output.change : {};
  const params = isRecord(change.summary) && isRecord(change.summary.params) ? change.summary.params : {};
  const input = isRecord(payload.input) ? payload.input : {};
  const text = (value: unknown) => (typeof value === "string" ? value : "");
  return {
    changeId: output.changeId,
    kind: text(params.kind) || text(input.kind),
    name: text(params.name) || text(input.name),
  };
}

/** Where a kind's own page is, for the kinds that have one. */
const PAGE: Record<string, string> = {
  Endpoint: "endpoints",
  Pipeline: "pipelines",
  ContextSpace: "spaces",
  DataSource: "datasources",
  Subscription: "subscriptions",
  App: "apps",
  Dashboard: "dashboards",
};

const pageOf = (project: string, kind: string): string | null =>
  Object.hasOwn(PAGE, kind) ? `/projects/${project}/${PAGE[kind]}` : kind === "DataModel" ? `/projects/${project}/models` : null;

type Stage = "done" | "working" | "waiting" | "failed" | "later";

function Step({ stage, title, children }: { stage: Stage; title: string; children?: ReactNode }): JSX.Element {
  const { t } = useTranslation();
  const icon = stage === "done" ? "check" : stage === "failed" ? "close" : null;
  return (
    <li className="flex gap-2">
      <span
        role="img"
        aria-label={t(`agentRun.follow.stage.${stage}`)}
        className={
          stage === "done"
            ? "mt-0.5 flex size-4 shrink-0 items-center justify-center rounded-full bg-success-soft text-success"
            : stage === "failed"
              ? "mt-0.5 flex size-4 shrink-0 items-center justify-center rounded-full bg-danger-soft text-danger"
              : stage === "later"
                ? "mt-0.5 size-4 shrink-0 rounded-full border border-border"
                : "mt-0.5 size-4 shrink-0 motion-safe:animate-pulse rounded-full border-2 border-primary"
        }
      >
        {icon ? <Icon name={icon} className="size-3" /> : null}
      </span>
      <div className="min-w-0 flex-1 space-y-1">
        <p className={stage === "later" ? "text-fg-muted" : "font-medium"}>{title}</p>
        {children}
      </div>
    </li>
  );
}

/** A few rows of a live endpoint, read with the person's own session (AG-87, T-2774). */
async function tryEndpoint(slug: string): Promise<{ type: string | null; rows: unknown[]; total: number | null }> {
  const base = `/api/endpoint/${encodeURIComponent(slug)}/ngsi-ld/v1`;
  const read = async (path: string): Promise<Response> => {
    const response = await fetch(`${base}${path}`, { headers: { Accept: "application/json" }, credentials: "same-origin" });
    if (!response.ok) {
      let detail = "";
      try {
        const problem = (await response.json()) as { detail?: unknown; title?: unknown };
        detail = typeof problem.detail === "string" ? problem.detail : typeof problem.title === "string" ? problem.title : "";
      } catch {
        // A body that is not a problem: the status says enough.
      }
      throw new ApiError(response.status, detail || `HTTP ${response.status}`);
    }
    return response;
  };
  const listed: unknown = await (await read("/types")).json();
  const types = (isRecord(listed) && Array.isArray(listed.typeList) ? listed.typeList : Array.isArray(listed) ? listed : [])
    .map((entry: unknown) => (typeof entry === "string" ? entry : isRecord(entry) ? entry.typeName ?? entry.id : undefined))
    .filter((entry: unknown): entry is string => typeof entry === "string" && entry !== "");
  const type = types[0] ?? null;
  if (type === null) {
    return { type, rows: [], total: 0 };
  }
  const response = await read(`/entities?type=${encodeURIComponent(type)}&limit=5&count=true`);
  const rows: unknown = await response.json();
  // NGSI-LD names the header `NGSILD-Results-Count`; some brokers spell it with the dash.
  const header = response.headers.get("NGSILD-Results-Count") ?? response.headers.get("NGSI-LD-Results-Count");
  const counted = header === null ? null : Number(header);
  return {
    type,
    rows: Array.isArray(rows) ? rows : [],
    total: counted !== null && Number.isInteger(counted) && counted >= 0 ? counted : null,
  };
}

function EndpointTest({ project, name, onOpenLink }: { project: string; name: string; onOpenLink?: OpenLink }): JSX.Element {
  const { t, i18n } = useTranslation();
  const endpoint = useQuery({
    queryKey: ["projects", project, "endpoints", name],
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}/{name}", {
          params: { path: { project, plural: "endpoints", name } },
        }),
      ),
  });
  const spec = isRecord(endpoint.data) && isRecord(endpoint.data.spec) ? endpoint.data.spec : {};
  const slug = typeof spec.slug === "string" ? spec.slug : null;
  const tried = useQuery({
    queryKey: ["endpoint-try", slug],
    enabled: slug !== null,
    retry: false,
    queryFn: () => tryEndpoint(slug ?? ""),
  });
  const failed = endpoint.error ?? tried.error;
  if (failed) {
    return (
      <Step stage="failed" title={t("agentRun.follow.test.failed")}>
        <InlineError>
          {failed instanceof ApiError ? (failed.problem?.detail ?? failed.message) : t("app.error.generic")}
        </InlineError>
        <Button size="xs" onClick={() => void (endpoint.error ? endpoint.refetch() : tried.refetch())}>
          {t("agentRun.follow.test.again")}
        </Button>
      </Step>
    );
  }
  if (!tried.data) {
    return <Step stage="working" title={t("agentRun.follow.test.reading", { name })} />;
  }
  const { type, rows, total } = tried.data;
  const explore = `/projects/${project}/explore?endpoint=${encodeURIComponent(name)}${type ? `&type=${encodeURIComponent(type)}` : ""}`;
  return (
    <Step
      stage="done"
      title={
        rows.length === 0
          ? t("agentRun.follow.test.empty", { name })
          : t("agentRun.follow.test.read", { count: rows.length, total: total ?? rows.length, type: type ?? "", name })
      }
    >
      {rows.length > 0 ? <QueryAnswer view={viewOf(rows, i18n.resolvedLanguage ?? i18n.language)} /> : null}
      <PortalLink href={explore} onOpenLink={onOpenLink}>
        {t("agentRun.follow.test.explore")}
      </PortalLink>
    </Step>
  );
}

export function ChangeFollowCard({
  project,
  followed,
  onOpenLink,
}: {
  project: string;
  followed: FollowedChange;
  onOpenLink?: OpenLink;
}): JSX.Element {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const identity = useIdentity();
  const permissions = usePermissions(project);
  const { changeId } = followed;
  const change = useQuery({
    queryKey: queryKeys.change(project, changeId),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/changes/{id}", {
          params: { path: { project, id: changeId } },
        }),
      ),
    // Somebody else may approve it at any time; a deploy takes seconds.
    refetchInterval: (query) => {
      const phase = query.state.data?.status.phase;
      return phase === "PendingApproval" ? 10_000 : phase === "Deploying" || phase === "Merged" ? 3_000 : false;
    },
  });
  const approve = useMutation({
    mutationFn: async () =>
      unwrap(
        await api.POST("/api/v1/projects/{project}/changes/{id}/approve", {
          params: { path: { project, id: changeId } },
        }),
      ),
    onSuccess: (approved) => {
      queryClient.setQueryData(queryKeys.change(project, changeId), (prev: unknown) =>
        prev && typeof prev === "object" ? { ...prev, status: approved.status } : prev,
      );
      void queryClient.invalidateQueries({ queryKey: queryKeys.changes(project), exact: true });
    },
  });

  const proposal = change.data;
  const kind = followed.kind || (proposal ? changedKind(proposal) : "");
  const phase = proposal?.status.phase;
  const approved = phase === "Deploying" || phase === "Merged" || phase === "Applied";
  // Nothing is said about who approves until the permissions are read: "needs a role" would flash first.
  const standing =
    proposal && permissions.data ? approvalStanding(permissions, identity?.email ?? undefined, proposal) : null;
  const red = proposal?.status.lane === "red";
  const review = `/projects/${project}/approvals/${changeId}`;
  const page = pageOf(project, kind);

  return (
    <Card as="section" padding="sm" aria-label={t("agentRun.follow.title", { name: followed.name || changeId })} className="text-sm">
      <ol className="space-y-2" aria-live="polite">
        <Step stage="done" title={t("agentRun.follow.proposed", { kind: kind || t("agentRun.follow.aChange"), name: followed.name })}>
          <PortalLink href={review} onOpenLink={onOpenLink}>
            {changeId}
          </PortalLink>
        </Step>

        {change.error ? (
          <Step stage="failed" title={t("agentRun.follow.unread")}>
            <InlineError>
              {change.error instanceof ApiError ? (change.error.problem?.detail ?? change.error.message) : t("app.error.generic")}
            </InlineError>
          </Step>
        ) : !proposal ? (
          <Step stage="working" title={t("agentRun.follow.reading")} />
        ) : phase === "Rejected" ? (
          <Step stage="failed" title={t("agentRun.follow.rejected")} />
        ) : approved ? (
          <Step stage="done" title={t("agentRun.follow.approved")} />
        ) : (
          <Step stage="waiting" title={t("agentRun.follow.waiting", { kind: kind || "*" })}>
            {red ? (
              <p className="text-fg-muted">{t("agentRun.follow.red")}</p>
            ) : standing?.block ? (
              <p className="text-fg-muted">{forPeople(t(`approvals.${standing.block}`))}</p>
            ) : standing ? (
              // The agent wrote this Change, and text it read can steer what it wrote: what the
              // Change does stands above the button, so nobody approves it unseen (F-R2UI2-005).
              <div className="space-y-2">
                <PlanDiffViewer fields={proposal.planFields} />
                <Button variant="primary" size="sm" loading={approve.isPending} onClick={() => approve.mutate()}>
                  {t("agentRun.follow.approve")}
                </Button>
              </div>
            ) : null}
            {approve.error ? (
              <InlineError>
                {approve.error instanceof ApiError ? (approve.error.problem?.detail ?? approve.error.message) : t("app.error.generic")}
              </InlineError>
            ) : null}
          </Step>
        )}

        {approved ? (
          <Step stage={phase === "Applied" ? "done" : "working"} title={t(phase === "Applied" ? "agentRun.follow.live" : "agentRun.follow.deploying")} />
        ) : (
          <Step stage="later" title={t("agentRun.follow.deploy")} />
        )}

        {phase === "Applied" && kind === "Endpoint" && followed.name ? (
          <EndpointTest project={project} name={followed.name} onOpenLink={onOpenLink} />
        ) : phase === "Applied" ? (
          <Step stage="done" title={t("agentRun.follow.open", { kind })}>
            {page ? (
              <PortalLink href={page} onOpenLink={onOpenLink}>
                {followed.name || kind}
              </PortalLink>
            ) : null}
          </Step>
        ) : phase === "Rejected" ? null : (
          <Step stage="later" title={t("agentRun.follow.test.later")} />
        )}
      </ol>
    </Card>
  );
}
