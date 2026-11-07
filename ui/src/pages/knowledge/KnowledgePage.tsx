import { RowActions } from "../../components/ui/RowActions";
import { useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { ManifestGrid } from "../../components/grid/ManifestGrid";
import { api, queryKeys, unwrap, reasonOf } from "../../api/client";
import { startJob } from "../../jobs";
import { RecordLink } from "../../components/RecordLink";
import { usePermissions } from "../../api/permissions";
import {
  Alert,
  Badge,
  Button,
  buttonClass,
  Dialog,
  EmptyState,
  PageFailed,
  PageHeader,
  PermissionGuard,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeaderCell,
  TableRow,
} from "../../components/ui";
import type { BadgeTone } from "../../components/ui";
import { putDraft } from "../../api/drafts";
import { duplicateDeployment, knowledgeKeys, listSources, topLanguages, listUsage, recrawl, timeText } from "./knowledge";
import type { KnowledgeSourceRow } from "./knowledge";
import { AssistantChat, EmbedSnippet } from "./AssistantChat";
import { sum } from "./IndexStatus";

/** How a source stands: never crawled, waiting, being read, read, or failed with its reason. */
export function standing(row: KnowledgeSourceRow): { key: string; tone: BadgeTone } {
  // Nobody crawls an upload source: it holds files or none, and its passages are searchable or still waiting (T-4869).
  if (row.type === "upload") {
    if (!row.documents) return { key: "knowledge.files.none", tone: "neutral" };
    return row.pending ? { key: "knowledge.state.indexing", tone: "info" } : { key: "knowledge.state.read", tone: "success" };
  }
  if (row.state === "not-crawled" && !row.job) return { key: "knowledge.state.notCrawled", tone: "neutral" };
  switch (row.job?.state) {
    case "queued":
      return { key: "knowledge.state.queued", tone: "info" };
    case "running":
      return { key: "knowledge.state.running", tone: "info" };
    case "failed":
      return { key: "knowledge.state.failed", tone: "danger" };
    default:
      return { key: "knowledge.state.crawled", tone: "success" };
  }
}

/** What a source reads: a website's start addresses, a catalogue's spaces, the guide or the CKAN instance. */
function useWhereOf(): (row: KnowledgeSourceRow) => string {
  const { t } = useTranslation();
  return (row) =>
    row.type === "website"
      ? row.startUrls.join(", ")
      : row.type === "catalogue"
        ? (row.contextSpaces ?? []).length > 0
          ? (row.contextSpaces ?? []).join(", ")
          : t("knowledge.everySpace")
        : row.type === "guide"
          ? t("knowledge.userGuide")
          : (row.ckanInstanceRef ?? "");
}

/**
 * The knowledge assistant's sources of one project (T-3057, API/01 §34): every
 * `KnowledgeSource` the project declares and what the assistant holds of it, with the page that
 * shows its pages and documents, and a crawl started now rather than at its schedule.
 */
export function KnowledgePage({ project }: { project: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const number = new Intl.NumberFormat(i18n.language);
  const whereOf = useWhereOf();
  const queryClient = useQueryClient();
  const { can } = usePermissions(project);
  const [queued, setQueued] = useState<string | null>(null);
  const sources = useQuery({
    queryKey: knowledgeKeys.sources(project),
    queryFn: () => listSources(project),
  });
  const crawl = useMutation({
    mutationFn: (source: string) => recrawl(project, source),
    onSuccess: (_, source) => {
      setQueued(source);
      // The person may leave: the Shell says when the crawl is done (T-3245).
      startJob({ kind: "knowledgeCrawl", project, name: source });
      void queryClient.invalidateQueries({ queryKey: knowledgeKeys.sources(project) });
    },
  });
  const rows = sources.data ?? [];
  const navigate = useNavigate();
  // The page's one action, in its header and in its empty state (T-3246).
  const addSource = (
    <PermissionGuard project={project} kind="KnowledgeSource" verb="propose">
      <Button
        variant="primary"
        size="sm"
        onClick={() =>
          void navigate({ to: "/projects/$project/$plural/new", params: { project, plural: "knowledgesources" } })
        }
      >
        {t("knowledge.addSource")}
      </Button>
    </PermissionGuard>
  );

  return (
    <section aria-label={t("knowledge.title")} className="space-y-6">
      <PageHeader
        compact
        title={t("knowledge.title")}
        description={t("knowledge.intro")}
        actions={
          <>
            {/* The stored conversations, shown to the assistant's administrator alone (T-4670). */}
            {can("AssistantDeployment", "approve") ? (
              <Link
                to="/projects/$project/knowledge/conversations"
                params={{ project }}
                className={buttonClass("secondary")}
              >
                {t("conversations.link")}
              </Link>
            ) : null}
            {addSource}
          </>
        }
      />
      {sources.isError ? (
        <PageFailed
          error={sources.error}
          onRetry={() => {
            void sources.refetch();
          }}
        >
          {t("knowledge.failed", { reason: reasonOf(sources.error, t("app.error.generic")) })}
        </PageFailed>
      ) : null}
      {crawl.isError ? (
        <Alert role="alert" tone="danger">
          {reasonOf(crawl.error, t("app.error.generic"))}
        </Alert>
      ) : null}
      {queued && !crawl.isError ? (
        <Alert role="status" tone="success">
          {t("knowledge.queued", { source: queued })}
        </Alert>
      ) : null}
      {sources.isLoading ? <p role="status">{t("app.loading")}</p> : null}
      {!sources.isLoading && !sources.isError && rows.length === 0 ? (
        <EmptyState title={t("knowledge.empty")} description={t("knowledge.emptyHint")} icon="search" action={addSource} />
      ) : null}
      {rows.length > 0 ? (
        <p role="status" className="text-caption text-fg-muted">
          {t("knowledge.index.totals", {
            pages: sum(rows, "pages"),
            files: sum(rows, "documents"),
            embedded: sum(rows, "embedded"),
            passages: sum(rows, "passages"),
            pending: sum(rows, "pending"),
            failed: sum(rows, "failed"),
          })}
        </p>
      ) : null}
      {rows.length > 0 ? (
        <ManifestGrid<KnowledgeSourceRow>
          caption={t("knowledge.sources")}
          storageKey="knowledge-sources"
          searchLabel={t("grid.searchIn", { list: t("knowledge.sources") })}
          rows={rows}
          rowKey={(row) => row.source}
          query={sources}
          name={{
            id: "source",
            label: t("knowledge.column.source"),
            text: (row) => row.source,
            // Every source opens: an upload before its first file is where it is sent (T-4185), a
            // source not crawled yet or whose crawl failed says so there and offers a search; the
            // row hands its click to this link (T-5328).
            cell: (row) => (
              <Link
                data-row-link=""
                to="/projects/$project/knowledge/$source"
                params={{ project, source: row.source }}
                className="focus-ring whitespace-nowrap rounded-sm font-mono text-primary-soft-fg underline underline-offset-2"
              >
                {row.source}
              </Link>
            ),
          }}
          columns={[
            {
              id: "type",
              label: t("knowledge.column.type"),
              filter: true,
              text: (row) => row.type,
              valueLabel: (type) => t(`knowledge.type.${type}`),
              cell: (row) => (
                <>
                  {t(`knowledge.type.${row.type}`)}
                  <span className="block text-caption text-fg-muted [overflow-wrap:anywhere]">{whereOf(row)}</span>
                </>
              ),
            },
            {
              id: "state",
              label: t("knowledge.column.state"),
              filter: true,
              text: (row) => standing(row).key,
              valueLabel: (key) => t(key),
              cell: (row) => (
                <>
                  <Badge tone={standing(row).tone}>{t(standing(row).key)}</Badge>
                  {row.job?.state === "failed" && row.job.error ? (
                    <span className="block text-caption text-fg-muted [overflow-wrap:anywhere]">{row.job.error}</span>
                  ) : null}
                </>
              ),
            },
            {
              id: "lastCrawl",
              label: t("knowledge.column.lastCrawl"),
              text: (row) => row.lastCrawl ?? "",
              cell: (row) => <span className="whitespace-nowrap">{timeText(row.lastCrawl, i18n.language)}</span>,
            },
            {
              id: "pages",
              label: t("knowledge.column.pages"),
              numeric: true,
              align: "right",
              text: (row) => (row.state === "crawled" ? String(row.pages ?? 0) : ""),
              cell: (row) => (
                <span className="whitespace-nowrap tabular-nums">
                  {row.state === "crawled"
                    ? t("knowledge.ofTotal", { part: number.format(row.pagesIncluded ?? 0), total: number.format(row.pages ?? 0) })
                    : "—"}
                </span>
              ),
            },
            {
              id: "documents",
              label: t("knowledge.column.documents"),
              numeric: true,
              align: "right",
              text: (row) => (row.state === "crawled" ? String(row.documents ?? 0) : ""),
              cell: (row) => <span className="tabular-nums">{row.state === "crawled" ? number.format(row.documents ?? 0) : "—"}</span>,
            },
            {
              id: "passages",
              label: t("knowledge.column.passages"),
              numeric: true,
              align: "right",
              text: (row) => (row.state === "crawled" ? String(row.passages ?? 0) : ""),
              cell: (row) => (
                <span className="whitespace-nowrap tabular-nums">
                  {row.state !== "crawled"
                    ? "—"
                    : (row.embedded ?? 0) === (row.passages ?? 0)
                      ? number.format(row.passages ?? 0)
                      : t("knowledge.embeddedOf", { embedded: number.format(row.embedded ?? 0), total: number.format(row.passages ?? 0) })}
                </span>
              ),
            },
            {
              id: "languages",
              label: t("knowledge.column.languages"),
              text: (row) => (row.state === "crawled" ? Object.keys(row.languages ?? {}).join(", ") : ""),
              cell: (row) => (row.state === "crawled" ? <Languages languages={row.languages} /> : "—"),
            },
          ]}
          actions={(row) => (
            <RowActions label={row.source} actions={[]} primary={row.type === "upload" ? undefined : (
              <PermissionGuard project={project} kind="KnowledgeSource" verb="propose">
                <Button
                  variant="secondary"
                  size="sm"
                  disabled={crawl.isPending || row.job?.state === "queued" || row.job?.state === "running"}
                  disabledReason={
                    row.job?.state === "queued" || row.job?.state === "running" ? t("knowledge.alreadyQueued") : t("knowledge.recrawlSending")
                  }
                  aria-label={t("knowledge.recrawlOf", { source: row.source })}
                  onClick={() => crawl.mutate(row.source)}
                >
                  {t("knowledge.recrawl")}
                </Button>
              </PermissionGuard>
            )} />
          )}
          empty={<EmptyState bare title={t("knowledge.empty")} description={t("knowledge.emptyHint")} icon="search" />}
        />
      ) : null}
      <Assistants project={project} />
    </section>
  );
}

interface DeploymentItem {
  metadata: { name: string; namespace?: string };
  spec: Record<string, unknown> & {
    publicId?: string;
    channel?: string;
    sources?: string[];
    connectors?: { endpoint: string }[];
    languages?: string[];
    privacy?: { contact?: string; documentsUrl?: string };
  };
}

/**
 * The project's assistant deployments: where each answers, from which sources and Endpoints, and
 * what it spent over the last 30 days (T-3057). Each is created and edited as a manifest on its
 * list, like every other kind.
 */
/** The three largest languages; the full list on hover and for a screen reader (T-4994). */
function Languages({ languages }: { languages: Record<string, number> | undefined }): JSX.Element {
  const { t, i18n } = useTranslation();
  const { top, more, all } = topLanguages(languages, i18n.language);
  if (more === 0) return <>{top}</>;
  return (
    <span title={all}>
      <span aria-hidden>
        {top} <span className="whitespace-nowrap text-fg-muted">{t("knowledge.languagesMore", { count: more })}</span>
      </span>
      <span className="sr-only">{all}</span>
    </span>
  );
}

function Assistants({ project }: { project: string }): JSX.Element {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const [usageOf, setUsageOf] = useState<string | null>(null);
  const [chatWith, setChatWith] = useState<DeploymentItem | null>(null);
  const [embedOf, setEmbedOf] = useState<DeploymentItem | null>(null);
  const list = useQuery({
    queryKey: queryKeys.list(project, "assistantdeployments"),
    queryFn: async () =>
      unwrap(await api.GET("/api/v1/projects/{project}/{plural}", { params: { path: { project, plural: "assistantdeployments" } } })),
  });
  const items = (list.data?.items ?? []) as unknown as DeploymentItem[];
  // A copy with a new name and publicId, kept as a draft and opened in the create form (T-4281).
  const duplicate = useMutation({
    mutationFn: async (item: DeploymentItem) => {
      const copy = duplicateDeployment(item, items);
      await putDraft(project, "AssistantDeployment", copy.metadata.name, copy);
      return copy.metadata.name;
    },
    onSuccess: (name) =>
      void navigate({ href: `/projects/${encodeURIComponent(project)}/assistantdeployments?draft=${encodeURIComponent(name)}` }),
  });
  const addAssistant = (
    <PermissionGuard project={project} kind="AssistantDeployment" verb="propose">
      <Button
        variant="secondary"
        size="sm"
        onClick={() =>
          void navigate({ to: "/projects/$project/$plural/new", params: { project, plural: "assistantdeployments" } })
        }
      >
        {t("knowledge.assistants.add")}
      </Button>
    </PermissionGuard>
  );
  return (
    <section aria-labelledby="knowledge-assistants" className="space-y-3">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h2 id="knowledge-assistants" className="text-lg font-semibold">
          {t("knowledge.assistants.title")}
        </h2>
        {addAssistant}
      </div>
      {list.isError ? (
        <PageFailed
          error={list.error}
          onRetry={() => {
            void list.refetch();
          }}
        />
      ) : null}
      {!list.isLoading && !list.isError && items.length === 0 ? (
        <EmptyState
          title={t("knowledge.assistants.empty")}
          description={t("knowledge.assistants.emptyHint")}
          icon="chat"
          action={addAssistant}
        />
      ) : null}
      {duplicate.isError ? <PageFailed error={duplicate.error} onRetry={() => duplicate.reset()} /> : null}
      {items.length > 0 ? (
        <ManifestGrid<DeploymentItem>
          caption={t("knowledge.assistants.title")}
          storageKey="knowledge-assistants"
          searchLabel={t("grid.searchIn", { list: t("knowledge.assistants.title") })}
          rows={items}
          rowKey={(item) => item.metadata.name}
          query={list}
          name={{
            id: "name",
            label: t("knowledge.field.name"),
            text: (item) => item.metadata.name,
            cell: (item) => <RecordLink project={project} plural="assistantdeployments" name={item.metadata.name} />,
          }}
          columns={[
            { id: "publicId", label: t("knowledge.field.publicId"), text: (item) => item.spec.publicId ?? "", cell: (item) => <span className="font-mono">{item.spec.publicId ?? "—"}</span> },
            {
              id: "channel",
              label: t("knowledge.field.channel"),
              filter: true,
              text: (item) => item.spec.channel ?? "",
              valueLabel: (channel) => t(`knowledge.channel.${channel}`),
              cell: (item) => (
                <>
                  {item.spec.channel ? t(`knowledge.channel.${item.spec.channel}`) : "—"}
                  {/* A visitor-facing assistant whose privacy dialog would name no contact (T-4670). */}
                  {item.spec.channel && item.spec.channel !== "internal" && !item.spec.privacy?.contact ? (
                    <Badge tone="warning" className="ml-2 whitespace-nowrap">
                      {t("knowledge.assistants.noPrivacyContact")}
                    </Badge>
                  ) : null}
                </>
              ),
            },
            { id: "sources", label: t("knowledge.field.sources"), text: (item) => (item.spec.sources ?? []).join(", "), cell: (item) => (item.spec.sources ?? []).join(", ") || "—" },
            {
              id: "connectors",
              label: t("knowledge.field.connectors"),
              text: (item) => (item.spec.connectors ?? []).map((c) => c.endpoint).join(", "),
              cell: (item) => (item.spec.connectors ?? []).map((c) => c.endpoint).join(", ") || "—",
            },
          ]}
          actions={(item) => (
            <div className="flex justify-end gap-1">
              <Button variant="ghost" size="sm" aria-label={t("knowledge.chat.askOf", { name: item.metadata.name })} onClick={() => setChatWith(item)}>
                {t("knowledge.chat.ask")}
              </Button>
              {item.spec.channel && item.spec.channel !== "internal" && item.spec.publicId ? (
                <Button variant="ghost" size="sm" aria-label={t("knowledge.embed.of", { name: item.metadata.name })} onClick={() => setEmbedOf(item)}>
                  {t("knowledge.embed.title")}
                </Button>
              ) : null}
              <PermissionGuard project={project} kind="AssistantDeployment" verb="propose">
                <Button
                  variant="ghost"
                  size="sm"
                  aria-label={t("knowledge.assistants.duplicateOf", { name: item.metadata.name })}
                  onClick={() => duplicate.mutate(item)}
                >
                  {t("knowledge.assistants.duplicate")}
                </Button>
              </PermissionGuard>
              <Button
                variant="ghost"
                size="sm"
                aria-label={t("knowledge.assistants.usageOf", { name: item.metadata.name })}
                onClick={() => setUsageOf(item.metadata.name)}
              >
                {t("knowledge.assistants.usage")}
              </Button>
            </div>
          )}
          empty={<EmptyState bare title={t("knowledge.assistants.empty")} description={t("knowledge.assistants.emptyHint")} icon="chat" />}
        />
      ) : null}
      <Dialog
        open={usageOf !== null}
        onOpenChange={(open) => (open ? undefined : setUsageOf(null))}
        title={t("knowledge.assistants.usageOf", { name: usageOf ?? "" })}
        closeLabel={t("app.close")}
      >
        {usageOf ? <Usage project={project} deployment={usageOf} /> : null}
      </Dialog>
      <Dialog
        open={chatWith !== null}
        onOpenChange={(open) => (open ? undefined : setChatWith(null))}
        title={t("knowledge.chat.askOf", { name: chatWith?.metadata.name ?? "" })}
        description={chatWith?.spec.channel === "internal" ? t("knowledge.chat.asYou") : t("knowledge.chat.asVisitor")}
        closeLabel={t("app.close")}
        size="lg"
      >
        {chatWith ? (
          <AssistantChat
            key={chatWith.metadata.name}
            project={project}
            deployment={chatWith.metadata.name}
            connectors={(chatWith.spec.connectors ?? []).map((c) => c.endpoint)}
          />
        ) : null}
      </Dialog>
      <Dialog
        open={embedOf !== null}
        onOpenChange={(open) => (open ? undefined : setEmbedOf(null))}
        title={t("knowledge.embed.of", { name: embedOf?.metadata.name ?? "" })}
        closeLabel={t("app.close")}
      >
        {embedOf?.spec.publicId ? <EmbedSnippet publicId={embedOf.spec.publicId} title={embedOf.metadata.name} language={embedOf.spec.languages?.[0]} /> : null}
      </Dialog>
    </section>
  );
}

function Usage({ project, deployment }: { project: string; deployment: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const usage = useQuery({
    queryKey: ["projects", project, "knowledge", "deployments", deployment, "usage"],
    queryFn: () => listUsage(project, deployment),
  });
  if (usage.isLoading) return <p role="status">{t("app.loading")}</p>;
  if (usage.isError) {
    return (
      <PageFailed
        error={usage.error}
        onRetry={() => {
          void usage.refetch();
        }}
      />
    );
  }
  const days = usage.data ?? [];
  if (days.length === 0) return <p>{t("knowledge.assistants.noUsage")}</p>;
  const number = new Intl.NumberFormat(i18n.language);
  return (
    <Table data-records="" caption={t("knowledge.assistants.usage")}>
      <TableHead>
        <TableHeaderCell>{t("knowledge.assistants.day")}</TableHeaderCell>
        <TableHeaderCell align="right">{t("knowledge.assistants.requests")}</TableHeaderCell>
        <TableHeaderCell align="right">{t("knowledge.assistants.tokensIn")}</TableHeaderCell>
        <TableHeaderCell align="right">{t("knowledge.assistants.tokensOut")}</TableHeaderCell>
      </TableHead>
      <TableBody>
        {days.map((day) => (
          <TableRow key={day.day}>
            <TableCell>{day.day}</TableCell>
            <TableCell align="right">{number.format(day.requests)}</TableCell>
            <TableCell align="right">{number.format(day.tokensIn)}</TableCell>
            <TableCell align="right">{number.format(day.tokensOut)}</TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  );
}
