import { useEffect, useState } from "react";
import { flushSync } from "react-dom";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { door } from "../../api/odata";
import type { RestAnswer } from "../../api/odata";
import { isChange, ORG_NAMESPACE } from "../../api/manifest";
import type { Change } from "../../api/manifest";
import { ChangeNotice } from "../../components/ChangeNotice";
import {
  Alert,
  Button,
  Card,
  Checkbox,
  EmptyState,
  Field,
  InlineError,
  Input,
  RadioGroup,
  RowActions,
  Select,
  Textarea,
} from "../../components/ui";
import { DATA_THEMES } from "../endpoints/catalog";
import { PagesHistory, usePreview } from "../organization/OpendataPages";
import { previewElements } from "../organization/pageMarkdown";
import type { PortalSpec } from "./PortalsArea";
import { linesOf, readDatasets, usePortalChange } from "./PortalSources";
import {
  addRow,
  addToRow,
  BLOCK_TYPES,
  cleanPages,
  duplicateBlock,
  faultsOfPages,
  HOME,
  MAX_GALLERY,
  MAX_ITEMS,
  MAX_PAGES,
  MAX_ROW_BLOCKS,
  MAX_ROWS,
  moveBlock,
  newBlock,
  PORTAL_NUMBERS,
  removeBlock,
  setBlock,
  starterHome,
  textOf,
  withText,
} from "./portalPages";
import type { At, BlockType, PortalBlock, PortalPage, Texts } from "./portalPages";

const FORMATS = ["CSV", "JSON", "GEOJSON", "XLSX", "XML", "PDF", "WMS", "OGCFEAT"] as const;
const DNS_LABEL = /^[a-z0-9]([-a-z0-9]{0,61}[a-z0-9])?$/;

type FileRow = RestAnswer<"list_portal_files">["items"][number];

/** A closed Change touched the portal's pages. */
export function touchesLayout(fields: { path: string }[] | null | undefined): boolean {
  return (fields ?? []).some((field) => field.path.startsWith("spec.pages"));
}

/** The datasets and files a block may name: only what the server lists for this portal. */
interface Choices {
  datasets: { name: string; title: string }[];
  files: FileRow[];
}

/**
 * The Layout tab of a portal (T-7434): its home and further pages as rows of blocks, edited
 * here, previewed at 1440 and 400 px, and published as one Change an approver accepts; the
 * portal shows it within seconds of the approval. A portal without pages shows the built-in home.
 */
