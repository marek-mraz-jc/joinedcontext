import { useState } from "react";
import type { JSX } from "react";
import { Link } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, unwrap } from "../../api/client";
import i18n from "../../i18n";
import { isChange, ORG_NAMESPACE } from "../../api/manifest";
import { AccountAccess } from "../access/AccountAccess";
import type { Manifest, ResourceProposal } from "../../api/manifest";
import { allows, usePermissions } from "../../api/permissions";
import { proposeChecked } from "../../api/proposal";
import type { components } from "../../api/schema";
import { Alert, Badge, Button, Card, EmptyState, safeHref, Table, TableBody, TableCell, TableHead, TableHeaderCell, TableRow, TableSkeleton, tabPanelProps, Tabs } from "../../components/ui";
import type { BadgeTone } from "../../components/ui/Badge";
import { PageFailed } from "../../components/ui/PageState";
import { AgentActivity } from "./AgentActivity";

type AgentItem = components["schemas"]["AgentItem"];
type AgentTab = "overview" | "access" | "activity" | "settings";

const STATE_TONE: Record<AgentItem["state"], BadgeTone> = {
  active: "success",
  paused: "warning",
  running: "info",
  waiting: "accent",
};

/** The plural of each agent kind, for its manifest's address. */
const PLURAL: Record<string, string> = {
  AssistantDeployment: "assistantdeployments",
  AgentProfile: "agentprofiles",
  McpServer: "mcpservers",
  ServiceAccount: "serviceaccounts",
};

/** One agent's address inside Administration: `/organization/agents/{project}/{kind}/{name}`. */
export function agentPath(
  item: Pick<AgentItem, "project" | "kind" | "name" | "runId">,
): string {
  return [item.project, item.kind, item.runId ?? item.name]
    .map(encodeURIComponent)
    .join("/");
}

/** The agents of the organization, shared by the list and an agent's page (API/01 §48). */
function useAgents() {
  return useQuery({
    queryKey: ["organizationAgents"],
    retry: false,
    queryFn: async () => unwrap(await api.GET("/api/v1/organization/agents")),
  });
}

function refusal(error: unknown): string | null {
  return error instanceof ApiError && error.status === 403
    ? (error.problem?.detail ?? error.message)
    : null;
}

/**
 * Administration → Agents (AG-126, T-4849): every agent identity the person may read, in one
 * table: who owns it, whom it acts as, what it reaches, what it spent and whether it answers now.
 * The Portal leaves out every kind the person may not read, so nothing here is counted for them.
 */
