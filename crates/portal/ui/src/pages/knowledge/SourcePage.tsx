import { reasonOf } from "../../api/client";
import { PageFailed } from "../../components/ui/PageState";
import { useDeferredValue, useState } from "react";
import type { FormEvent } from "react";
import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import {
  Alert,
  Badge,
  Button,
  Checkbox,
  Dialog,
  EmptyState,
  ExternalLink,
  Field,
  Input,
  PageHeader,
  PermissionGuard,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeaderCell,
  TableRow,
  Tabs,
  tabPanelProps,
} from "../../components/ui";
import {
  bytesText,
  findPages,
  knowledgeKeys,
  listDocuments,
  listLinks,
  listPages,
  listPassages,
  listSources,
  PAGE_QUERY_MIN,
  setInclusion,
  timeText,
  trySearch,
  uploadedFile,
} from "./knowledge";
import type { DocumentRow, InclusionCounts, PageRow } from "./knowledge";
import { useUrlParam } from "../../navigation/urlState";
import { UploadedFiles } from "./UploadedFiles";
import { IndexStatus } from "./IndexStatus";

type View = "pages" | "documents";

/** What the person asked to see in the side dialog: a page's or a document's passages, or a page's links. */
type Opened =
  | { what: "passages"; title: string; page?: number; document?: number }
  | { what: "links"; title: string; page: number };

/** Whether an item is indexed, and if not who left it out (AG-113). */
export function InclusionBadge({ included, excludedBy }: { included: boolean; excludedBy: PageRow["excludedBy"] }): JSX.Element {
  const { t } = useTranslation();
  if (included) return <Badge tone="success">{t("knowledge.included")}</Badge>;
  return (
    <Badge tone={excludedBy === "administrator" ? "warning" : "neutral"}>
      {t(excludedBy === "administrator" ? "knowledge.excludedByAdministrator" : "knowledge.excludedByPattern")}
    </Badge>
  );
}

/**
 * What the assistant holds of one source (T-3057, API/01 §34): its pages as a tree read a level
 * at a time, the documents they link, the passages of any of them and the links of a page; and
 * pages, whole branches or documents left out of every answer or taken back in.
 */