export function PortalLayoutTab({ name, spec }: { name: string; spec: PortalSpec }): JSX.Element {
  const { t } = useTranslation();
  const stored = (spec.pages ?? []) as PortalPage[];
  const languages = spec.languages?.length ? spec.languages : ["sk"];
  const [pages, setPages] = useState<PortalPage[]>(stored);
  const [current, setCurrent] = useState(0);
  const [lang, setLang] = useState(languages[0] ?? "sk");
  const [change, setChange] = useState<Change | null>(null);
  const [refusal, setRefusal] = useState<string[]>([]);
  const [tried, setTried] = useState(false);
  const propose = usePortalChange(name);
  const datasets = useQuery({ queryKey: ["portalDatasets", name], retry: false, queryFn: () => readDatasets(name) });
  const files = useQuery({
    queryKey: ["portalFiles", name],
    retry: false,
    queryFn: async () => (await door("portal_portal_files", { name })) as RestAnswer<"list_portal_files">,
  });
  const choices: Choices = {
    datasets: (datasets.data?.items ?? []).filter((d) => !d.hidden).map((d) => ({ name: d.name, title: d.title || d.name })),
    files: files.data?.items ?? [],
  };
  const cleaned = cleanPages(pages);
  const dirty = JSON.stringify(cleaned) !== JSON.stringify(cleanPages(stored));
  const faults = faultsOfPages(pages);
  const server = refusedFields(refusal);
  const shownFaults = {
    ...Object.fromEntries(Object.entries(tried ? faults : {}).map(([path, key]) => [path, t(`portals.layout.fault.${key}`)])),
    ...server.fields,
  };
  const page = pages[current];
  const marked = Object.keys(shownFaults).length;
  // Below xl the preview would sit under the whole form: it opens on demand, above the form.
  const [showPreview, setShowPreview] = useState(false);
  const markedList = Object.keys(shownFaults)
    .map((path) => markedField(path, pages, lang, t))
    .filter((field): field is NonNullable<typeof field> => field !== null);
  const fieldLinks =
    markedList.length > 0 ? (
      <ul className="mt-1 flex flex-col items-start gap-1">
        {markedList.map((field) => (
          <li key={field.name}>
            <Button
              variant="ghost"
              className="underline"
              onClick={() => {
                flushSync(() => setCurrent(field.page));
                const chosen = (field.target ? document.getElementById(field.target) : null) ?? document.querySelector<HTMLElement>("[aria-invalid='true']");
                chosen?.scrollIntoView?.({ block: "center" });
                chosen?.focus({ preventScroll: true });
              }}
            >
              {field.name}
            </Button>
          </li>
        ))}
      </ul>
    ) : null;
  useEffect(() => {
    if (marked === 0) return;
    const first = document.querySelector<HTMLElement>("[aria-invalid='true']");
    first?.scrollIntoView?.({ block: "center" });
    first?.focus({ preventScroll: true });
  }, [marked, current]);

  const publish = () => {
    setTried(true);
    setRefusal([]);
    if (Object.keys(faults).length > 0) return;
    propose.mutate(
      { ...spec, pages: cleaned.length > 0 ? cleaned : undefined } as PortalSpec,
      {
        onSuccess: (result) => {
          if (isChange(result)) setChange(result);
          setTried(false);
        },
        onError: (error) => {
          const lines = linesOf(error, t("app.error.generic"));
          setRefusal(lines);
          const first = Object.keys(refusedFields(lines).fields)[0];
          const onPage = first ? Number(/^spec\.pages\[(\d+)\]/.exec(first)?.[1]) : NaN;
          if (Number.isInteger(onPage) && onPage < pages.length) setCurrent(onPage);
        },
      },
    );
  };
  const discard = () => {
    setPages(stored);
    setCurrent(0);
    setTried(false);
    setRefusal([]);
  };

  const notices = (
    <>
      <h2 className="text-title font-semibold">{t("portals.layout.heading")}</h2>
      {change ? <ChangeNotice change={change} project={ORG_NAMESPACE} /> : null}
      {refusal.length > 0 ? (
        <Alert tone="danger" role="alert" title={t("portals.layout.refused")}>
          {Object.keys(server.fields).length > 0 ? <p>{t("portals.layout.refusedHint")}</p> : null}
          {Object.keys(server.fields).length > 0 ? fieldLinks : null}
          {server.rest.length > 0 ? <p>{server.rest.join("; ")}</p> : null}
        </Alert>
      ) : null}
    </>
  );

  if (pages.length === 0) {
    return (
      <div className="space-y-4">
        {notices}
        <EmptyState
          icon="globe"
          title={t("portals.layout.empty")}
          description={t("portals.layout.emptyHint")}
          action={
            <Button variant="primary" onClick={() => setPages([starterHome()])}>
              {t("portals.layout.start")}
            </Button>
          }
        />
        {dirty ? <PublishBar dirty={dirty} pending={propose.isPending} onPublish={publish} onDiscard={discard} /> : null}
        <PagesHistory organization={name} kind="Portal" touches={touchesLayout} heading={t("portals.layout.history")} empty={t("portals.layout.historyEmpty")} />
      </div>
    );
  }

  return (
    <div className="space-y-4">
      {notices}
      {tried && Object.keys(faults).length > 0 ? (
        <Alert tone="danger" role="alert" title={t("portals.layout.fix")}>
          {t("portals.layout.fixHint", { count: Object.keys(faults).length })}
          {fieldLinks}
        </Alert>
      ) : null}
      <PageBar
        pages={pages}
        current={current}
        lang={lang}
        languages={languages}
        onLang={setLang}
        onPick={setCurrent}
        onAdd={(added) => {
          setPages([...pages, added]);
          setCurrent(pages.length);
        }}
      />
      <Button size="sm" variant="secondary" className="xl:hidden" aria-expanded={showPreview} aria-controls="layout-preview" onClick={() => setShowPreview(!showPreview)}>
        {showPreview ? t("portals.layout.hidePreview") : t("portals.layout.toPreview")}
      </Button>
      <div className="grid gap-4 xl:grid-cols-[minmax(0,1fr)_minmax(0,1fr)]">
        <div className="min-w-0 space-y-4">
          {page ? (
            <PageForm
              page={page}
              at={current}
              lang={lang}
              choices={choices}
              faults={shownFaults}
              onChange={(next) => setPages(pages.map((p, i) => (i === current ? next : p)))}
              onPages={(fn) => setPages(fn(pages))}
              onRemove={() => {
                setPages(pages.filter((_, i) => i !== current));
                setCurrent(0);
              }}
            />
          ) : null}
        </div>
        <div className={showPreview ? "order-first min-w-0 xl:order-none xl:sticky xl:top-4 xl:self-start" : "hidden min-w-0 xl:sticky xl:top-4 xl:block xl:self-start"}>
          {page ? <LayoutPreview page={page} lang={lang} choices={choices} spec={spec} /> : null}
        </div>
      </div>
      <PublishBar dirty={dirty} pending={propose.isPending} onPublish={publish} onDiscard={discard} />
      <PagesHistory organization={name} kind="Portal" touches={touchesLayout} heading={t("portals.layout.history")} empty={t("portals.layout.historyEmpty")} />
    </div>
  );
}

function PublishBar({ dirty, pending, onPublish, onDiscard }: { dirty: boolean; pending: boolean; onPublish: () => void; onDiscard: () => void }): JSX.Element {
  const { t } = useTranslation();
  return (
    <Card padding="md" className={dirty ? "sticky bottom-2 z-10 flex flex-wrap items-center gap-3 shadow-md" : "flex flex-wrap items-center gap-3"}>
      <p className="min-w-0 flex-1 basis-60 text-body text-fg-muted">{dirty ? t("portals.layout.unpublished") : t("portals.layout.published")}</p>
      <div className="flex flex-wrap gap-2">
        <Button variant="secondary" disabled={!dirty || pending} disabledReason={t("portals.layout.why.nothing")} onClick={onDiscard}>
          {t("portals.layout.discard")}
        </Button>
        <Button variant="primary" disabled={!dirty} disabledReason={t("portals.layout.why.nothing")} loading={pending} onClick={onPublish}>
          {t("portals.layout.publish")}
        </Button>
      </div>
    </Card>
  );
}