export function Agents(): JSX.Element {
  const { t } = useTranslation();
  const agents = useAgents();
  const items = agents.data?.items ?? [];
  const denied = refusal(agents.error);
  return (
    <section
      className="space-y-4"
      aria-labelledby="organization-agents-heading"
    >
      <div>
        <h2
          id="organization-agents-heading"
          className="text-title font-semibold text-fg"
        >
          {t("organization.agents.title")}
        </h2>
        <p className="text-body text-fg-muted">
          {t("organization.agents.lead")}
        </p>
      </div>
      {denied !== null ? (
        <Alert tone="info">{denied}</Alert>
      ) : agents.isError ? (
        <PageFailed
          error={agents.error}
          onRetry={() => void agents.refetch()}
        />
      ) : agents.isSuccess && items.length === 0 ? (
        <EmptyState
          icon="access"
          title={t("organization.agents.empty")}
          description={t("organization.agents.emptyHint")}
        />
      ) : (
        <>
          <ul
            className="space-y-3 sm:hidden"
            aria-label={t("organization.agents.caption")}
          >
            {items.map((item) => (
              <Card as="li" padding="md" key={agentPath(item)}>
                <div className="flex items-start justify-between gap-3">
                  <div className="min-w-0">
                    <Link
                      to="/organization/$tab/$"
                      params={{ tab: "agents", _splat: agentPath(item) }}
                      className="font-semibold underline [overflow-wrap:anywhere]"
                    >
                      {item.title ?? item.name}
                    </Link>
                    <span className="block text-caption text-fg-muted">
                      {t(`organization.agents.kind.${item.kind}`)} ·{" "}
                      {projectName(t, item.project)}
                    </span>
                  </div>
                  <Badge tone={STATE_TONE[item.state]}>
                    {t(`organization.agents.state.${item.state}`)}
                  </Badge>
                </div>
                <p className="mt-2 text-body text-fg">
                  {t(`organization.agents.actsAs.${item.actsAs}`)}
                </p>
                <p className="text-caption text-fg-muted">
                  {reachText(t, item)} · {spendText(t, item)}
                </p>
              </Card>
            ))}
          </ul>
          <div className="hidden sm:block">
            <Table
              caption={t("organization.agents.caption")}
              status={agents.isPending ? t("app.loading") : undefined}
            >
              <TableHead>
                <TableHeaderCell>
                  {t("organization.agents.column.name")}
                </TableHeaderCell>
                <TableHeaderCell>
                  {t("organization.agents.column.owner")}
                </TableHeaderCell>
                <TableHeaderCell>
                  {t("organization.agents.column.actsAs")}
                </TableHeaderCell>
                <TableHeaderCell>
                  {t("organization.agents.column.reach")}
                </TableHeaderCell>
                <TableHeaderCell>
                  {t("organization.agents.column.lastActivity")}
                </TableHeaderCell>
                <TableHeaderCell>
                  {t("organization.agents.column.spend")}
                </TableHeaderCell>
                <TableHeaderCell>
                  {t("organization.agents.column.state")}
                </TableHeaderCell>
              </TableHead>
              {agents.isPending ? (
                <TableSkeleton columns={7} />
              ) : (
                <TableBody>
                  {items.map((item) => (
                    <TableRow key={agentPath(item)}>
                      <TableCell primary>
                        <Link
                          data-row-link=""
                          to="/organization/$tab/$"
                          params={{ tab: "agents", _splat: agentPath(item) }}
                          className="underline"
                        >
                          {item.title ?? item.name}
                        </Link>
                        <span className="block text-caption text-fg-muted">
                          {t(`organization.agents.kind.${item.kind}`)} ·{" "}
                          {projectName(t, item.project)}
                        </span>
                      </TableCell>
                      <TableCell>{ownerText(t, item)}</TableCell>
                      <TableCell>
                        {t(`organization.agents.actsAs.${item.actsAs}`)}
                      </TableCell>
                      <TableCell>{reachText(t, item)}</TableCell>
                      <TableCell>
                        <When value={item.lastActivity} />
                      </TableCell>
                      <TableCell>{spendText(t, item)}</TableCell>
                      <TableCell>
                        <Badge tone={STATE_TONE[item.state]}>
                          {t(`organization.agents.state.${item.state}`)}
                        </Badge>
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              )}
            </Table>
          </div>
        </>
      )}
    </section>
  );
}

type Translate = ReturnType<typeof useTranslation>["t"];

function projectName(t: Translate, project: string): string {
  return project === ORG_NAMESPACE
    ? t("organization.agents.organization")
    : project;
}

function ownerText(t: Translate, item: AgentItem): string {
  if (item.owner.type === "person") {
    return item.owner.name;
  }
  return item.owner.name === ORG_NAMESPACE
    ? t("organization.agents.organization")
    : t("organization.agents.ownerProject", { project: item.owner.name });
}

function reachText(t: Translate, item: AgentItem): string {
  if (item.spaces.length === 0 && item.endpoints.length === 0) {
    return t("organization.agents.reachNone");
  }
  return t("organization.agents.reachCount", {
    spaces: item.spaces.length,
    endpoints: item.endpoints.length,
  });
}

function spendText(t: Translate, item: AgentItem): string {
  const { used, cap, per } = item.spend;
  const format = new Intl.NumberFormat(
    i18n.resolvedLanguage ?? i18n.language ?? "en",
  );
  const number = (value: number) => format.format(value);
  if (used == null && cap == null) {
    return t("organization.agents.spendNone");
  }
  if (used == null) {
    return t(`organization.agents.spendCapOnly.${per ?? "day"}`, {
      cap: number(cap ?? 0),
    });
  }
  if (cap == null) {
    return t(`organization.agents.spendUsedOnly.${per ?? "day"}`, {
      used: number(used),
    });
  }
  return t(`organization.agents.spendOf.${per ?? "day"}`, {
    used: number(used),
    cap: number(cap),
  });
}

function When({ value }: { value: string | null | undefined }): JSX.Element {
  const { t } = useTranslation();
  if (!value) {
    return (
      <span className="text-fg-muted">
        {t("organization.agents.noActivity")}
      </span>
    );
  }
  const date = new Date(value);
  return (
    <time dateTime={value}>
      {date.toLocaleString(i18n.resolvedLanguage ?? "en", {
        dateStyle: "medium",
        timeStyle: "short",
      })}
    </time>
  );
}

/** Where the agent is managed today, beside this page. */
function homeOf(item: AgentItem): string {
  const org = item.project === ORG_NAMESPACE;
  switch (item.kind) {
    case "AssistantDeployment":
      return `/projects/${encodeURIComponent(item.project)}/knowledge`;
    case "AgentProfile":
      return "/organization/agentprofiles";
    case "McpServer":
      return org
        ? "/organization/mcp-servers"
        : `/projects/${encodeURIComponent(item.project)}/mcp`;
    case "ServiceAccount":
      return org
        ? "/organization/service-accounts"
        : `/projects/${encodeURIComponent(item.project)}/settings/service-accounts`;
    default:
      return `/projects/${encodeURIComponent(item.project)}/assistant`;
  }
}

/** An agent's page (AG-128): Overview, Access, Activity and Settings, and its pause (AG-129). */
export function AgentPage({ path }: { path: string }): JSX.Element {
  const { t } = useTranslation();
  const agents = useAgents();
  // An alert notice opens the Activity tab with `#activity` (T-5301).
  const [tab, setTab] = useState<AgentTab>(() =>
    window.location.hash === "#activity" ? "activity" : "overview",
  );
  const item = agents.data?.items.find(
    (candidate) => agentPath(candidate) === path,
  );
  const denied = refusal(agents.error);
  if (agents.isPending) {
    return (
      <p role="status" className="text-body text-fg-muted">
        {t("app.loading")}
      </p>
    );
  }
  if (denied !== null || agents.isError) {
    return denied !== null ? (
      <Alert tone="info">{denied}</Alert>
    ) : (
      <PageFailed error={agents.error} onRetry={() => void agents.refetch()} />
    );
  }
  if (!item) {
    return (
      <EmptyState
        heading={1}
        icon="search"
        title={t("organization.agents.gone")}
        description={t("organization.agents.goneHint")}
        action={
          <Link
            to="/organization/$tab"
            params={{ tab: "agents" }}
            className="underline"
          >
            {t("organization.agents.back")}
          </Link>
        }
      />
    );
  }
  const tabs: { value: AgentTab; label: string }[] = (
    ["overview", "access", "activity", "settings"] as const
  ).map((value) => ({ value, label: t(`organization.agents.tab.${value}`) }));
  return (
    <article className="space-y-4" aria-labelledby="agent-heading">
      <Link
        to="/organization/$tab"
        params={{ tab: "agents" }}
        className="text-body text-fg-muted underline"
      >
        {t("organization.agents.back")}
      </Link>
      <header className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h1 id="agent-heading" className="text-title font-semibold text-fg">
            {item.title ?? item.name}
          </h1>
          <p className="text-body text-fg-muted">
            {t(`organization.agents.kind.${item.kind}`)} ·{" "}
            {projectName(t, item.project)}
          </p>
        </div>
        <Badge tone={STATE_TONE[item.state]}>
          {t(`organization.agents.state.${item.state}`)}
        </Badge>
      </header>
      <Tabs
        id="agent"
        label={t("organization.agents.tabsLabel")}
        tabs={tabs}
        value={tab}
        onChange={setTab}
      />
      <div {...tabPanelProps("agent", tab)} className="space-y-4">
        {tab === "overview" ? <Overview item={item} /> : null}
        {tab === "access" ? <Access item={item} /> : null}
        {tab === "activity" ? <Activity item={item} /> : null}
        {tab === "settings" ? <Settings item={item} /> : null}
      </div>
    </article>
  );
}

function Facts({ rows }: { rows: [string, React.ReactNode][] }): JSX.Element {
  return (
    <dl className="grid grid-cols-1 gap-x-6 gap-y-3 sm:grid-cols-[minmax(10rem,auto)_1fr]">
      {rows.map(([label, value]) => (
        <div key={label} className="contents">
          <dt className="text-caption font-semibold text-fg-muted">{label}</dt>
          <dd className="text-body text-fg">{value}</dd>
        </div>
      ))}
    </dl>
  );
}

function Overview({ item }: { item: AgentItem }): JSX.Element {
  const { t } = useTranslation();
  return (
    <>
      <Facts
        rows={[
          [t("organization.agents.column.owner"), ownerText(t, item)],
          [
            t("organization.agents.column.actsAs"),
            t(`organization.agents.actsAsLong.${item.actsAs}`),
          ],
          [t("organization.agents.column.reach"), reachText(t, item)],
          [
            t("organization.agents.column.lastActivity"),
            <When key="when" value={item.lastActivity} />,
          ],
          [t("organization.agents.column.spend"), spendText(t, item)],
        ]}
      />
      <PauseControl item={item} />
    </>
  );
}

/**
 * Pause and resume (AG-129): a Change of an assistant deployment's `spec.enabled`, checked and
 * sent for review like any edit. Only a person who may propose the kind sees the button; the
 * other kinds have no off switch and say where they are removed.
 */
function PauseControl({ item }: { item: AgentItem }): JSX.Element | null {
  const { t } = useTranslation();
  const permissions = usePermissions(item.project);
  const queryClient = useQueryClient();
  const [sent, setSent] = useState<string | null>(null);
  const pausable = item.kind === "AssistantDeployment";
  const mayPropose = allows(permissions.data, "AssistantDeployment", "propose");
  const flip = useMutation({
    mutationFn: async () => {
      const stored = (await unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}/{name}", {
          params: {
            path: {
              project: item.project,
              plural: "assistantdeployments",
              name: item.name,
            },
          },
        }),
      )) as Manifest & { status?: unknown };
      const { status: _status, ...envelope } = stored;
      const body = {
        ...envelope,
        spec: { ...stored.spec, enabled: item.state === "paused" },
      } as unknown as ResourceProposal;
      return proposeChecked(item.project, "assistantdeployments", body, false);
    },
    onSuccess: (result) => {
      setSent(isChange(result) ? result.metadata.name : "");
      void queryClient.invalidateQueries({ queryKey: ["organizationAgents"] });
    },
  });
  if (item.kind === "AgentRun") {
    return (
      <p className="text-body text-fg-muted">
        {t("organization.agents.runStop")}{" "}
        <a className="underline" href={safeHref(homeOf(item))}>
          {t("organization.agents.openHome")}
        </a>
      </p>
    );
  }
  if (!pausable) {
    return (
      <p className="text-body text-fg-muted">
        {t("organization.agents.noPause")}{" "}
        <a className="underline" href={safeHref(homeOf(item))}>
          {t("organization.agents.openHome")}
        </a>
      </p>
    );
  }
  if (sent !== null) {
    return (
      <Alert tone="success" role="status">
        {t(
          item.state === "paused"
            ? "organization.agents.resumeSent"
            : "organization.agents.pauseSent",
        )}{" "}
        {sent ? (
          <Link
            to="/projects/$project/approvals/$id"
            params={{ project: item.project, id: sent }}
            className="underline"
          >
            {t("organization.agents.openChange")}
          </Link>
        ) : null}
      </Alert>
    );
  }
  if (!mayPropose) {
    return null;
  }
  return (
    <div className="space-y-2">
      <p className="text-body text-fg-muted">
        {t(
          item.state === "paused"
            ? "organization.agents.resumeLead"
            : "organization.agents.pauseLead",
        )}
      </p>
      <Button
        variant={item.state === "paused" ? "primary" : "secondary"}
        onClick={() => flip.mutate()}
        loading={flip.isPending}
      >
        {t(
          item.state === "paused"
            ? "organization.agents.resume"
            : "organization.agents.pause",
        )}
      </Button>
      {flip.isError ? (
        <Alert tone="danger">
          {flip.error instanceof ApiError
            ? (flip.error.problem?.detail ?? flip.error.message)
            : String(flip.error)}
        </Alert>
      ) : null}
    </div>
  );
}