export function SourcePage({ project, source }: { project: string; source: string }): JSX.Element {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  // The tab is in the address, so back, reload and a sent link open it (T-3239).
  const [view, setView] = useUrlParam<View>("tab", "pages", ["pages", "documents"]);
  const [pages, setPages] = useState<ReadonlySet<number>>(new Set());
  const [documents, setDocuments] = useState<ReadonlySet<number>>(new Set());
  const [subtree, setSubtree] = useState(true);
  const [done, setDone] = useState<{ counts: InclusionCounts; included: boolean } | null>(null);
  const [opened, setOpened] = useState<Opened | null>(null);
  const [find, setFind] = useState("");
  const found = useDeferredValue(find.trim());
  const change = useMutation({
    mutationFn: (included: boolean) =>
      setInclusion(project, source, { pages: [...pages], documents: [...documents], subtree, included }),
    onSuccess: (counts, included) => {
      setDone({ counts, included });
      setPages(new Set());
      setDocuments(new Set());
      void queryClient.invalidateQueries({ queryKey: ["projects", project, "knowledge"] });
    },
  });
  const toggle = (set: ReadonlySet<number>, id: number): Set<number> => {
    const next = new Set(set);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    return next;
  };
  const selected = pages.size + documents.size;
  // A `source: upload` holds files an administrator sends, not crawled pages (T-4185).
  const sources = useQuery({ queryKey: knowledgeKeys.sources(project), queryFn: () => listSources(project) });
  const held = sources.data?.find((row) => row.source === source);
  const upload = held?.type === "upload";
  const status = held ? <IndexStatus project={project} row={held} /> : null;
  const header = (
    <PageHeader
      title={t("knowledge.source.title", { source })}
      description={t(upload ? "knowledge.files.intro" : "knowledge.source.intro")}
      actions={
        <Link to="/projects/$project/knowledge" params={{ project }} className="focus-ring rounded-sm text-primary-soft-fg underline underline-offset-2">
          {t("knowledge.source.back")}
        </Link>
      }
    />
  );
  if (upload) {
    return (
      <section aria-label={t("knowledge.source.title", { source })} className="space-y-6">
        {header}
        {status}
        <TrySearch project={project} source={source} />
        <UploadedFiles project={project} source={source} />
      </section>
    );
  }

  return (
    <section aria-label={t("knowledge.source.title", { source })} className="space-y-6">
      {header}
      {status}
      <TrySearch project={project} source={source} />
      {change.isError ? (
        <Alert role="alert" tone="danger">
          {reasonOf(change.error, t("app.error.generic"))}
        </Alert>
      ) : null}
      {done ? (
        <Alert role="status" tone="success">
          {t(done.included ? "knowledge.source.includedDone" : "knowledge.source.excludedDone", {
            pages: done.counts.pages,
            documents: done.counts.documents,
            passages: done.counts.passagesRemoved,
          })}
        </Alert>
      ) : null}

      <div
        role="group"
        aria-label={t("knowledge.source.selection")}
        className="sticky top-0 z-10 flex flex-wrap items-center gap-3 rounded-md border border-border bg-surface p-3"
      >
        <span aria-live="polite">{t("knowledge.source.selected", { count: selected })}</span>
        <Checkbox
          label={t("knowledge.source.subtree")}
          checked={subtree}
          onChange={(event) => setSubtree(event.target.checked)}
        />
        <PermissionGuard project={project} kind="KnowledgeSource" verb="propose">
          <Button
            variant="secondary"
            size="sm"
            disabled={selected === 0 || change.isPending}
            disabledReason={t(selected === 0 ? "knowledge.source.selectFirst" : "knowledge.source.sending")}
            onClick={() => change.mutate(false)}
          >
            {t("knowledge.source.exclude")}
          </Button>
        </PermissionGuard>
        <PermissionGuard project={project} kind="KnowledgeSource" verb="propose">
          <Button
            variant="secondary"
            size="sm"
            disabled={selected === 0 || change.isPending}
            disabledReason={t(selected === 0 ? "knowledge.source.selectFirst" : "knowledge.source.sending")}
            onClick={() => change.mutate(true)}
          >
            {t("knowledge.source.include")}
          </Button>
        </PermissionGuard>
      </div>

      <Tabs
        id="knowledge-source"
        label={t("knowledge.source.views")}
        tabs={[
          { value: "pages", label: t("knowledge.source.pages") },
          { value: "documents", label: t("knowledge.source.documents") },
        ]}
        value={view}
        onChange={setView}
      />
      <div {...tabPanelProps("knowledge-source", view)}>
        {view === "pages" ? (
          <>
            <Field id="knowledge-find-pages" label={t("knowledge.source.findPages")} help={t("knowledge.source.findPagesHelp")} className="mb-3 max-w-xl">
              <Input id="knowledge-find-pages" type="search" maxLength={100} value={find} onChange={(event) => setFind(event.target.value)} />
            </Field>
            <PageLevel
              project={project}
              source={source}
              parent={null}
              q={found.length >= PAGE_QUERY_MIN ? found : undefined}
              selected={pages}
              onSelect={(id) => setPages((set) => toggle(set, id))}
              onOpen={setOpened}
            />
          </>
        ) : (
          <Documents
            project={project}
            source={source}
            selected={documents}
            onSelect={(id) => setDocuments((set) => toggle(set, id))}
            onOpen={setOpened}
          />
        )}
      </div>

      <Dialog
        open={opened !== null}
        onOpenChange={(open) => (open ? undefined : setOpened(null))}
        title={opened?.title ?? ""}
        size="lg"
        closeLabel={t("app.close")}
      >
        {opened?.what === "passages" ? (
          <Passages project={project} source={source} page={opened.page} document={opened.document} />
        ) : opened?.what === "links" ? (
          <Links project={project} source={source} page={opened.page} />
        ) : null}
      </Dialog>
    </section>
  );
}

/**
 * One level of the page tree; a branch opens the level below it in place. With `q`, the pages of
 * the whole source whose title or address holds it instead, as one flat list (UI-99).
 */