function PageBar({
  pages,
  current,
  lang,
  languages,
  onLang,
  onPick,
  onAdd,
}: {
  pages: PortalPage[];
  current: number;
  lang: string;
  languages: string[];
  onLang: (lang: string) => void;
  onPick: (at: number) => void;
  onAdd: (page: PortalPage) => void;
}): JSX.Element {
  const { t } = useTranslation();
  const [adding, setAdding] = useState(false);
  const [slug, setSlug] = useState("");
  const [title, setTitle] = useState("");
  const taken = pages.some((p) => p.slug === slug);
  const slugError = slug === "" ? undefined : !DNS_LABEL.test(slug) || slug === HOME ? t("portals.layout.fault.slug") : taken ? t("portals.layout.fault.slugTaken") : undefined;
  const hasHome = pages.some((p) => p.slug === HOME);
  return (
    <Card padding="md" className="space-y-3">
        {languages.length > 1 ? (
          <Field id="layout-lang" label={t("portals.layout.language")} help={t("portals.layout.hint.language")} className="max-w-md">
            <Select id="layout-lang" value={lang} onChange={(event) => onLang(event.target.value)}>
              {languages.map((l) => (
                <option key={l} value={l}>
                  {l.toUpperCase()}
                </option>
              ))}
            </Select>
          </Field>
        ) : null}
      <div className="flex flex-wrap items-center gap-3">
        <nav aria-label={t("portals.layout.pages")} className="min-w-0 flex-1">
          <ul className="flex flex-wrap gap-2">
            {pages.map((p, i) => (
              <li key={`${p.slug}-${i}`}>
                <Button size="sm" variant={i === current ? "primary" : "secondary"} aria-pressed={i === current} onClick={() => onPick(i)}>
                  {p.slug === HOME ? t("portals.layout.home") : textOf(p.title, lang) || p.slug}
                </Button>
              </li>
            ))}
          </ul>
        </nav>
        <Button size="sm" variant="secondary" disabled={pages.length >= MAX_PAGES} disabledReason={t("portals.layout.why.pages", { max: MAX_PAGES })} onClick={() => setAdding(!adding)} aria-expanded={adding}>
          {t("portals.layout.addPage")}
        </Button>
        {!hasHome ? (
          <Button size="sm" variant="ghost" disabled={pages.length >= MAX_PAGES} disabledReason={t("portals.layout.why.pages", { max: MAX_PAGES })} onClick={() => onAdd(starterHome())}>
            {t("portals.layout.addHome")}
          </Button>
        ) : null}
      </div>
      {adding ? (
        <form
          className="grid gap-3 sm:grid-cols-[1fr_1fr_auto] sm:items-start"
          onSubmit={(event) => {
            event.preventDefault();
            if (slug === "" || slugError || title.trim() === "") return;
            onAdd({ slug, title: { [lang]: title.trim() }, rows: [{ blocks: [newBlock("text", lang)] }] });
            setSlug("");
            setTitle("");
            setAdding(false);
          }}
        >
          <Field id="layout-new-slug" label={t("portals.layout.slug")} help={t("portals.layout.hint.slug")} errors={slugError ? [slugError] : undefined} required>
            <Input id="layout-new-slug" value={slug} autoComplete="off" spellCheck={false} placeholder="doprava" onChange={(event) => setSlug(event.target.value.trim().toLowerCase())} />
          </Field>
          <Field id="layout-new-title" label={t("portals.layout.pageTitle")} help={t("portals.layout.hint.pageTitle")} required>
            <Input id="layout-new-title" value={title} placeholder={t("portals.layout.pageTitlePlaceholder")} onChange={(event) => setTitle(event.target.value)} />
          </Field>
          <Button type="submit" variant="primary" className="sm:mt-6" disabled={slug === "" || !!slugError || title.trim() === ""} disabledReason={t("portals.layout.why.newPage")}>
            {t("portals.layout.createPage")}
          </Button>
        </form>
      ) : null}
    </Card>
  );
}

const FIELD_LABELS: Record<string, string> = {
  markdown: "markdown",
  title: "blockTitle",
  intro: "intro",
  image: "image",
  tag: "tag",
  limit: "limit",
  items: "numbers",
  files: "files",
  datasets: "addDataset",
};

/**
 * A marked field as the banner names it ("Transport › Row 2 › Chart: Dataset in the chart"), its
 * page and the id of its control, from the server's path of the field.
 */
export function markedField(
  path: string,
  pages: PortalPage[],
  lang: string,
  t: (key: string, options?: Record<string, unknown>) => string,
): { page: number; name: string; target: string } | null {
  const named = /^spec\.pages\[(\d+)\]\.(?:rows\[(\d+)\]\.blocks\[(\d+)\]\.)?(.+)$/.exec(path);
  if (!named?.[1] || !named[4]) return null;
  const at = Number(named[1]);
  const page = pages[at];
  if (!page) return null;
  const pageName = page.slug === HOME ? t("portals.layout.home") : textOf(page.title, lang) || page.slug;
  const rest = named[4];
  if (named[2] === undefined || named[3] === undefined) {
    return { page: at, name: `${pageName}: ${t(rest === "slug" ? "portals.layout.slug" : "portals.layout.pageTitle")}`, target: rest === "title" ? "layout-page-title" : "" };
  }
  const row = Number(named[2]);
  const block = page.rows?.[row]?.blocks[Number(named[3])];
  if (!block) return null;
  const id = `spec.pages[${at}].rows[${row}].blocks[${named[3]}]`.replace(/[^a-z0-9]+/gi, "-");
  const card = /^cards\[(\d+)\]\.(title|url|text)$/.exec(rest);
  const label = card
    ? `card${card[2] === "url" ? "Url" : card[2] === "text" ? "Text" : "Title"}`
    : rest === "dataset"
      ? block.type === "map"
        ? "mapDataset"
        : "chartDataset"
      : (FIELD_LABELS[rest] ?? "blockTitle");
  return {
    page: at,
    name: `${pageName} › ${t("portals.layout.row", { n: row + 1 })} › ${t(`portals.layout.type.${block.type}`)}: ${t(`portals.layout.${label}`)}`,
    target: card ? `${id}-card${card[1]}-${card[2]}` : `${id}-${rest}`,
  };
}