function Access({ item }: { item: AgentItem }): JSX.Element {
  const { t } = useTranslation();
  const list = (values: string[]) =>
    values.length === 0 ? (
      <span className="text-fg-muted">{t("organization.agents.none")}</span>
    ) : (
      <ul className="flex flex-wrap gap-1">
        {values.map((value) => (
          <li key={value}>
            <Badge tone="neutral">{value}</Badge>
          </li>
        ))}
      </ul>
    );
  const facts = (
    <Facts
      rows={[
        [
          t("organization.agents.column.actsAs"),
          t(`organization.agents.actsAsLong.${item.actsAs}`),
        ],
        [t("organization.agents.spaces"), list(item.spaces)],
        [t("organization.agents.endpoints"), list(item.endpoints)],
      ]}
    />
  );
  // A ServiceAccount's own grants: the kind × verb matrix with each right's source, Revoke now (AG-130..AG-133).
  return item.kind === "ServiceAccount" ? (
    <div className="space-y-4">
      {facts}
      <AccountAccess project={item.project} account={item.name} level={2} />
    </div>
  ) : (
    facts
  );
}

/**
 * The Activity tab: an agent profile's runs, or the one run, step by step (AG-134); a deployment,
 * an MCP server or a service account keeps its record on its own page.
 */