function PageLevel({
  project,
  source,
  parent,
  q,
  selected,
  onSelect,
  onOpen,
}: {
  project: string;
  source: string;
  parent: number | null;
  q?: string;
  selected: ReadonlySet<number>;
  onSelect: (id: number) => void;
  onOpen: (opened: Opened) => void;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const [open, setOpen] = useState<ReadonlySet<number>>(new Set());
  const level = useQuery({
    queryKey: q === undefined ? knowledgeKeys.pages(project, source, parent) : knowledgeKeys.found(project, source, q),
    queryFn: () => (q === undefined ? listPages(project, source, parent) : findPages(project, source, q)),
  });
  if (level.isLoading) return <p role="status">{t("app.loading")}</p>;
  if (level.isError) {
    return (
      <PageFailed error={level.error} onRetry={() => void level.refetch()} />
    );
  }
  const rows = level.data ?? [];
  if (rows.length === 0 && q !== undefined) {
    return <p role="status">{t("knowledge.source.noPagesFound", { q })}</p>;
  }
  if (rows.length === 0 && parent === null) {
    return <EmptyState title={t("knowledge.source.noPages")} description={t("knowledge.source.noPagesHint")} icon="search" />;
  }
  return (
    <ul className={parent === null ? "space-y-1" : "ml-6 space-y-1 border-l border-border pl-3"}>
      {rows.map((row) => {
        const expanded = open.has(row.id);
        const childrenId = `knowledge-children-${row.id}`;
        return (
          <li key={row.id}>
            <div className="flex flex-wrap items-center gap-2 py-1">
              {row.children > 0 && q === undefined ? (
                <Button
                  variant="ghost"
                  size="sm"
                  aria-expanded={expanded}
                  aria-controls={childrenId}
                  aria-label={t(expanded ? "knowledge.source.collapse" : "knowledge.source.expand", {
                    url: row.url,
                    count: row.children,
                  })}
                  onClick={() =>
                    setOpen((set) => {
                      const next = new Set(set);
                      if (next.has(row.id)) next.delete(row.id);
                      else next.add(row.id);
                      return next;
                    })
                  }
                >
                  {expanded ? "▾" : "▸"} {row.children}
                </Button>
              ) : (
                <span className="inline-block w-12" aria-hidden="true" />
              )}
              <Checkbox
                label={<span className="sr-only">{t("knowledge.source.selectPage", { url: row.url })}</span>}
                checked={selected.has(row.id)}
                onChange={() => onSelect(row.id)}
              />
              <Titled row={row} />
              <InclusionBadge included={row.included} excludedBy={row.excludedBy} />
              {row.status !== "fetched" ? <Badge tone={row.status === "failed" ? "danger" : "neutral"}>{t(`knowledge.status.${row.status}`)}</Badge> : null}
              {row.language ? <Badge mono>{row.language}</Badge> : null}
              <span className="text-caption text-fg-muted">
                {t("knowledge.source.pageCounts", { passages: row.passages, documents: row.documents })}
                {" · "}
                {timeText(row.fetchedAt, i18n.language)}
                {row.modified ? ` · ${t("knowledge.source.modified", { at: timeText(row.modified, i18n.language) })}` : null}
              </span>
              <Button
                variant="ghost"
                size="sm"
                aria-label={t("knowledge.source.passagesOf", { url: row.url })}
                onClick={() => onOpen({ what: "passages", title: row.url, page: row.id })}
              >
                {t("knowledge.source.passages")}
              </Button>
              <Button
                variant="ghost"
                size="sm"
                aria-label={t("knowledge.source.linksOf", { url: row.url })}
                onClick={() => onOpen({ what: "links", title: row.url, page: row.id })}
              >
                {t("knowledge.source.links")}
              </Button>
            </div>
            {expanded ? (
              <div id={childrenId}>
                <PageLevel
                  project={project}
                  source={source}
                  parent={row.id}
                  selected={selected}
                  onSelect={onSelect}
                  onOpen={onOpen}
                />
              </div>
            ) : null}
          </li>
        );
      })}
    </ul>
  );
}

function Documents({
  project,
  source,
  selected,
  onSelect,
  onOpen,
}: {
  project: string;
  source: string;
  selected: ReadonlySet<number>;
  onSelect: (id: number) => void;
  onOpen: (opened: Opened) => void;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const documents = useQuery({
    queryKey: knowledgeKeys.documents(project, source),
    queryFn: () => listDocuments(project, source),
  });
  if (documents.isLoading) return <p role="status">{t("app.loading")}</p>;
  if (documents.isError) {
    return (
      <PageFailed error={documents.error} onRetry={() => void documents.refetch()} />
    );
  }
  const rows: DocumentRow[] = documents.data ?? [];
  if (rows.length === 0) {
    return <EmptyState title={t("knowledge.source.noDocuments")} description={t("knowledge.source.noDocumentsHint")} icon="search" />;
  }
  return (
    <Table data-records="" caption={t("knowledge.source.documents")}>
      <TableHead>
        <TableHeaderCell>
          <span className="sr-only">{t("knowledge.source.select")}</span>
        </TableHeaderCell>
        <TableHeaderCell>{t("knowledge.column.document")}</TableHeaderCell>
        <TableHeaderCell align="right">{t("knowledge.column.size")}</TableHeaderCell>
        <TableHeaderCell align="right">{t("knowledge.column.pdfPages")}</TableHeaderCell>
        <TableHeaderCell>{t("knowledge.column.modified")}</TableHeaderCell>
        <TableHeaderCell>{t("knowledge.column.state")}</TableHeaderCell>
        <TableHeaderCell align="right">{t("knowledge.column.passages")}</TableHeaderCell>
        <TableHeaderCell align="right">
          <span className="sr-only">{t("approvals.actions")}</span>
        </TableHeaderCell>
      </TableHead>
      <TableBody>
        {rows.map((row) => (
          <TableRow key={row.id}>
            <TableCell>
              <Checkbox
                label={<span className="sr-only">{t("knowledge.source.selectDocument", { url: row.url })}</span>}
                checked={selected.has(row.id)}
                onChange={() => onSelect(row.id)}
              />
            </TableCell>
            <TableCell>
              <Titled row={row} />
              {row.offDomain ? (
                <Badge tone="warning" className="ml-2">
                  {t("knowledge.source.offDomain")}
                </Badge>
              ) : null}
            </TableCell>
            <TableCell align="right">{bytesText(row.bytes, i18n.language)}</TableCell>
            <TableCell align="right">{row.pages ?? "—"}</TableCell>
            <TableCell>{timeText(row.modified, i18n.language)}</TableCell>
            <TableCell>
              <InclusionBadge included={row.included} excludedBy={row.excludedBy} />
              {row.status !== "fetched" ? (
                <Badge tone={row.status === "failed" ? "danger" : "neutral"} className="ml-2">
                  {t(`knowledge.status.${row.status}`)}
                </Badge>
              ) : null}
            </TableCell>
            <TableCell align="right">{row.passages}</TableCell>
            <TableCell align="right">
              <Button
                variant="ghost"
                size="sm"
                aria-label={t("knowledge.source.passagesOf", { url: row.url })}
                onClick={() => onOpen({ what: "passages", title: row.url, document: row.id })}
              >
                {t("knowledge.source.passages")}
              </Button>
            </TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  );
}

function Passages({ project, source, page, document }: { project: string; source: string; page?: number; document?: number }): JSX.Element {
  const { t } = useTranslation();
  const owner = page !== undefined ? { page } : { document };
  const passages = useQuery({
    queryKey: knowledgeKeys.passages(project, source, owner),
    queryFn: () => listPassages(project, source, owner),
  });
  if (passages.isLoading) return <p role="status">{t("app.loading")}</p>;
  if (passages.isError) {
    return (
      <PageFailed error={passages.error} onRetry={() => void passages.refetch()} />
    );
  }
  const rows = passages.data ?? [];
  if (rows.length === 0) return <p>{t("knowledge.source.noPassages")}</p>;
  return (
    <ol className="space-y-3" aria-label={t("knowledge.source.passages")}>
      {rows.map((passage) => (
        <li key={passage.ordinal} className="rounded-md border border-border p-3">
          <p className="whitespace-pre-wrap text-body [overflow-wrap:anywhere]" lang={passage.lang ?? undefined}>
            {passage.text}
          </p>
        </li>
      ))}
    </ol>
  );
}

function Links({ project, source, page }: { project: string; source: string; page: number }): JSX.Element {
  const { t } = useTranslation();
  const links = useQuery({
    queryKey: knowledgeKeys.links(project, source, page),
    queryFn: () => listLinks(project, source, page),
  });
  if (links.isLoading) return <p role="status">{t("app.loading")}</p>;
  if (links.isError) {
    return (
      <PageFailed error={links.error} onRetry={() => void links.refetch()} />
    );
  }
  const rows = links.data ?? [];
  if (rows.length === 0) return <p>{t("knowledge.source.noLinks")}</p>;
  return (
    <ul className="space-y-1" aria-label={t("knowledge.source.links")}>
      {rows.map((link) => (
        <li key={link.url} className="flex flex-wrap items-center gap-2">
          <Badge>{t(`knowledge.link.${link.kind}`)}</Badge>
          <ExternalLink href={link.url} className="font-mono [overflow-wrap:anywhere]">
            {link.url}
          </ExternalLink>
        </li>
      ))}
    </ul>
  );
}

/**
 * A page's or document's own title over its address, or the address alone when it states none (AG-120).
 * An uploaded file has no address to open: its name and page, never `upload:…` (T-4869).
 */
function Titled({ row }: { row: { url: string; title?: string | null } }): JSX.Element {
  const file = uploadedFile(row.url);
  if (file) return <UploadedTitled title={row.title} file={file} />;
  if (!row.title) {
    return (
      <ExternalLink href={row.url} className="font-mono [overflow-wrap:anywhere]">
        {row.url}
      </ExternalLink>
    );
  }
  return (
    <span className="inline-flex min-w-0 flex-col">
      <ExternalLink href={row.url} className="[overflow-wrap:anywhere]">
        {row.title}
      </ExternalLink>
      <span className="font-mono text-caption text-fg-muted [overflow-wrap:anywhere]">{row.url}</span>
    </span>
  );
}

/** An uploaded file's passage: its title over the file's name and page. */
function UploadedTitled({ title, file }: { title?: string | null; file: { name: string; page: number | null } }): JSX.Element {
  const { t } = useTranslation();
  return (
    <span className="inline-flex min-w-0 flex-col">
      {title ? <span className="[overflow-wrap:anywhere]">{title}</span> : null}
      <span className="font-mono text-caption text-fg-muted [overflow-wrap:anywhere]">
        {file.page === null ? file.name : t("knowledge.source.filePage", { name: file.name, page: file.page })}
      </span>
    </span>
  );
}

/**
 * "Vyskúšať vyhľadávanie" (UI-99, T-4659): the search the MCP tool `search_documents` runs, over
 * this one source, with the hits it would answer. Internal passages count too, as an internal
 * deployment would read them.
 */
function TrySearch({ project, source }: { project: string; source: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const [text, setText] = useState("");
  const [asked, setAsked] = useState<string | null>(null);
  const hits = useQuery({
    queryKey: knowledgeKeys.search(project, source, asked ?? ""),
    queryFn: () => trySearch(project, source, asked ?? ""),
    enabled: asked !== null,
  });
  const submit = (event: FormEvent) => {
    event.preventDefault();
    const q = text.trim();
    if (q) setAsked(q);
  };
  return (
    <section aria-labelledby="knowledge-try-search" className="space-y-3 rounded-md border border-border p-4">
      <h2 id="knowledge-try-search" className="text-title">
        {t("knowledge.trySearch.title")}
      </h2>
      <p className="text-caption text-fg-muted">{t("knowledge.trySearch.intro")}</p>
      <form onSubmit={submit} className="flex flex-wrap items-end gap-2">
        <Field id="knowledge-try-search-q" label={t("knowledge.trySearch.query")} className="min-w-0 flex-1">
          <Input id="knowledge-try-search-q" type="search" maxLength={300} value={text} onChange={(event) => setText(event.target.value)} />
        </Field>
        <Button type="submit" disabled={!text.trim() || hits.isFetching} disabledReason={t(text.trim() ? "knowledge.trySearch.searching" : "knowledge.trySearch.typeFirst")}>
          {t("knowledge.trySearch.run")}
        </Button>
      </form>
      {asked === null ? null : hits.isLoading ? (
        <p role="status">{t("app.loading")}</p>
      ) : hits.isError ? (
        <PageFailed error={hits.error} onRetry={() => void hits.refetch()} />
      ) : (hits.data ?? []).length === 0 ? (
        <p role="status">{t("knowledge.trySearch.none", { q: asked })}</p>
      ) : (
        <ol className="space-y-3" aria-label={t("knowledge.trySearch.hits", { q: asked })}>
          {(hits.data ?? []).map((hit, at) => (
            <li key={`${hit.url}-${at}`} className="space-y-1 rounded-md border border-border p-3">
              <div className="flex flex-wrap items-center gap-2">
                <Titled row={hit} />
                {hit.language ? <Badge mono>{hit.language}</Badge> : null}
                {hit.modified ? (
                  <span className="text-caption text-fg-muted">
                    {t("knowledge.source.modified", { at: timeText(hit.modified, i18n.language) })}
                  </span>
                ) : null}
              </div>
              {hit.description ? <p className="text-caption text-fg-muted">{hit.description}</p> : null}
              <p className="whitespace-pre-wrap text-body [overflow-wrap:anywhere]" lang={hit.language ?? undefined}>
                {hit.text}
              </p>
            </li>
          ))}
        </ol>
      )}
    </section>
  );
}