/** A block type to put beside the blocks of one row, chosen in that row. */
function RowAdd({ id, onAdd }: { id: string; onAdd: (type: BlockType) => void }): JSX.Element {
  const { t } = useTranslation();
  const [type, setType] = useState<BlockType>("text");
  const [open, setOpen] = useState(false);
  if (!open) {
    return (
      <Button size="sm" variant="secondary" aria-expanded={false} onClick={() => setOpen(true)}>
        {t("portals.layout.addBlock")}
      </Button>
    );
  }
  return (
    <div className="flex flex-wrap items-end gap-2">
      <Field id={id} label={t("portals.layout.rowType")} help={t("portals.layout.hint.rowType")} className="min-w-0 flex-1">
        <Select id={id} value={type} onChange={(event) => setType(event.target.value as BlockType)}>
          {BLOCK_TYPES.map((option) => (
            <option key={option} value={option}>
              {t(`portals.layout.type.${option}`)}
            </option>
          ))}
        </Select>
      </Field>
      <Button
        size="sm"
        variant="secondary"
        onClick={() => {
          onAdd(type);
          setOpen(false);
        }}
      >
        {t("portals.layout.addHere")}
      </Button>
    </div>
  );
}

/**
 * The server's refusal lines split into those naming a field of `spec.pages` (shown on that
 * field, keyed by its path) and the rest (shown in the banner).
 */
export function refusedFields(lines: string[]): { fields: Record<string, string>; rest: string[] } {
  const fields: Record<string, string> = {};
  const rest: string[] = [];
  for (const line of lines) {
    const named = /^(spec\.pages\[\d+\][\w.[\]]*): (.+)$/.exec(line);
    if (named?.[1] && named[2]) fields[named[1].replace(/\.(markdown|title|intro|text)\.[a-z]{2,3}$/, ".$1")] = named[2].charAt(0).toUpperCase() + named[2].slice(1);
    else rest.push(line);
  }
  return { fields, rest };
}

function PageForm({
  page,
  at,
  lang,
  choices,
  faults,
  onChange,
  onPages,
  onRemove,
}: {
  page: PortalPage;
  at: number;
  lang: string;
  choices: Choices;
  faults: Record<string, string>;
  onChange: (page: PortalPage) => void;
  onPages: (fn: (pages: PortalPage[]) => PortalPage[]) => void;
  onRemove: () => void;
}): JSX.Element {
  const { t } = useTranslation();
  const [adding, setAdding] = useState<BlockType>("text");
  const rows = page.rows ?? [];
  const home = page.slug === HOME;
  const fault = (path: string) => (faults[path] ? [faults[path]] : undefined);
  return (
    <div className="space-y-4">
      <Card padding="md" className="space-y-3">
        <div className="flex flex-wrap items-start justify-between gap-3">
          <h3 className="text-title font-semibold">{home ? t("portals.layout.home") : `/page/${page.slug}`}</h3>
          <Button size="sm" variant="secondary" onClick={onRemove}>
            {home ? t("portals.layout.removeHome") : t("portals.layout.removePage")}
          </Button>
        </div>
        {home ? (
          <p className="text-body text-fg-muted">{t("portals.layout.homeHint")}</p>
        ) : (
          <>
            <Field id="layout-page-title" label={t("portals.layout.pageTitle")} help={t("portals.layout.hint.pageTitle")} errors={fault(`spec.pages[${at}].title`)} required>
              <Input id="layout-page-title" value={page.title?.[lang] ?? ""} placeholder={t("portals.layout.pageTitlePlaceholder")} onChange={(event) => onChange({ ...page, title: withText(page.title, lang, event.target.value) })} />
            </Field>
            <Checkbox label={t("portals.layout.menu")} hint={t("portals.layout.menuHint")} checked={page.menu !== false} onChange={(event) => onChange({ ...page, menu: event.target.checked })} />
          </>
        )}
      </Card>
      {rows.length === 0 ? <p className="text-body text-fg-muted">{t("portals.layout.noBlocks")}</p> : null}
      {rows.map((row, r) => (
        <section key={r} aria-label={t("portals.layout.row", { n: r + 1 })} className="space-y-3 border-t border-dashed border-border pt-3 sm:rounded-md sm:border sm:p-3">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <p className="text-body font-medium text-fg-muted">{t("portals.layout.row", { n: r + 1 })}</p>
            {row.blocks.length < MAX_ROW_BLOCKS ? <RowAdd id={`layout-row${r}-type`} onAdd={(type) => onPages((pages) => addToRow(pages, at, r, newBlock(type, lang)))} /> : null}
          </div>
          {row.blocks.map((block, b) => {
            const where: At = { page: at, row: r, block: b };
            return (
              <BlockCard
                key={`${r}-${b}-${block.type}`}
                block={block}
                path={`spec.pages[${at}].rows[${r}].blocks[${b}]`}
                faulted={Object.keys(faults).some((key) => key.startsWith(`spec.pages[${at}].rows[${r}].blocks[${b}].`))}
                lang={lang}
                choices={choices}
                fault={fault}
                canUp={r > 0 || row.blocks.length > 1}
                canDown={r < rows.length - 1 || row.blocks.length > 1}
                canLeft={b > 0}
                canRight={b < row.blocks.length - 1}
                canDuplicate={row.blocks.length < MAX_ROW_BLOCKS || rows.length < MAX_ROWS}
                onChange={(next) => onPages((pages) => setBlock(pages, where, next))}
                onMove={(dir) => onPages((pages) => moveBlock(pages, where, dir))}
                onDuplicate={() => onPages((pages) => duplicateBlock(pages, where))}
                onRemove={() => onPages((pages) => removeBlock(pages, where))}
              />
            );
          })}
        </section>
      ))}
      <Card padding="md" className="flex flex-wrap items-end gap-3">
        <Field id="layout-add-type" label={t("portals.layout.blockType")} help={t("portals.layout.blockTypeHint")} className="min-w-0 flex-1">
          <Select id="layout-add-type" value={adding} onChange={(event) => setAdding(event.target.value as BlockType)}>
            {BLOCK_TYPES.map((type) => (
              <option key={type} value={type}>
                {t(`portals.layout.type.${type}`)}
              </option>
            ))}
          </Select>
        </Field>
        <Button variant="secondary" disabled={rows.length >= MAX_ROWS} disabledReason={t("portals.layout.why.rows", { max: MAX_ROWS })} onClick={() => onPages((pages) => addRow(pages, at, newBlock(adding, lang)))}>
          {t("portals.layout.addRow")}
        </Button>
      </Card>
    </div>
  );
}