function Activity({ item }: { item: AgentItem }): JSX.Element {
  const { t } = useTranslation();
  if (item.kind === "AgentProfile" || item.runId) {
    return (
      <AgentActivity
        scope={
          item.runId
            ? { project: item.project, run: item.runId }
            : { project: "", profile: item.name }
        }
      />
    );
  }
  return (
    <>
      <Facts
        rows={[
          [
            t("organization.agents.column.lastActivity"),
            <When key="when" value={item.lastActivity} />,
          ],
        ]}
      />
      <p className="text-body text-fg-muted">
        {t("organization.agents.activityLead")}{" "}
        <a className="underline" href={safeHref(homeOf(item))}>
          {t("organization.agents.openHome")}
        </a>
      </p>
    </>
  );
}

/** The fields of a manifest an administrator tunes; never a prompt, a key or a credential (AG-127). */
function settingsOf(
  t: Translate,
  kind: string,
  spec: Record<string, unknown>,
): [string, string][] {
  const at = (path: string): unknown =>
    path
      .split(".")
      .reduce<unknown>(
        (value, key) => (value as Record<string, unknown> | undefined)?.[key],
        spec,
      );
  const text = (value: unknown): string | null =>
    typeof value === "string" ||
    typeof value === "number" ||
    typeof value === "boolean"
      ? String(value)
      : null;
  const fields: Record<string, string[]> = {
    AssistantDeployment: [
      "channel",
      "model.answer",
      "model.router",
      "budget.tokensPerDay",
      "budget.costPerDayUsd",
      "rateLimit.requestsPerMinute",
      "enabled",
    ],
    AgentProfile: [
      "role",
      "model.provider",
      "model.name",
      "model.maxTokensPerRun",
      "limits.stepsPerRun",
      "limits.wallClock",
      "limits.concurrentRunsPerOrganization",
    ],
    McpServer: ["audience"],
    ServiceAccount: ["purpose", "limits.requestsPerMinute"],
  };
  const rows: [string, string][] = (fields[kind] ?? []).flatMap((path) => {
    const value = text(at(path));
    return value === null
      ? []
      : [[t(`organization.agents.setting.${path}`), value] as [string, string]];
  });
  if (kind === "ServiceAccount") {
    const credentials = Array.isArray(spec.credentials)
      ? spec.credentials.length
      : 0;
    rows.push([
      t("organization.agents.setting.credentials"),
      String(credentials),
    ]);
  }
  return rows;
}

