import { useEffect, useEffectEvent, useState } from "react";
import type { JSX, ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "@tanstack/react-router";
import { forPeople } from "../../api/client";
import { callOperation } from "../../api/operations";
import { usePermissions } from "../../api/permissions";
import { handPrefill, takePrefill } from "../../assistant/state";
import { ChangeNotice } from "../../components/ChangeNotice";
import { refName } from "../../api/manifest";
import type { Change } from "../../api/manifest";
import type { Verdict } from "../../api/drafts";
import { parse as parseYaml } from "yaml";
import { Alert, Badge, Button, Card, Field, FileDropZone, Icon, Input, PageHeader } from "../../components/ui";
import type { IconName } from "../../components/ui";
import { ResourceNamePicker } from "../../components/pickers/ResourceNamePicker";
import { MAX_FILES_BYTES, SAMPLE_BYTES, sampleOf, spaceNameOf, tableNameOf, typeNameOf } from "./csvUpload";
import { DraftedModel } from "./DraftedModel";
import { csvOf, MAX_ROWS, parseCsv, readSheets, sheetFileName, unreadableText } from "./importRows";
import type { Sheet, Table } from "./importRows";
import { LoadRows } from "./LoadRows";
import { ModelReview } from "./ModelReview";
import { SheetChoice } from "./SheetChoice";

interface CompletedDraft {
  kind: string;
  name: string;
  inferred: boolean;
  manifest: Record<string, unknown>;
  verdict?: Verdict | null;
}

interface CompleteResult {
  space: string;
  found: string[];
  drafts: CompletedDraft[];
  proposeReady: boolean;
  lane: string;
  change?: Change | null;
  /** The space already serves this model: nothing to propose, the rows load straight away. */
  alreadyLive?: boolean;
}

/** The page a draft of each kind opens on; the dashboards page reads `?draft=` as a Layer, then a Dashboard (JC1-40). */
const DRAFT_PAGE: Record<string, string> = {
  DataSource: "datasources",
  Pipeline: "pipelines",
  Endpoint: "endpoints",
  Dashboard: "dashboards",
  Layer: "dashboards",
  ContextSpace: "spaces",
  DataModel: "models",
};

const KIND_ICON: Record<string, IconName> = {
  DataModel: "models",
  ContextSpace: "spaces",
  DataSource: "datasources",
  Endpoint: "endpoints",
  Pipeline: "pipelines",
  Layer: "dashboards",
  Dashboard: "dashboards",
};
/** The icon of a kind, by own property only: a kind named `constructor` is not an icon. */
function iconOf(kind: string, fallback: IconName): IconName {
  return Object.hasOwn(KIND_ICON, kind) ? KIND_ICON[kind] : fallback;
}


type Translate = (key: string, values?: Record<string, unknown>) => string;

/** What one draft is, in one line a person reads without knowing the kind: its type, feed, schedule or audience. */
function summaryOf(draft: CompletedDraft, t: Translate): string | null {
  const spec = (draft.manifest.spec ?? {}) as Record<string, unknown>;
  const text = (value: unknown): string | undefined => (typeof value === "string" && value !== "" ? value : undefined);
  switch (draft.kind) {
    case "DataModel": {
      const type = Array.isArray(spec.classes) ? text(spec.classes[0]) : undefined;
      if (type === undefined) return null;
      let attributes = 0;
      try {
        const source = parseYaml(text(spec.source) ?? "") as {
          classes?: Record<string, { attributes?: Record<string, unknown>; slots?: unknown[] }>;
        } | null;
        // Model Tools lists a class's slots; a hand-written model may declare attributes inline.
        const declared = source?.classes?.[type];
        attributes = (declared?.slots?.length ?? 0) + Object.keys(declared?.attributes ?? {}).length;
      } catch {
        attributes = 0;
      }
      return t("spaces.complete.summary.DataModel", { type, count: attributes });
    }
    case "ContextSpace": {
      const model = refName(spec.dataModelRef) || undefined;
      return model === undefined ? null : t("spaces.complete.summary.ContextSpace", { model });
    }
    case "DataSource": {
      const url = text((spec.http as { url?: unknown } | undefined)?.url);
      if (url === undefined) return null;
      try {
        return t("spaces.complete.summary.DataSource", { host: new URL(url).host });
      } catch {
        return null;
      }
    }
    case "Pipeline": {
      const type = text((spec.output as { type?: unknown } | undefined)?.type);
      const period = text(spec.period);
      return type === undefined || period === undefined ? null : t("spaces.complete.summary.Pipeline", { type, period });
    }
    case "Endpoint": {
      const audience = text(spec.audience);
      return audience === undefined ? null : t(`spaces.complete.summary.audience.${audience}`);
    }
    case "Layer": {
      const type = text(spec.entityType);
      const colour = text((spec.colorBy as { property?: unknown } | undefined)?.property);
      if (type === undefined) return null;
      return colour === undefined
        ? t("spaces.complete.summary.Layer", { type })
        : t("spaces.complete.summary.LayerColoured", { type, property: colour });
    }
    case "Dashboard": {
      const pages = Array.isArray(spec.pages) ? spec.pages.length : 0;
      return t("spaces.complete.summary.Dashboard", { count: pages });
    }
    default:
      return null;
  }
}

/** The one table these files are, with the name it travels under; none when they are anything else. */
function rowsOf(files: { name: string; content: string }[], books: { name: string; sheets: Sheet[]; sheet: string }[]): { name: string; title: string; table: Table } | undefined {
  if (files.length + books.length !== 1) return undefined;
  const book = books[0];
  const sheet = book?.sheets.find((s) => s.name === book.sheet);
  if (book && sheet) return { name: sheetFileName(book.name, book.sheets, sheet), title: tableNameOf(book.name, sheet.name), table: sheet.table };
  const file = files[0];
  return file?.name.toLowerCase().endsWith(".csv") ? { name: file.name, title: tableNameOf(file.name), table: parseCsv(file.content) } : undefined;
}

/** The model the op drafted: the type its rows take and the LinkML that checks them. */
function draftedModel(drafts: CompletedDraft[]): { type: string; source: string | undefined } | undefined {
  const spec = drafts.find((d) => d.kind === "DataModel")?.manifest.spec as { classes?: unknown; source?: unknown } | undefined;
  const type = Array.isArray(spec?.classes) ? spec.classes[0] : undefined;
  return typeof type === "string" ? { type, source: typeof spec?.source === "string" ? spec.source : undefined } : undefined;
}

/** How much the chosen table holds; a sheet's option says it already when there are several. */
function RowsRead({ rows, name, oneSheet }: { rows: { table: Table }; name: string; oneSheet: boolean }): JSX.Element {
  const { t } = useTranslation();
  return (
    <>
      {oneSheet ? <p className="text-caption text-fg-muted">{t("spaces.import.read", { name, rows: rows.table.rows.length, columns: rows.table.columns.length })}</p> : null}
      {rows.table.total !== undefined ? <Alert tone="warning">{t("spaces.import.cut", { total: rows.table.total, max: MAX_ROWS })}</Alert> : null}
    </>
  );
}

/** What follows a proposal: the rows of the same file, without choosing it again, or the notice alone. */
function ProposedNext({ project, result, change, rows }: { project: string; result: CompleteResult; change: Change; rows?: { table: Table } }): JSX.Element {
  const model = draftedModel(result.drafts);
  return rows && model ? (
    <LoadRows project={project} space={result.space} type={model.type} source={model.source} table={rows.table} change={change} />
  ) : (
    <ChangeNotice change={change} project={project} />
  );
}

/** The drafts the assistant's `space_complete` handed over with its navigation, when it did. */
function handedOver(prefill: Record<string, unknown> | null): { result: CompleteResult | null; url: string } {
  const result = prefill?.result as CompleteResult | undefined;
  return {
    result: result && Array.isArray(result.drafts) && typeof result.space === "string" ? result : null,
    url: typeof prefill?.url === "string" ? prefill.url : "",
  };
}

/** The drafts, folded once they are a Change: what comes next is then the page's subject. */
function Drafts({ folded, count, children }: { folded: boolean; count: number; children: ReactNode }): JSX.Element {
  const { t } = useTranslation();
  const list = <div className="flex flex-col gap-3">{children}</div>;
  if (!folded) return list;
  return (
    <details>
      <summary className="cursor-pointer text-body text-fg">{t("spaces.complete.inChange", { count })}</summary>
      <div className="mt-2">{list}</div>
    </details>
  );
}

/** One draft: what it is, whether its check passed, and the way to its own page (AG-73). */
function DraftCard({ project, draft: d }: { project: string; draft: CompletedDraft }): JSX.Element {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const open = () => {
    const base = `/projects/${project}`;
    const plural = Object.hasOwn(DRAFT_PAGE, d.kind) ? DRAFT_PAGE[d.kind] : undefined;
    if (plural === undefined) {
      void navigate({ href: `${base}/spaces` });
    } else if (d.kind === "DataModel") {
      const source = (d.manifest.spec as { source?: unknown } | undefined)?.source;
      if (typeof source === "string") handPrefill(`${base}/models`, { source });
      void navigate({ href: `${base}/models` });
    } else {
      void navigate({ href: `${base}/${plural}?draft=${encodeURIComponent(d.name)}` });
    }
  };
  const summary = summaryOf(d, t);
  return (
    <Card data-testid={`complete-draft-${d.kind}`}>
      <div className="flex items-center justify-between gap-2 p-3">
        <div className="flex min-w-0 flex-col gap-1">
          <div className="flex flex-wrap items-center gap-2">
            <Icon name={iconOf(d.kind, "spaces")} className="size-4 text-fg-muted" />
            <span className="font-semibold text-fg">{KIND_ICON[d.kind] ? t(`spaces.complete.kind.${d.kind}`) : d.kind}</span>
            <span className="font-mono text-caption text-fg-muted">{d.name}</span>
            {d.inferred ? null : <Badge tone="neutral">{t("spaces.complete.alreadyThere")}</Badge>}
            {d.verdict ? <Badge tone={d.verdict.ok ? "success" : "danger"}>{d.verdict.ok ? t("spaces.complete.checkPassed") : t("spaces.complete.checkFailed")}</Badge> : null}
          </div>
          {summary !== null ? <p className="text-caption text-fg-muted">{summary}</p> : null}
        </div>
        <Button size="sm" variant="ghost" onClick={open}>
          {t("spaces.complete.open")}
        </Button>
      </div>
      {d.verdict && !d.verdict.ok && d.verdict.findings.length > 0 ? (
        <ul className="border-t border-border px-3 py-2 text-caption text-danger">
          {d.verdict.findings.map((f, i) => (
            <li key={i}>
              {f.path ? `${f.path}: ` : ""}
              {forPeople(f.message)}
            </li>
          ))}
        </ul>
      ) : null}
    </Card>
  );
}

export function SpaceComplete({ project }: { project: string }): JSX.Element {
  const { t } = useTranslation();
  return (
    <div className="flex flex-col gap-section">
      <PageHeader title={t("spaces.complete.title")} description={t("spaces.complete.lead")} />
      <CompleteFlow project={project} />
    </div>
  );
}

/**
 * One table or feed into a Context Space: the model drafted and reviewed, the bundle proposed, the
 * approval followed and the rows loaded. The page shows it and the assistant dock opens it on an
 * attached table (T-6456), so both doors are this one flow.
 */
export function CompleteFlow({ project, file, inDock = false }: {
  project: string;
  /** The table the dock was given; a File lives only in this tab's memory. */
  file?: File;
  /** The file's way in only: a dock has no room for the feed address. */
  inDock?: boolean;
}): JSX.Element {
  const { t } = useTranslation();
  const permissions = usePermissions(project);
  const [spaceName, setSpaceName] = useState(() => {
    if (file !== undefined) return spaceNameOf(file.name);
    if (typeof window === "undefined") return "";
    return new URLSearchParams(window.location.search).get("space") ?? "";
  });
  const [url, setUrl] = useState("");
  const [files, setFiles] = useState<{ name: string; content: string }[]>([]);
  // An Excel workbook is read here, every sheet, and goes to the op as the CSV of the sheet the
  // person chose: the op takes text, and a workbook read as text is noise (JC1-07).
  const [books, setBooks] = useState<{ name: string; sheets: Sheet[]; sheet: string }[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<CompleteResult | null>(null);
  // The drafted model as the person corrected it; none until they change a type.
  const [corrected, setCorrected] = useState<string | null>(null);
  const takeFiles = async (selected: File[]) => {
    setError(null);
    setResult(null);
    setCorrected(null);
    const workbook = (f: File) => f.name.toLowerCase().endsWith(".xlsx");
    if (selected.reduce((sum, f) => sum + f.size, 0) > MAX_FILES_BYTES) {
      setBooks([]);
      setFiles([]);
      setError(t("spaces.complete.tooLarge"));
      return;
    }
    try {
      const read = await Promise.all(
        selected.filter(workbook).map(async (f) => {
          const sheets = await readSheets(f);
          return { name: f.name, sheets, sheet: sheets[0]?.name ?? "" };
        }),
      );
      setBooks(read);
      // A sheet says what its rows are better than a file name does: the space is named after it
      // while the person has not named it themselves (T-6567).
      const named = read.length === 1 && selected.length === 1 ? spaceNameOf(tableNameOf(read[0].name, read[0].sheet)) : "";
      if (named) setSpaceName((now) => (now.trim() === "" || now === spaceNameOf(read[0].name) ? named : now));
      setFiles(await Promise.all(selected.filter((f) => !workbook(f)).map(async (f) => ({ name: f.name, content: await f.text() }))));
    } catch (err: unknown) {
      setBooks([]);
      setFiles([]);
      setError(unreadableText(t, err));
    }
  };

  const takeHanded = useEffectEvent((file: File) => void takeFiles([file]));
  // The agent already completed the space (AG-73): its drafts open here ready to propose. Taken
  // once the page is on screen: a render React throws away and starts over (the dock's stream
  // updates while the navigation is pending) would otherwise take them and show nothing (T-0894).
  useEffect(() => {
    if (file !== undefined) {
      // eslint-disable-next-line react-hooks/set-state-in-effect -- a handed file is taken once after mount
      takeHanded(file);
      return;
    }
    if (inDock) {
      return;
    }
    const handed = handedOver(takePrefill(window.location.pathname));
    if (handed.result === null && handed.url === "") {
      return;
    }
    // Taking the prefill empties it, so it is taken once after mount; lazy state lost it to a discarded render (T-0894).
    setUrl(handed.url);
    setResult(handed.result);
  }, [file, inDock]);

  /** What the op reads: the text files as they are, each workbook as its chosen sheet's CSV. */
  const given = () => [
    ...files,
    ...books.flatMap((book) => {
      const sheet = book.sheets.find((s) => s.name === book.sheet);
      return sheet ? [{ name: sheetFileName(book.name, book.sheets, sheet), content: sampleOf(csvOf(sheet.table), SAMPLE_BYTES) }] : [];
    }),
    // The op takes a model file as it takes one dropped here, and checks it the same way.
    ...(corrected !== null && result ? [{ name: `${result.space}.linkml.yaml`, content: corrected }] : []),
  ];
  const chosen = [...files.map((f) => f.name), ...books.map((b) => b.name)];
  // One table and nothing that describes a source: its rows have no feed to come from, so the
  // person loads them here (T-6445) and the op drafts the Endpoint and the grant for that (DM-81).
  const rows = url.trim() ? undefined : rowsOf(files, books);
  const model = result ? draftedModel(result.drafts) : undefined;

  const executeComplete = async (propose: boolean) => {
    setLoading(true);
    setError(null);
    try {
      const payload: Record<string, unknown> = {
        propose,
      };
      if (spaceName.trim()) {
        payload.space = spaceName.trim();
      }
      if (url.trim()) {
        payload.url = url.trim();
      } else if (chosen.length > 0) {
        payload.files = given();
      }
      if (rows) {
        payload.upload = true;
        payload.typeName = typeNameOf(rows.title);
      }

      const answer = await callOperation(project, "jc_space_complete", payload);
      if (!answer.ok) {
        setError(answer.reason ?? `HTTP ${answer.status}`);
        return;
      }

      const data = answer.output as CompleteResult;
      setResult(data);
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  };

  // A role that may not propose a space reads why, not a form whose every step ends in a 403 (T-4263).
  if (permissions.data !== undefined && !permissions.can("ContextSpace", "propose")) {
    return (
      <Alert role="status" tone="info">
        {t("form.mayNotCreate")}
      </Alert>
    );
  }

  return (
    <div className="flex flex-col gap-section">
      {inDock && result?.change ? null : <Card className="flex flex-col gap-4">
        {inDock && file !== undefined ? <p className="truncate text-body font-medium text-fg">{t("spaces.complete.fileGiven", { name: file.name })}</p> : null}
        <Field id="complete-space" label={t("spaces.complete.space")} help={t("spaces.complete.spaceHint")}>
          {/* An existing space to complete, or the name of a new one (ADR-N-033). */}
          <ResourceNamePicker
            id="complete-space"
            label={t("spaces.complete.space")}
            labelled
            from={{ project, plural: "spaces" }}
            value={spaceName}
            onChange={setSpaceName}
            create
          />
        </Field>

        {inDock ? null : (
          <Field id="complete-url" label={t("spaces.complete.url")} help={t("spaces.complete.urlHint")}>
            <Input
              id="complete-url"
              value={url}
              onChange={(e) => setUrl(e.target.value)}
              placeholder={t("spaces.complete.urlPlaceholder")}
            />
          </Field>
        )}

        {inDock ? null : (
          <FileDropZone
            label={t("spaces.complete.files")}
            button={t("form.dropMany")}
            hint={t("spaces.complete.filesHint")}
            multiple
            onFiles={(selected) => void takeFiles(selected)}
          >
            {chosen.length > 0 ? (
              <p className="text-caption text-fg">
                {t("form.chosenFiles", { names: chosen.join(", ") })}
              </p>
            ) : null}
          </FileDropZone>
        )}
        {books.map((book, at) => (
          <SheetChoice
            key={book.name}
            id={`complete-sheet-${at}`}
            sheets={book.sheets}
            value={book.sheet}
            onChange={(sheet) => setBooks((all) => all.map((b, i) => (i === at ? { ...b, sheet } : b)))}
          />
        ))}
        {rows ? <RowsRead rows={rows} name={chosen[0]} oneSheet={(books[0]?.sheets.length ?? 0) < 2} /> : null}
        {rows && model && result && !result.change && !result.alreadyLive ? (
          <div className={inDock ? "flex flex-col gap-4" : "grid gap-4 lg:grid-cols-2"}>
            <ModelReview project={project} space={result.space} type={model.type} source={corrected ?? model.source} table={rows.table} onCorrect={setCorrected} />
            <DraftedModel source={corrected ?? model.source} type={model.type} table={rows.table} />
          </div>
        ) : null}

        {/* The attributes above can outgrow the dock: the next step stays in view. */}
        <div className={`flex flex-wrap items-center gap-3 ${inDock ? "sticky bottom-0 border-t border-border bg-surface py-2" : ""}`}>
          <Button
            id="complete-btn"
            variant={result?.proposeReady ? "secondary" : "primary"}
            // UI-44: nothing to read from is a refusal with a reason on the button, not a grey
            // button and a guess; `loading` keeps the label and its width while the run is out.
            disabled={!url.trim() && chosen.length === 0}
            disabledReason={
              !url.trim() && chosen.length === 0 ? t("spaces.complete.needInput") : undefined
            }
            loading={loading}
            onClick={() => void executeComplete(false)}
          >
            {t(inDock ? "spaces.complete.checkFile" : "spaces.complete.action")}
          </Button>

          {result?.proposeReady && !result.change ? (
            <Button
              id="complete-propose"
              variant="primary"
              loading={loading}
              onClick={() => void executeComplete(true)}
            >
              {t("spaces.complete.proposeAll")}
            </Button>
          ) : null}
          {!url.trim() && chosen.length === 0 ? <span className="text-caption text-fg-muted" aria-hidden="true">{t("spaces.complete.needInput")}</span> : null}
        </div>
      </Card>}

      {error ? (
        <Alert role="alert" tone="danger">
          {error}
        </Alert>
      ) : null}

      {result?.change ? <ProposedNext project={project} result={result} change={result.change} rows={rows} /> : null}
      {result?.alreadyLive && rows && model ? <LoadRows project={project} space={result.space} type={model.type} source={model.source} table={rows.table} change={null} /> : null}

      {result?.drafts && result.drafts.length > 0 ? (
        <Drafts folded={Boolean(result.change) || result.alreadyLive === true} count={result.drafts.length}>
          {result.drafts.map((d) => (
            <DraftCard key={`${d.kind}-${d.name}`} project={project} draft={d} />
          ))}
          {result.drafts.some((d) => d.verdict?.ok === false) ? <p className="text-caption text-danger">{t("spaces.complete.failedNext")}</p> : null}
          {result.proposeReady && !result.change ? (
            <p className="text-caption text-fg-muted">{t(rows ? "spaces.complete.onApprovalRows" : "spaces.complete.onApproval", { count: rows?.table.rows.length })}</p>
          ) : null}
        </Drafts>
      ) : null}
    </div>
  );
}