function BlockCard({
  block,
  path,
  lang,
  choices,
  fault,
  canUp,
  canDown,
  canLeft,
  canRight,
  canDuplicate,
  faulted,
  onChange,
  onMove,
  onDuplicate,
  onRemove,
}: {
  block: PortalBlock;
  path: string;
  lang: string;
  choices: Choices;
  fault: (path: string) => string[] | undefined;
  canUp: boolean;
  canDown: boolean;
  canLeft: boolean;
  canRight: boolean;
  canDuplicate: boolean;
  /** A field of the block is marked: its form stays open. */
  faulted: boolean;
  onChange: (block: PortalBlock) => void;
  onMove: (where: "up" | "down" | "left" | "right") => void;
  onDuplicate: () => void;
  onRemove: () => void;
}): JSX.Element {
  const { t } = useTranslation();
  const typeLabel = t(`portals.layout.type.${block.type}`);
  const id = path.replace(/[^a-z0-9]+/gi, "-");
  // A finished block folds to its heading line; a new or incomplete one opens with its form.
  const [editing, setEditing] = useState(() => Object.keys(faultsOfPages([{ slug: HOME, rows: [{ blocks: [block] }] }])).length > 0);
  const open = editing || faulted;
  const dataset = block.type === "map" || block.type === "chart" ? (choices.datasets.find((d) => d.name === block.dataset)?.title ?? block.dataset) : "";
  const summary = block.type === "text" ? (textOf(block.markdown, lang).split("\n").find((line) => line.trim() !== "") ?? "").replace(/^#+\s*/, "") : textOf(block.title, lang) || dataset;
  const edge = t("portals.layout.cannotMove");
  const titleField =
    block.type === "text" ? null : (
      <Field id={`${id}-title`} label={t("portals.layout.blockTitle")} help={t("portals.layout.hint.blockTitle")} errors={fault(`${path}.title`)}>
        <Input id={`${id}-title`} value={block.title?.[lang] ?? ""} onChange={(event) => onChange({ ...block, title: withText(block.title, lang, event.target.value) } as PortalBlock)} />
      </Field>
    );
  const datasetSelect = (value: string, onPick: (name: string) => void, field: string, label: string) => (
    <Field id={`${id}-${field}`} label={label} help={choices.datasets.length === 0 ? t("portals.layout.noDatasets") : t(block.type === "chart" ? "portals.layout.hint.chartDataset" : "portals.layout.hint.dataset")} errors={fault(`${path}.${field}`)} required>
      <Select id={`${id}-${field}`} value={value} onChange={(event) => onPick(event.target.value)}>
        <option value="">{t("portals.layout.pickDataset")}</option>
        {choices.datasets.map((d) => (
          <option key={d.name} value={d.name}>
            {d.title}
          </option>
        ))}
      </Select>
    </Field>
  );
  return (
    <Card padding="md" className="space-y-3">
      <div className="flex items-center justify-between gap-2">
        <div className="min-w-0 flex-1">
          <h4 className="text-body font-semibold">{typeLabel}</h4>
          {!open && summary ? <p className="truncate text-body text-fg-muted">{summary}</p> : null}
        </div>
        {faulted ? null : (
          <Button size="sm" variant="secondary" aria-expanded={open} aria-controls={`${id}-form`} onClick={() => setEditing(!editing)}>
            {open ? t("portals.layout.fold", { type: typeLabel }) : t("portals.layout.edit", { type: typeLabel })}
          </Button>
        )}
        <RowActions
          label={typeLabel}
          actions={[
            { key: "up", label: t("portals.layout.moveUp"), onSelect: () => onMove("up"), disabledReason: canUp ? undefined : edge },
            { key: "down", label: t("portals.layout.moveDown"), onSelect: () => onMove("down"), disabledReason: canDown ? undefined : edge },
            { key: "left", label: t("portals.layout.moveLeft"), onSelect: () => onMove("left"), disabledReason: canLeft ? undefined : edge },
            { key: "right", label: t("portals.layout.moveRight"), onSelect: () => onMove("right"), disabledReason: canRight ? undefined : edge, separatorAfter: true },
            { key: "duplicate", label: t("portals.layout.duplicate"), onSelect: onDuplicate, disabledReason: canDuplicate ? undefined : t("portals.layout.full") },
            { key: "remove", label: t("portals.layout.remove"), onSelect: onRemove, tone: "danger" },
          ]}
        />
      </div>
      {open ? (
        <div id={`${id}-form`} className="space-y-3">
      {titleField}
      {block.type === "hero" ? (
        <>
          <MarkdownField id={`${id}-intro`} label={t("portals.layout.intro")} help={t("portals.layout.hint.intro")} value={block.intro?.[lang] ?? ""} errors={fault(`${path}.intro`)} onChange={(value) => onChange({ ...block, intro: withText(block.intro, lang, value) })} />
          <Field id={`${id}-image`} label={t("portals.layout.image")} help={choices.files.some((f) => isRaster(f)) ? t("portals.layout.hint.image") : t("portals.layout.noImages")}>
            <Select id={`${id}-image`} value={block.image ?? ""} onChange={(event) => onChange({ ...block, image: event.target.value || undefined })}>
              <option value="">{t("portals.layout.noImage")}</option>
              {choices.files.filter(isRaster).map((f) => (
                <option key={f.id} value={f.path}>
                  {f.name}
                </option>
              ))}
            </Select>
          </Field>
          <Checkbox label={t("portals.layout.search")} hint={t("portals.layout.hint.search")} checked={block.search !== false} onChange={(event) => onChange({ ...block, search: event.target.checked })} />
        </>
      ) : block.type === "text" ? (
        <MarkdownField id={`${id}-markdown`} label={t("portals.layout.markdown")} help={t("portals.layout.hint.markdown")} value={block.markdown[lang] ?? ""} errors={fault(`${path}.markdown`)} onChange={(value) => onChange({ ...block, markdown: withText(block.markdown, lang, value) ?? {} })} />
      ) : block.type === "dataset-gallery" ? (
        <div className="grid gap-3 sm:grid-cols-2">
          <Field id={`${id}-tag`} label={t("portals.layout.tag")} help={t("portals.layout.hint.tag")}>
            <Input id={`${id}-tag`} value={block.tag ?? ""} placeholder="doprava" onChange={(event) => onChange({ ...block, tag: event.target.value.trim() || undefined })} />
          </Field>
          <Field id={`${id}-theme`} label={t("portals.layout.theme")} help={t("portals.layout.hint.theme")}>
            <Select id={`${id}-theme`} value={block.theme ?? ""} onChange={(event) => onChange({ ...block, theme: event.target.value || undefined })}>
              <option value="">{t("portals.layout.any")}</option>
              {DATA_THEMES.map((code) => (
                <option key={code} value={code}>
                  {t(`choice.dataTheme.${code}`)}
                </option>
              ))}
            </Select>
          </Field>
          <Field id={`${id}-format`} label={t("portals.layout.format")} help={t("portals.layout.hint.format")}>
            <Select id={`${id}-format`} value={block.format ?? ""} onChange={(event) => onChange({ ...block, format: event.target.value || undefined })}>
              <option value="">{t("portals.layout.any")}</option>
              {FORMATS.map((f) => (
                <option key={f} value={f}>
                  {f}
                </option>
              ))}
            </Select>
          </Field>
          <Field id={`${id}-limit`} label={t("portals.layout.limit")} help={t("portals.layout.hint.limit", { max: MAX_GALLERY })} errors={fault(`${path}.limit`)}>
            <Input id={`${id}-limit`} type="number" min={1} max={MAX_GALLERY} value={block.limit ?? 6} onChange={(event) => onChange({ ...block, limit: Number(event.target.value) })} />
          </Field>
          <RadioGroup
            name={`${id}-layout`}
            legend={t("portals.layout.listLayout")}
            description={t("portals.layout.hint.listLayout")}
            value={block.layout ?? "cards"}
            onChange={(value) => onChange({ ...block, layout: value === "cards" ? undefined : value })}
            options={[
              { value: "cards", label: t("portals.layout.cards") },
              { value: "table", label: t("portals.layout.table") },
            ]}
          />
        </div>
      ) : block.type === "featured-datasets" ? (
        <>
          {datasetSelect("", (picked) => picked && !block.datasets.includes(picked) && block.datasets.length < MAX_ITEMS && onChange({ ...block, datasets: [...block.datasets, picked] }), "datasets", t("portals.layout.addDataset"))}
          <PickedList
            items={block.datasets.map((d) => ({ key: d, label: choices.datasets.find((c) => c.name === d)?.title ?? d }))}
            onRemove={(key) => onChange({ ...block, datasets: block.datasets.filter((d) => d !== key) })}
          />
        </>
      ) : block.type === "map" || block.type === "chart" ? (
        datasetSelect(block.dataset, (picked) => onChange({ ...block, dataset: picked }), "dataset", block.type === "map" ? t("portals.layout.mapDataset") : t("portals.layout.chartDataset"))
      ) : block.type === "numbers" ? (
        <fieldset className="flex flex-wrap gap-x-6 gap-y-2">
          <legend className="mb-2 text-body font-medium">{t("portals.layout.numbers")}</legend>
          <p className="basis-full text-body text-fg-muted">{t("portals.layout.hint.numbers")}</p>
          {PORTAL_NUMBERS.map((n) => (
            <Checkbox
              key={n}
              label={t(`portals.layout.number.${n}`)}
              checked={block.items.includes(n)}
              onChange={(event) => onChange({ ...block, items: event.target.checked ? PORTAL_NUMBERS.filter((x) => x === n || block.items.includes(x)) : block.items.filter((x) => x !== n) })}
            />
          ))}
          {fault(`${path}.items`) ? <InlineError>{fault(`${path}.items`)?.[0]}</InlineError> : null}
        </fieldset>
      ) : block.type === "link-cards" ? (
        <div className="space-y-3">
          {block.cards.map((card, c) => (
            <div key={c} className="grid gap-3 rounded-md bg-surface-subtle p-3 sm:grid-cols-2">
              <Field id={`${id}-card${c}-title`} label={t("portals.layout.cardTitle")} help={t("portals.layout.hint.cardTitle")} errors={fault(`${path}.cards[${c}].title`)} required>
                <Input id={`${id}-card${c}-title`} value={card.title[lang] ?? ""} placeholder={t("portals.layout.cardTitlePlaceholder")} onChange={(event) => onChange({ ...block, cards: block.cards.map((x, i) => (i === c ? { ...x, title: withText(x.title, lang, event.target.value) ?? {} } : x)) })} />
              </Field>
              <Field id={`${id}-card${c}-url`} label={t("portals.layout.cardUrl")} help={t("portals.layout.hint.cardUrl")} errors={fault(`${path}.cards[${c}].url`)} required>
                <Input id={`${id}-card${c}-url`} value={card.url} spellCheck={false} placeholder="/page/rozpocet" onChange={(event) => onChange({ ...block, cards: block.cards.map((x, i) => (i === c ? { ...x, url: event.target.value } : x)) })} />
              </Field>
              <Field id={`${id}-card${c}-text`} label={t("portals.layout.cardText")} help={t("portals.layout.hint.cardText")} errors={fault(`${path}.cards[${c}].text`)} className="sm:col-span-2">
                <Input id={`${id}-card${c}-text`} value={card.text?.[lang] ?? ""} placeholder={t("portals.layout.cardTextPlaceholder")} onChange={(event) => onChange({ ...block, cards: block.cards.map((x, i) => (i === c ? { ...x, text: withText(x.text, lang, event.target.value) } : x)) })} />
              </Field>
              <div className="sm:col-span-2">
                <Button size="sm" variant="ghost" disabled={block.cards.length === 1} disabledReason={t("portals.layout.why.lastCard")} onClick={() => onChange({ ...block, cards: block.cards.filter((_, i) => i !== c) })}>
                  {t("portals.layout.removeCard")}
                </Button>
              </div>
            </div>
          ))}
          <Button size="sm" variant="secondary" disabled={block.cards.length >= MAX_ITEMS} disabledReason={t("portals.layout.why.cards", { max: MAX_ITEMS })} onClick={() => onChange({ ...block, cards: [...block.cards, { title: { [lang]: "" }, url: "" }] })}>
            {t("portals.layout.addCard")}
          </Button>
        </div>
      ) : block.type === "file-list" ? (
        <fieldset className="flex flex-col gap-2">
          <legend className="mb-2 text-body font-medium">{t("portals.layout.files")}</legend>
          <p className="text-body text-fg-muted">{t("portals.layout.hint.files")}</p>
          {choices.files.length === 0 ? <p className="text-body text-fg-muted">{t("portals.layout.noFiles")}</p> : null}
          {choices.files.map((f) => (
            <Checkbox
              key={f.id}
              label={f.name}
              checked={block.files.includes(f.path)}
              disabledReason={!block.files.includes(f.path) && block.files.length >= MAX_ITEMS ? t("portals.layout.full") : undefined}
              onChange={(event) => onChange({ ...block, files: event.target.checked ? [...block.files, f.path] : block.files.filter((x) => x !== f.path) })}
            />
          ))}
          {fault(`${path}.files`) ? <InlineError>{fault(`${path}.files`)?.[0]}</InlineError> : null}
        </fieldset>
      ) : null}
        </div>
      ) : null}
    </Card>
  );
}

function isRaster(f: FileRow): boolean {
  return ["image/png", "image/jpeg", "image/webp"].includes(f.contentType);
}

function PickedList({ items, onRemove }: { items: { key: string; label: string }[]; onRemove: (key: string) => void }): JSX.Element | null {
  const { t } = useTranslation();
  if (items.length === 0) return null;
  return (
    <ol className="space-y-1">
      {items.map((item) => (
        <li key={item.key} className="flex items-center justify-between gap-2 rounded-md bg-surface-subtle px-3 py-1">
          <span className="min-w-0 break-words text-body">{item.label}</span>
          <Button size="xs" variant="ghost" aria-label={t("portals.layout.removeNamed", { name: item.label })} onClick={() => onRemove(item.key)}>
            {t("portals.layout.remove")}
          </Button>
        </li>
      ))}
    </ol>
  );
}

function MarkdownField({ id, label, help, value, errors, onChange }: { id: string; label: string; help: string; value: string; errors?: string[]; onChange: (value: string) => void }): JSX.Element {
  return (
    <Field id={id} label={label} help={help} errors={errors}>
      <Textarea id={id} rows={3} value={value} onChange={(event) => onChange(event.target.value)} />
    </Field>
  );
}

/** One block drawn as the portal will show it, Markdown through the portal's own renderer. */
function PreviewBlock({ block, lang, choices }: { block: PortalBlock; lang: string; choices: Choices }): JSX.Element {
  const { t } = useTranslation();
  const markdown = block.type === "text" ? textOf(block.markdown, lang) : block.type === "hero" ? textOf(block.intro, lang) : "";
  const preview = usePreview(markdown);
  const title = block.type === "text" ? "" : textOf(block.title, lang);
  const datasetTitle = (name: string) => choices.datasets.find((d) => d.name === name)?.title ?? name;
  const html = preview.data?.html ? <div className="text-body [&_a]:underline">{previewElements(preview.data.html)}</div> : null;
  return (
    <div className={block.type === "hero" ? "rounded-md bg-primary p-4 text-primary-fg" : "rounded-md border border-border p-3"}>
      {title ? <p className="font-semibold">{title}</p> : null}
      {block.type === "hero" ? (
        <>
          {block.image ? (
            <p className="mb-2 rounded-md border border-primary-fg/40 px-2 py-6 text-center text-caption">
              {t("portals.layout.preview.image", { name: choices.files.find((f) => f.path === block.image)?.name ?? block.image })}
            </p>
          ) : null}
          {!title ? <p className="font-semibold">{t("portals.layout.preview.siteTitle")}</p> : null}
          {html}
          {block.search !== false ? (
            <p className="mt-2 rounded-md bg-surface px-2 py-1.5 text-caption text-fg-muted">{t("portals.layout.preview.search")}</p>
          ) : null}
        </>
      ) : block.type === "text" ? (
        (html ??
          (markdown.trim() !== "" ? (
            <p className="whitespace-pre-line text-body">{markdown}</p>
          ) : (
            <p className="text-caption text-fg-muted">{t("portals.layout.preview.emptyText")}</p>
          )))
      ) : block.type === "featured-datasets" ? (
        <ul className="list-disc pl-5 text-body">{block.datasets.map((d) => <li key={d}>{datasetTitle(d)}</li>)}</ul>
      ) : block.type === "map" || block.type === "chart" ? (
        <p className="text-caption text-fg-muted">{block.dataset ? t(`portals.layout.preview.${block.type}`, { name: datasetTitle(block.dataset) }) : t("portals.layout.pickDataset")}</p>
      ) : block.type === "numbers" ? (
        <p className="text-body">{block.items.map((n) => t(`portals.layout.number.${n}`)).join(" · ")}</p>
      ) : block.type === "link-cards" ? (
        <ul className="grid gap-2">
          {block.cards.map((c, i) => (
            <li key={i} className="rounded-md border border-border p-2">
              <p className="text-body font-semibold">{textOf(c.title, lang) || c.url}</p>
              {textOf(c.text, lang) ? <p className="text-caption text-fg-muted">{textOf(c.text, lang)}</p> : null}
            </li>
          ))}
        </ul>
      ) : block.type === "file-list" ? (
        <ul className="list-disc pl-5 text-body">{block.files.map((f) => <li key={f}>{choices.files.find((x) => x.path === f)?.name ?? f}</li>)}</ul>
      ) : block.type === "dataset-gallery" ? (
        <p className="text-caption text-fg-muted">{t("portals.layout.preview.gallery", { count: block.limit ?? 6 })}</p>
      ) : null}
    </div>
  );
}

/** The page drawn from the form at 1440 or 400 px: rows side by side, or stacked on a phone. */
function LayoutPreview({ page, lang, choices, spec }: { page: PortalPage; lang: string; choices: Choices; spec: PortalSpec }): JSX.Element {
  const { t } = useTranslation();
  const [width, setWidth] = useState<"1440" | "400">(() => (window.matchMedia?.("(max-width: 767px)").matches ? "400" : "1440"));
  const narrow = width === "400";
  const heading = page.slug === HOME ? textOf(spec.title as Texts | undefined, lang) || t("portals.layout.home") : textOf(page.title, lang) || page.slug;
  return (
    <Card padding="md" className="space-y-3" id="layout-preview">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h3 className="text-title font-semibold">{t("portals.layout.previewTitle")}</h3>
        <RadioGroup
          name="layout-preview-width"
          legend={t("portals.layout.previewWidth")}
          value={width}
          onChange={setWidth}
          options={[
            { value: "1440", label: t("opendataAppearance.previewWidth", { width: 1440 }) },
            { value: "400", label: t("opendataAppearance.previewWidth", { width: 400 }) },
          ]}
        />
      </div>
      <p className="text-caption text-fg-muted">{t("portals.layout.previewHint")}</p>
      <div className={narrow ? "mx-auto w-full max-w-phone space-y-3 rounded-md border border-border p-3" : "space-y-3 rounded-md border border-border p-3"} data-width={width}>
        <p className="text-caption font-semibold text-fg-muted">{heading}</p>
        {(page.rows ?? []).map((row, r) => (
          <div key={r} className={narrow ? "grid gap-3" : `grid gap-3 ${row.blocks.length === 3 ? "grid-cols-3" : row.blocks.length === 2 ? "grid-cols-2" : "grid-cols-1"}`}>
            {row.blocks.map((block, b) => (
              <PreviewBlock key={`${r}-${b}`} block={block} lang={lang} choices={choices} />
            ))}
          </div>
        ))}
      </div>
    </Card>
  );
}
