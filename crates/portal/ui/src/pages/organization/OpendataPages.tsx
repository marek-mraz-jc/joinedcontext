import { useEffect, useRef, useState } from "react";
import type { JSX } from "react";
import { useMutation, useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { ApiError, queryKeys } from "../../api/client";
import { door, doorAction } from "../../api/odata";
import type { RestAnswer } from "../../api/odata";
import { ORG_NAMESPACE } from "../../api/manifest";
import type { Change } from "../../api/manifest";
import type { components } from "../../api/schema";
import { ChangeNotice } from "../../components/ChangeNotice";
import {
  Alert,
  Badge,
  Button,
  buttonClass,
  ConfirmDialog,
  Field,
  InlineError,
  PageFailed,
  PageLoading,
  Tabs,
  tabPanelProps,
  Textarea,
} from "../../components/ui";
import { useIsPhone } from "../../phone";
import { applyFormat, previewElements } from "./pageMarkdown";
import type { Format } from "./pageMarkdown";

type Pages = components["schemas"]["OpenDataPages"];
type PageText = components["schemas"]["OpenDataPageText"];
type ChangeProposal = components["schemas"]["ChangeProposal"];

const QUERY = ["opendataPages"] as const;
const OPEN = ["PendingApproval", "Deploying"];
const FORMATS: readonly Format[] = ["bold", "heading", "list", "link", "code"];
/** i18next reads a dot as a key path: page ids reach the locale through this fixed map. */
const PAGE_KEYS: Record<string, string> = {
  connector: "connector",
  privacy: "privacy",
  terms: "terms",
  support: "support",
  accessibility: "accessibility",
};

const keyOf = (page: string, lang: string) => `${page}/${lang}`;

function problem(error: unknown): string {
  return error instanceof ApiError ? (error.problem?.detail ?? error.message) : String(error);
}

/** The open Changes of the Organization manifest, and the page texts each would change. */
function usePending(data: Pages | undefined): { changes: ChangeProposal[]; pending: Map<string, string> } {
  const list = useQuery({
    queryKey: queryKeys.changes(ORG_NAMESPACE),
    enabled: data !== undefined,
    queryFn: async () =>
      (await door("portal_organization_change_list", {})) as RestAnswer<"list_changes">,
  });
  const changes = (list.data?.items ?? []).filter((change) => {
    const params = change.summary.params as Record<string, unknown>;
    return OPEN.includes(change.status.phase) && params.kind === "Organization" && params.name === data?.organization;
  });
  const proposed = useQueries({
    queries: changes.map((change) => ({
      queryKey: [...queryKeys.change(ORG_NAMESPACE, change.metadata.name), "manifests"],
      queryFn: async () =>
        (await door("portal_organization_change_manifests", {
          id: change.metadata.name,
        })) as RestAnswer<"change_manifests">,
    })),
  });
  const pending = new Map<string, string>();
  proposed.forEach((answer, at) => {
    const manifest = answer.data?.items.find((item) => item.kind === "Organization");
    if (!manifest || !data) return;
    const texts =
      (manifest.spec as { appearance?: { pages?: { page: string; lang: string; markdown: string }[] } }).appearance
        ?.pages ?? [];
    for (const row of data.pages) {
      const next = texts.find((text) => text.page === row.page && text.lang === row.lang)?.markdown;
      if (next !== row.markdown) pending.set(keyOf(row.page, row.lang), changes[at].metadata.name);
    }
  });
  return { changes, pending };
}

/**
 * Administration → Open data pages (T-7041, T-7031): the texts of the Organization's open data
 * portal pages per language, each the built-in one or the Organization's own Markdown, edited
 * with a preview the catalogue's own renderer draws and published as a Change an approver merges.
 * The server answers only the Organization's administrators and checks every write again.
 */
export function OpendataPages(): JSX.Element {
  const { t } = useTranslation();
  const pages = useQuery({
    queryKey: QUERY,
    queryFn: async () => (await door("portal_opendata_pages", {})) as RestAnswer<"list_opendata_pages">,
    retry: false,
  });
  const { changes, pending } = usePending(pages.data);
  const [open, setOpen] = useState<PageText | null>(null);

  if (pages.isPending) return <PageLoading label={t("app.loading")} />;
  if (pages.isError) {
    return pages.error instanceof ApiError && pages.error.status === 403 ? (
      <section aria-labelledby="opendata-pages-title" className="flex flex-col gap-4">
        <h2 id="opendata-pages-title" className="text-title font-semibold text-fg">
          {t("opendataPages.title")}
        </h2>
        <Alert tone="info">{t("opendataPages.refused.list")}</Alert>
      </section>
    ) : (
      <PageFailed error={pages.error} onRetry={() => void pages.refetch()} />
    );
  }
  const data = pages.data;
  return (
    <section aria-labelledby="opendata-pages-title" className="flex flex-col gap-4">
      <div className="flex flex-col gap-1">
        <h2 id="opendata-pages-title" className="text-title font-semibold text-fg">
          {t("opendataPages.title")}
        </h2>
        <p className="text-body text-fg-muted">{t("opendataPages.lead", { organization: data.organization })}</p>
      </div>
      {changes.length > 0 && !open ? (
        <Alert tone="warning">
          <span className="flex flex-wrap items-center gap-2">
            {t("opendataPages.openChange")}
            {changes.map((change) => (
              <Link
                key={change.metadata.name}
                to="/projects/$project/approvals/$id"
                params={{ project: ORG_NAMESPACE, id: change.metadata.name }}
                className="font-mono text-primary-soft-fg underline"
              >
                {change.metadata.name}
              </Link>
            ))}
          </span>
        </Alert>
      ) : null}
      {open ? (
        <PageEditor
          key={keyOf(open.page, open.lang)}
          text={open}
          maxBytes={data.maxBytes}
          pendingChange={pending.get(keyOf(open.page, open.lang)) ?? changes[0]?.metadata.name}
          ownPending={pending.has(keyOf(open.page, open.lang))}
          onClose={() => setOpen(null)}
        />
      ) : (
        <PageList data={data} pending={pending} onEdit={setOpen} />
      )}
      <PagesHistory organization={data.organization} />
    </section>
  );
}

/** Every page in every language, what the portal shows for it, and the way into its editor. */
function PageList({
  data,
  pending,
  onEdit,
}: {
  data: Pages;
  pending: Map<string, string>;
  onEdit: (text: PageText) => void;
}): JSX.Element {
  const { t } = useTranslation();
  const names = [...new Set(data.pages.map((text) => text.page))];
  const own = data.pages.some((text) => text.markdown !== undefined);
  return (
    <div className="flex flex-col gap-3">
      {own ? null : <Alert tone="info">{t("opendataPages.noneOwn")}</Alert>}
      <ul className="grid gap-3 lg:grid-cols-2" data-testid="opendata-pages-list">
        {names.map((name) => (
          <li key={name} className="flex flex-col gap-2 rounded-md border border-border bg-surface p-4">
            <h3 className="text-body font-semibold text-fg">
              {t(`opendataPages.page.${PAGE_KEYS[name] ?? "other"}`, { page: name })}
            </h3>
            <ul className="flex flex-col divide-y divide-border">
              {data.pages
                .filter((text) => text.page === name)
                .map((text) => (
                  <li key={text.lang} className="flex flex-wrap items-center justify-between gap-2 py-2">
                    <span className="flex min-w-0 flex-wrap items-center gap-2">
                      <span className="font-mono text-caption uppercase text-fg-muted">{text.lang}</span>
                      <span className="truncate text-body text-fg">{text.title}</span>
                      {text.markdown === undefined ? (
                        <Badge tone="neutral">{t("opendataPages.builtIn")}</Badge>
                      ) : (
                        <Badge tone="primary">{t("opendataPages.own")}</Badge>
                      )}
                      {pending.has(keyOf(text.page, text.lang)) ? (
                        <Badge tone="warning">{t("opendataPages.pending")}</Badge>
                      ) : null}
                    </span>
                    <Button
                      size="sm"
                      variant="secondary"
                      aria-label={t("opendataPages.editLabel", { title: text.title, lang: text.lang })}
                      onClick={() => onEdit(text)}
                    >
                      {t("opendataPages.edit")}
                    </Button>
                  </li>
                ))}
            </ul>
          </li>
        ))}
      </ul>
    </div>
  );
}

/** The preview the catalogue's renderer draws of `markdown`, asked again a moment after typing stops. */
function usePreview(markdown: string) {
  const [asked, setAsked] = useState(markdown);
  useEffect(() => {
    const timer = setTimeout(() => setAsked(markdown), 400);
    return () => clearTimeout(timer);
  }, [markdown]);
  return useQuery({
    queryKey: [...QUERY, "preview", asked],
    enabled: asked.trim() !== "",
    retry: false,
    queryFn: async () =>
      (await doorAction("portal_opendata_page_preview", {
        body: { markdown: asked },
      })) as RestAnswer<"preview_opendata_page">,
  });
}

function Preview({ markdown }: { markdown: string }): JSX.Element {
  const { t } = useTranslation();
  const preview = usePreview(markdown);
  if (markdown.trim() === "") return <p className="text-body text-fg-muted">{t("opendataPages.previewEmpty")}</p>;
  if (preview.isError) return <InlineError>{problem(preview.error)}</InlineError>;
  if (!preview.data) return <p className="text-body text-fg-muted">{t("app.loading")}</p>;
  return (
    <article className="flex flex-col gap-3 text-body text-fg [&_a]:text-primary-soft-fg [&_a]:underline [&_code]:font-mono [&_ol]:list-decimal [&_ol]:pl-5 [&_pre]:overflow-x-auto [&_pre]:rounded-md [&_pre]:bg-surface-subtle [&_pre]:p-3 [&_ul]:list-disc [&_ul]:pl-5">
      {preview.data.title ? <p className="text-title font-semibold">{preview.data.title}</p> : null}
      {previewElements(preview.data.html)}
    </article>
  );
}

/** Bold, heading, list, link and code, each written as Markdown around the selection. */
function Toolbar({ onFormat }: { onFormat: (format: Format) => void }): JSX.Element {
  const { t } = useTranslation();
  return (
    <div
      role="toolbar"
      aria-label={t("opendataPages.toolbar")}
      aria-controls="opendata-page-markdown"
      className="flex flex-wrap gap-1"
    >
      {FORMATS.map((format) => (
        <Button key={format} size="sm" variant="ghost" onClick={() => onFormat(format)}>
          {t(`opendataPages.format.${format}`)}
        </Button>
      ))}
    </div>
  );
}

/** Where a write ended: its Change, or why the server refused it, in words to act on. */
function Outcome({ change, error }: { change: Change | null; error: unknown }): JSX.Element | null {
  const { t } = useTranslation();
  if (change) {
    return (
      <div className="flex flex-col gap-2">
        <ChangeNotice change={change} project={ORG_NAMESPACE} />
        <p className="text-caption text-fg-muted">{t("opendataPages.waits")}</p>
      </div>
    );
  }
  if (!(error instanceof ApiError) || error.status === 422) return null;
  const known = error.status === 403 || error.status === 409;
  return (
    <Alert tone={error.status === 409 ? "warning" : "danger"}>
      {known ? t(`opendataPages.refused.${error.status}`, { detail: problem(error) }) : problem(error)}
    </Alert>
  );
}

function PageEditor({
  text,
  maxBytes,
  pendingChange,
  ownPending,
  onClose,
}: {
  text: PageText;
  maxBytes: number;
  /** The open Change of the Organization: this page's text in it, or another edit (a publish would meet a 409). */
  pendingChange: string | undefined;
  ownPending: boolean;
  onClose: () => void;
}): JSX.Element {
  const { t } = useTranslation();
  const phone = useIsPhone();
  const queryClient = useQueryClient();
  const area = useRef<HTMLTextAreaElement>(null);
  const [markdown, setMarkdown] = useState(text.markdown ?? text.builtIn);
  const [view, setView] = useState<"write" | "preview">("write");
  const [confirming, setConfirming] = useState(false);
  const path = { page: text.page, lang: text.lang };
  const settle = async () => {
    setConfirming(false);
    await queryClient.invalidateQueries({ queryKey: QUERY });
    await queryClient.invalidateQueries({ queryKey: queryKeys.changes(ORG_NAMESPACE) });
  };
  const publish = useMutation({
    mutationFn: async () =>
      (await doorAction("portal_opendata_page_set", { ...path, body: { markdown } })) as RestAnswer<"publish_opendata_page">,
    onSuccess: settle,
    // A refused text goes back to the field, on a phone from the preview tab too.
    onError: (failure) => {
      if (!(failure instanceof ApiError) || failure.status !== 422) return;
      setView("write");
      requestAnimationFrame(() => area.current?.focus());
    },
  });
  const revert = useMutation({
    mutationFn: async () =>
      (await doorAction("portal_opendata_page_revert", path)) as RestAnswer<"revert_opendata_page">,
    onSuccess: settle,
  });
  const write = publish.isSuccess ? publish : revert;
  const done = (write.data as Change | undefined) ?? null;
  const error = publish.error ?? revert.error;
  // The server names the manifest field; the person reads the reason alone, under the field.
  const invalid =
    error instanceof ApiError && error.status === 422
      ? problem(error).replace(/^spec\.appearance\.pages\[\d+\]\.\w+:\s*/, "")
      : null;
  // Publish meets a 409 while a Change of the Organization is open: say so once, beside the buttons.
  const blocked = pendingChange !== undefined && !done;
  const bytes = new TextEncoder().encode(markdown).length;
  const tooLong = bytes > maxBytes;

  const format = (kind: Format) => {
    const box = area.current;
    const edit = applyFormat(
      markdown,
      box?.selectionStart ?? markdown.length,
      box?.selectionEnd ?? markdown.length,
      kind,
    );
    setMarkdown(edit.text);
    requestAnimationFrame(() => {
      box?.focus();
      box?.setSelectionRange(edit.start, edit.end);
    });
  };

  const editor = (
    <div className="flex flex-col gap-2">
      <Toolbar onFormat={format} />
      <Field
        id="opendata-page-markdown"
        label={t("opendataPages.markdown")}
        help={t("opendataPages.markdownHint", { max: Math.floor(maxBytes / 1024) })}
        errors={invalid ? [invalid] : undefined}
      >
        <Textarea
          ref={area}
          id="opendata-page-markdown"
          rows={phone ? 14 : 22}
          value={markdown}
          onChange={(event) => setMarkdown(event.target.value)}
          className="font-mono text-caption"
        />
      </Field>
      <p id="opendata-page-size" className={tooLong ? "text-caption text-danger" : "text-caption text-fg-muted"}>
        {tooLong
          ? t("opendataPages.sizeOver", { bytes, max: maxBytes, over: bytes - maxBytes })
          : t("opendataPages.size", { bytes, max: maxBytes })}
      </p>
    </div>
  );
  const preview = (
    <div className="flex flex-col gap-2 rounded-md border border-border bg-surface p-4">
      <h4 className="text-caption font-semibold uppercase tracking-wide text-fg-subtle">
        {t("opendataPages.preview")}
      </h4>
      <Preview markdown={markdown} />
    </div>
  );
  const views = [
    { value: "write" as const, label: t("opendataPages.write") },
    { value: "preview" as const, label: t("opendataPages.preview") },
  ];

  return (
    <div className="flex flex-col gap-4" data-testid="opendata-page-editor">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h3 className="text-body font-semibold text-fg">
          {t("opendataPages.editing", { title: text.title, lang: text.lang.toUpperCase() })}
        </h3>
        <Button size="sm" variant="ghost" onClick={onClose}>
          {t("opendataPages.back")}
        </Button>
      </div>
      {blocked ? (
        <Alert tone="warning" id="opendata-page-blocked">
          <span className="flex flex-wrap items-center gap-2">
            {t(ownPending ? "opendataPages.pendingHere" : "opendataPages.openChange")}
            <Link
              to="/projects/$project/approvals/$id"
              params={{ project: ORG_NAMESPACE, id: pendingChange }}
              className={buttonClass("secondary", "sm")}
            >
              {t("changes.review")}
            </Link>
          </span>
        </Alert>
      ) : null}
      {phone ? (
        <>
          <Tabs id="opendata-page-view" label={t("opendataPages.views")} tabs={views} value={view} onChange={setView} />
          <div {...tabPanelProps("opendata-page-view", view)}>{view === "write" ? editor : preview}</div>
        </>
      ) : (
        <div className="grid grid-cols-2 gap-4">
          {editor}
          {preview}
        </div>
      )}
      <Outcome change={done} error={error} />
      {done ? null : (
        <div className="flex flex-wrap gap-2">
          <Button
            variant="primary"
            loading={publish.isPending}
            disabled={blocked || tooLong}
            aria-describedby={blocked ? "opendata-page-blocked" : tooLong ? "opendata-page-size" : undefined}
            onClick={() => publish.mutate()}
          >
            {t("opendataPages.publish")}
          </Button>
          {text.markdown !== undefined ? (
            <Button
              variant="secondary"
              disabled={blocked}
              aria-describedby={blocked ? "opendata-page-blocked" : undefined}
              onClick={() => setConfirming(true)}
            >
              {t("opendataPages.revert")}
            </Button>
          ) : null}
        </div>
      )}
      <ConfirmDialog
        open={confirming}
        onOpenChange={setConfirming}
        title={t("opendataPages.revertTitle")}
        description={t("opendataPages.revertBody", { title: text.title, lang: text.lang.toUpperCase() })}
        confirmLabel={t("opendataPages.revert")}
        pending={revert.isPending}
        onConfirm={() => revert.mutate()}
      />
    </div>
  );
}

/** Whether a closed Change wrote the page texts: a field under `spec.appearance.pages`. */
function touchesPages(fields: { path: string; from?: unknown; to?: unknown }[] | null | undefined): boolean {
  return (fields ?? []).some(
    (field) =>
      field.path.startsWith("spec.appearance.pages") ||
      (field.path.startsWith("spec.appearance") && JSON.stringify([field.from, field.to]).includes('"pages"')),
  );
}

/**
 * The closed Changes of the Organization manifest that wrote its page texts, newest first, with
 * who proposed each and when it closed. ponytail: the forge's first history page only; the
 * Approvals history holds the rest.
 */
function PagesHistory({ organization }: { organization: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "sk";
  const history = useQuery({
    queryKey: [...queryKeys.changes(ORG_NAMESPACE), "opendataPagesHistory", organization],
    queryFn: async () =>
      (await door("portal_organization_change_history", {
        page: 1,
        kind: "Organization",
        name: organization,
      })) as RestAnswer<"list_change_history">,
  });
  const closed = (history.data?.items ?? []).filter(
    (change) => (change.summary.params as Record<string, unknown>).name === organization,
  );
  const details = useQueries({
    queries: closed.map((change) => ({
      queryKey: queryKeys.change(ORG_NAMESPACE, change.metadata.name),
      queryFn: async () =>
        (await door("portal_organization_change_get", {
          id: change.metadata.name,
        })) as RestAnswer<"get_change">,
    })),
  });
  const shown = closed.filter((_, at) => touchesPages(details[at]?.data?.planFields));
  const loading = history.isPending || details.some((detail) => detail.isPending);
  const when = new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" });
  return (
    <section aria-labelledby="opendata-pages-history" className="flex flex-col gap-2">
      <h3 id="opendata-pages-history" className="text-body font-semibold text-fg">
        {t("opendataPages.history")}
      </h3>
      {history.isError ? <InlineError>{problem(history.error)}</InlineError> : null}
      {loading && !history.isError ? <p className="text-caption text-fg-muted">{t("app.loading")}</p> : null}
      {!loading && shown.length === 0 ? (
        <p className="text-caption text-fg-muted">{t("opendataPages.historyEmpty")}</p>
      ) : null}
      {shown.length > 0 ? (
        <ul
          className="flex flex-col divide-y divide-border rounded-md border border-border"
          data-testid="opendata-pages-history"
        >
          {shown.map((change) => (
            <li key={change.metadata.name} className="flex flex-wrap items-center justify-between gap-2 p-3">
              <span className="text-body text-fg">
                {t("opendataPages.historyRow", {
                  who: change.author.name,
                  at: when.format(new Date(change.decision?.at ?? change.createdAt)),
                  phase: t(`opendataPages.phase.${change.status.phase === "Rejected" ? "rejected" : "merged"}`),
                })}
              </span>
              <Link
                to="/projects/$project/approvals/$id"
                params={{ project: ORG_NAMESPACE, id: change.metadata.name }}
                className={buttonClass("ghost", "sm")}
              >
                {t("changes.open")}
              </Link>
            </li>
          ))}
        </ul>
      ) : null}
    </section>
  );
}