function Settings({ item }: { item: AgentItem }): JSX.Element {
  const { t } = useTranslation();
  const plural = PLURAL[item.kind];
  const manifest = useQuery({
    queryKey: ["organizationAgents", item.project, plural, item.name],
    enabled: plural !== undefined,
    queryFn: async () =>
      (await unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}/{name}", {
          params: { path: { project: item.project, plural, name: item.name } },
        }),
      )) as Manifest,
  });
  if (plural === undefined) {
    return (
      <p className="text-body text-fg-muted">
        {t("organization.agents.runSettings", { profile: item.profile ?? "" })}{" "}
        <a className="underline" href={safeHref(homeOf(item))}>
          {t("organization.agents.openHome")}
        </a>
      </p>
    );
  }
  if (manifest.isPending) {
    return (
      <p role="status" className="text-body text-fg-muted">
        {t("app.loading")}
      </p>
    );
  }
  if (manifest.isError) {
    return (
      <PageFailed
        error={manifest.error}
        onRetry={() => void manifest.refetch()}
      />
    );
  }
  return (
    <>
      <Facts rows={settingsOf(t, item.kind, manifest.data.spec)} />
      <p className="text-body">
        <a className="underline" href={safeHref(homeOf(item))}>
          {t("organization.agents.openManifest")}
        </a>
      </p>
    </>
  );
}
