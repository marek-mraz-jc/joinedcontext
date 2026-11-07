import { useMemo, useState } from "react";
import type { ChangeEvent, JSX, ReactNode } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import type { UseQueryResult } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Alert, Badge, Button, Card, CardHeader, Field, InlineError, Input, Select, Skeleton } from "../../ui";
import { reasonProblem } from "../reason";
import { instantOf, MAX_CSV_ROWS, parseCsv, planImport, readHistory, runImport, valueOf, writeMeasurement } from "./historyApi";
import type { CsvProblem, CsvRow, Measurement, Version } from "./historyApi";

/** One page of measurements, as many as the broker serves per attribute (temporal.md). */
const PAGE = 9;

function formatValue(value: unknown): string {
  if (value === undefined || value === null) return "";
  return typeof value === "string" ? value : JSON.stringify(value);
}

/** An instant in the person's language, never the browser's (UI-15). */
function when(iso: string, language: string): string {
  return new Date(iso).toLocaleString(language, { dateStyle: "medium", timeStyle: "medium" });
}

/** The attribute every form writes to, and what to do once a write landed. */
interface Target {
  slug: string;
  entityId: string;
  attribute: string;
  /** The newest measurement: a typed field reads its text as that value's type. */
  sample: unknown;
  unitCode?: string;
  written: () => Promise<void>;
}

/** One write at a time: busy while it runs, the gateway's message when it fails. */
export function useWrite(target: Target) {
  const { t } = useTranslation();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const run = async (write: () => Promise<void>): Promise<boolean> => {
    setBusy(true);
    setError(null);
    try {
      await write();
      await target.written();
      return true;
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : t("app.error.generic"));
      return false;
    } finally {
      setBusy(false);
    }
  };
  return { busy, error, run };
}

function Panel({ id, title, actions, children }: { id: string; title: string; actions?: ReactNode; children: ReactNode }): JSX.Element {
  return (
    <Card as="section" padding="md" aria-labelledby={id}>
      <CardHeader as="h3" titleId={id} title={title} actions={actions} />
      {children}
    </Card>
  );
}

/** A form folded to its title: the timeline leads, and a phone scrolls past nothing it did not open. */
function FoldedPanel({ id, title, children }: { id: string; title: string; children: ReactNode }): JSX.Element {
  return (
    <Card as="section" padding="md" aria-labelledby={id}>
      <details name="history-forms">
        <summary className="cursor-pointer font-semibold text-fg">
          <span id={id}>{title}</span>
        </summary>
        {children}
      </details>
    </Card>
  );
}

function ReasonField({ id, value, onChange }: { id: string; value: string; onChange: (next: string) => void }): JSX.Element {
  const { t } = useTranslation();
  return (
    <Field id={id} label={t("attributeHistory.reason")} help={t("attributeHistory.reasonHint")}>
      <Input id={id} value={value} onChange={(e) => onChange(e.target.value)} />
    </Field>
  );
}

function SaveButton({ write, ready, label, hint, onClick }: { write: { busy: boolean }; ready: boolean; label: string; hint?: string; onClick: () => void }): JSX.Element {
  const { t } = useTranslation();
  const why = hint ?? t("attributeHistory.needsReason");
  return (
    <>
      {ready ? null : <span className="self-center text-caption text-fg">{why}</span>}
      <Button variant="primary" size="sm" loading={write.busy} disabled={!ready} disabledReason={ready ? undefined : why} onClick={onClick}>
        {label}
      </Button>
    </>
  );
}

/** The current value, replaced through the Endpoint with a reason (DM-113). */
function EditCurrent({ target }: { target: Target }): JSX.Element {
  const { t } = useTranslation();
  // The field shows the newest value until the person types.
  const [typed, setTyped] = useState<string | undefined>(undefined);
  const [reason, setReason] = useState("");
  const write = useWrite(target);
  const text = typed ?? formatValue(target.sample);
  const value = valueOf(text, target.sample);
  const save = async () => {
    const observedAt = new Date().toISOString();
    const done = await write.run(() => writeMeasurement(target.slug, target.entityId, target.attribute, { value, observedAt, unitCode: target.unitCode }, reason));
    if (done) {
      setReason("");
      setTyped(undefined);
    }
  };
  return (
    <FoldedPanel id="history-edit-current-title" title={t("attributeHistory.editCurrent")}>
      <div className="mt-3 grid gap-3 sm:grid-cols-2">
        <Field id="history-edit-value" label={t("attributeHistory.value")} help={t("attributeHistory.valueHint")} errors={text !== "" && value === undefined ? [t("attributeHistory.valueInvalid")] : undefined}>
          <Input id="history-edit-value" value={text} onChange={(e) => setTyped(e.target.value)} />
        </Field>
        <ReasonField id="history-edit-reason" value={reason} onChange={setReason} />
      </div>
      {write.error ? <InlineError className="mt-2">{write.error}</InlineError> : null}
      <div className="mt-3 flex flex-wrap justify-end gap-2">
        <SaveButton write={write} ready={!reasonProblem(reason) && value !== undefined} label={t("attributeHistory.save")} onClick={() => void save()} />
      </div>
    </FoldedPanel>
  );
}

/** A value of an earlier instant: a row in history, the current value unchanged (DM-106). */
function AddOldValue({ target }: { target: Target }): JSX.Element {
  const { t } = useTranslation();
  const [local, setLocal] = useState("");
  const [text, setText] = useState("");
  const [reason, setReason] = useState("");
  const write = useWrite(target);
  const observedAt = instantOf(local);
  const value = valueOf(text, target.sample);
  const save = async () => {
    if (!observedAt) return;
    const done = await write.run(() => writeMeasurement(target.slug, target.entityId, target.attribute, { value, observedAt, unitCode: target.unitCode }, reason));
    if (done) {
      setLocal("");
      setText("");
      setReason("");
    }
  };
  return (
    <FoldedPanel id="history-add-old-title" title={t("attributeHistory.addOldValue")}>
      <div className="mt-3 grid gap-3 sm:grid-cols-3">
        <Field id="history-old-date" label={t("attributeHistory.observedAt")} help={t("attributeHistory.observedAtHint")} errors={local !== "" && !observedAt ? [t("attributeHistory.observedAtInvalid")] : undefined}>
          <Input id="history-old-date" type="datetime-local" value={local} onChange={(e) => setLocal(e.target.value)} />
        </Field>
        <Field id="history-old-val" label={t("attributeHistory.value")} help={t("attributeHistory.valueHint")} errors={text !== "" && value === undefined ? [t("attributeHistory.valueInvalid")] : undefined}>
          <Input id="history-old-val" value={text} onChange={(e) => setText(e.target.value)} />
        </Field>
        <ReasonField id="history-old-reason" value={reason} onChange={setReason} />
      </div>
      {write.error ? <InlineError className="mt-2">{write.error}</InlineError> : null}
      <div className="mt-3 flex flex-wrap justify-end gap-2">
        <SaveButton write={write} ready={!reasonProblem(reason) && Boolean(observedAt) && value !== undefined && text !== ""} label={t("attributeHistory.save")} onClick={() => void save()} />
      </div>
    </FoldedPanel>
  );
}

export function CsvNotes({ problems, unchecked }: { problems: CsvProblem[]; unchecked: number }): JSX.Element {
  const { t } = useTranslation();
  return (
    <>
      {unchecked > 0 ? <p className="mt-1 text-caption text-fg-muted">{t("attributeHistory.csvUnchecked", { count: unchecked })}</p> : null}
      {problems.length > 0 ? (
        <Alert tone="warning" className="mt-2">
          <ul className="list-disc pl-5">
            {problems.map((problem) => (
              <li key={`${problem.line}-${problem.reason}`}>
                {t("attributeHistory.csvProblems", { line: problem.line, reason: t(`attributeHistory.csvProblem.${problem.reason}`, { max: MAX_CSV_ROWS }) })}
              </li>
            ))}
          </ul>
          <p>{t("attributeHistory.csvLeftOut")}</p>
        </Alert>
      ) : null}
    </>
  );
}

/** Old values from a file: the counts first, nothing is sent until the person confirms (DM-81). */
function CsvImport({ target, measurements, complete }: { target: Target; measurements: Measurement[]; complete: boolean }): JSX.Element {
  const { t } = useTranslation();
  const [rows, setRows] = useState<CsvRow[]>([]);
  const [problems, setProblems] = useState<CsvProblem[]>([]);
  const [reason, setReason] = useState("");
  const [progress, setProgress] = useState<{ sent: number; total: number } | null>(null);
  const [result, setResult] = useState<string | null>(null);
  const write = useWrite(target);
  const plan = useMemo(() => planImport(rows, measurements, complete), [rows, measurements, complete]);
  const count = plan.added.length + plan.unchecked.length + plan.corrections.length;

  const choose = async (e: ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0];
    if (!file) return;
    setResult(null);
    const parsed = parseCsv(await file.text(), target.sample);
    setRows(parsed.rows);
    setProblems(parsed.problems);
  };
  const confirm = async () => {
    setResult(null);
    const done = await write.run(async () => {
      const ran = await runImport(target, plan, reason, setProgress);
      if (ran.failed) throw new Error(t("attributeHistory.importFailed", { line: ran.failed.line, reason: ran.failed.message }));
      setResult(t("attributeHistory.importResult", { sent: ran.sent }));
    });
    if (done) {
      setRows([]);
      setProblems([]);
      setReason("");
      setProgress(null);
    }
  };

  return (
    <FoldedPanel id="history-import-csv-title" title={t("attributeHistory.importCsv")}>
      <div className="mt-3 grid gap-3 sm:grid-cols-2">
        <Field id="history-import-file" label={t("attributeHistory.file")} help={t("attributeHistory.fileHint", { max: MAX_CSV_ROWS })}>
          <Input id="history-import-file" type="file" accept=".csv,text/csv" onChange={(e) => void choose(e)} />
        </Field>
        <ReasonField id="history-import-reason" value={reason} onChange={setReason} />
      </div>
      {rows.length > 0 ? (
        <Alert tone="info" role="status" className="mt-2">
          {t("attributeHistory.csvPreview", { added: plan.added.length + plan.unchecked.length, duplicates: plan.duplicates.length, corrections: plan.corrections.length })}
        </Alert>
      ) : null}
      <CsvNotes problems={problems} unchecked={plan.unchecked.length} />
      {progress ? <p className="mt-2 text-caption text-fg-muted">{t("attributeHistory.importProgress", progress)}</p> : null}
      {result ? <Alert tone="success" className="mt-2">{result}</Alert> : null}
      {write.error ? <InlineError className="mt-2">{write.error}</InlineError> : null}
      <div className="mt-3 flex flex-wrap justify-end gap-2">
        <SaveButton write={write} ready={!reasonProblem(reason) && count > 0} label={t("attributeHistory.confirmImport")} hint={t(count === 0 ? "attributeHistory.csvNothing" : "attributeHistory.csvNeedsReason")} onClick={() => void confirm()} />
      </div>
    </FoldedPanel>
  );
}

/**
 * The correction or revert of one measurement, inline: a dialog opened inside the row's `<dialog>`
 * would sit behind it. `revertTo` is the older version written again; absent, the person types.
 */
function VersionForm({ target, measurement, revertTo, close }: { target: Target; measurement: Measurement; revertTo?: Version; close: () => void }): JSX.Element {
  const { t, i18n } = useTranslation();
  const [text, setText] = useState(formatValue(measurement.current.value));
  const [reason, setReason] = useState("");
  const write = useWrite(target);
  const value = revertTo ? revertTo.value : valueOf(text, measurement.current.value);
  const save = async () => {
    const { observedAt, unitCode } = measurement.current;
    if (await write.run(() => writeMeasurement(target.slug, target.entityId, target.attribute, { value, observedAt, unitCode }, reason))) close();
  };
  const heading = `${t(revertTo ? "attributeHistory.revertTitle" : "attributeHistory.correctTitle")}: ${when(measurement.observedAt, i18n.language)}`;
  return (
    <section aria-labelledby="history-version-title" className="flex flex-col gap-3 border-l-2 border-primary pl-3">
      <h4 id="history-version-title" className="text-caption font-semibold text-fg">
        {heading}
      </h4>
      {revertTo ? (
        <p className="text-caption text-fg-muted">{t("attributeHistory.revertTo", { value: formatValue(revertTo.value) })}</p>
      ) : (
        <Field id="history-version-value" label={t("attributeHistory.value")} help={t("attributeHistory.valueHint")} errors={value === undefined ? [t("attributeHistory.valueInvalid")] : undefined}>
          <Input id="history-version-value" value={text} onChange={(e) => setText(e.target.value)} />
        </Field>
      )}
      <ReasonField id="history-version-reason" value={reason} onChange={setReason} />
      {write.error ? <InlineError>{write.error}</InlineError> : null}
      <div className="flex flex-wrap justify-end gap-2">
        <Button variant="secondary" size="sm" onClick={close}>
          {t("attributeHistory.cancel")}
        </Button>
        <SaveButton write={write} ready={!reasonProblem(reason) && value !== undefined} label={t(revertTo ? "attributeHistory.revert" : "attributeHistory.save")} onClick={() => void save()} />
      </div>
    </section>
  );
}

/** One version: when, what, and who wrote it why (ADR-N-054 2.3). A stack, as the row's dialog is narrow. */
function VersionRow({ measurement, version, first, open }: { measurement: Measurement; version: Version; first: boolean; open: (revertTo?: Version) => void }): JSX.Element {
  const { t, i18n } = useTranslation();
  const current = version === measurement.current;
  const { channel, operation, reason, writtenAt } = version.provenance ?? {};
  // Without the `user:` or `serviceAccount:` prefix the broker keeps it with.
  const actor = version.provenance?.actor?.replace(/^[^:]*:/, "");
  const who = [actor, [channel, operation].filter(Boolean).join(" "), writtenAt ? when(writtenAt, i18n.language) : ""].filter(Boolean).join(" · ");
  return (
    <li className={`flex flex-wrap items-start justify-between gap-2 py-2 ${first ? "border-t border-border" : "ml-2 border-l-2 border-border pl-3"}`}>
      <div className="min-w-0 flex-1 basis-48">
        {first ? <p className="text-caption font-medium text-fg">{when(measurement.observedAt, i18n.language)}</p> : null}
        <p className="flex flex-wrap items-center gap-1.5">
          <span className="font-mono [overflow-wrap:anywhere]">{formatValue(version.value)}</span>
          {version.unitCode ? <span className="text-caption text-fg-muted">{version.unitCode}</span> : null}
          {/* It tells versions apart: on a lone reading it would only repeat itself down the list. */}
          {measurement.versions.length > 1 ? <Badge tone={current ? "success" : "neutral"}>{t(current ? "attributeHistory.current" : "attributeHistory.superseded")}</Badge> : null}
        </p>
        {who ? <p className="text-caption text-fg-muted [overflow-wrap:anywhere]">{who}</p> : null}
        {reason ? <p className="text-caption text-fg [overflow-wrap:anywhere]">{`${t("attributeHistory.reason")}: ${reason}`}</p> : null}
      </div>
      <Button size="sm" variant="secondary" onClick={() => open(current ? undefined : version)}>
        {t(current ? "attributeHistory.correct" : "attributeHistory.revert")}
      </Button>
    </li>
  );
}

/** The measurements of one page, each with its versions oldest first. */
function Timeline({ measurements, open, form }: { measurements: Measurement[]; open: (measurement: Measurement, revertTo?: Version) => void; form?: { measurement: Measurement; node: ReactNode } }): JSX.Element {
  const { t } = useTranslation();
  return (
    <ul aria-label={t("attributeHistory.timeline")} className="flex flex-col">
      {measurements.flatMap((measurement) => [
        ...measurement.versions.map((version, index) => (
          <VersionRow key={`${measurement.observedAt}-${version.instanceId ?? index}`} measurement={measurement} version={version} first={index === 0} open={(revertTo) => open(measurement, revertTo)} />
        )),
        // Under the measurement it changes, in sight on a phone.
        form?.measurement.observedAt === measurement.observedAt ? <li key={`${measurement.observedAt}-form`} className="py-2">{form.node}</li> : null,
      ])}
    </ul>
  );
}

/** The timeline's panel: paging, the read's loading, empty and failed states, and what a write said. */
export function TimelinePanel({
  history,
  measurements,
  saved,
  older,
  newest,
  open,
  form,
}: {
  history: Pick<UseQueryResult<{ versions: boolean }>, "isPending" | "isError" | "isSuccess" | "error" | "data" | "refetch">;
  measurements: Measurement[];
  saved: boolean;
  older?: () => void;
  newest?: () => void;
  open: (measurement: Measurement, revertTo?: Version) => void;
  form?: { measurement: Measurement; node: ReactNode };
}): JSX.Element {
  const { t } = useTranslation();
  return (
    <Panel
      id="history-timeline-title"
      title={t("attributeHistory.timeline")}
      actions={
        <>
          {newest ? (
            <Button size="sm" variant="secondary" onClick={newest}>
              {t("attributeHistory.newest")}
            </Button>
          ) : null}
          {older ? (
            <Button size="sm" variant="secondary" onClick={older}>
              {t("attributeHistory.older")}
            </Button>
          ) : null}
        </>
      }
    >
      {saved ? <Alert tone="success" className="mb-2">{t("attributeHistory.saved")}</Alert> : null}
      {history.isPending ? (
        <div role="status" aria-busy="true" aria-label={t("app.loading")}>
          <Skeleton className="h-32 w-full" />
        </div>
      ) : null}
      {/* An unreadable history is not an empty one: it says so and offers a retry. */}
      {history.isError ? (
        <InlineError className="flex flex-wrap items-center gap-2">
          {history.error instanceof Error ? history.error.message : t("app.error.generic")}
          <Button size="sm" variant="ghost" onClick={() => void history.refetch()}>
            {t("app.error.retry")}
          </Button>
        </InlineError>
      ) : null}
      {history.isSuccess && measurements.length === 0 ? (
        <p role="status" className="text-caption text-fg-muted">
          {t("attributeHistory.noHistory")}
        </p>
      ) : null}
      {history.data && !history.data.versions ? <p className="mb-2 text-caption text-fg-muted">{t("attributeHistory.versionsRestricted")}</p> : null}
      {measurements.length > 0 ? <Timeline measurements={measurements} open={open} form={form} /> : null}
    </Panel>
  );
}

/** One attribute's pages of measurements: the read, the way to older pages, and its newest value. */
function useMeasurements(slug: string, entityId: string, attribute: string) {
  const [before, setBefore] = useState<string | undefined>(undefined);
  const history = useQuery({
    queryKey: ["attribute-history", slug, entityId, attribute, before],
    retry: false,
    queryFn: () => readHistory(slug, entityId, attribute, { before }),
  });
  const measurements = useMemo(() => history.data?.measurements ?? [], [history.data]);
  // The newest page holds the newest value; an older page never stands in for it.
  const [newest, setNewest] = useState<Version | undefined>(undefined);
  if (before === undefined && measurements[0] && measurements[0].current !== newest) setNewest(measurements[0].current);
  // A short page is the last one: nothing older to ask for.
  const oldest = measurements.length >= PAGE ? measurements.at(-1)?.observedAt : undefined;
  return {
    history,
    measurements,
    newest,
    // Fewer than a page on the newest page is the whole history: every CSV row compares.
    complete: before === undefined && measurements.length < PAGE,
    older: oldest ? () => setBefore(oldest) : undefined,
    toNewest: before ? () => setBefore(undefined) : undefined,
  };
}

/** One attribute's measurements and every form that writes them. */
function OneAttribute({ slug, entityId, attribute }: { slug: string; entityId: string; attribute: string }): JSX.Element {
  const queryClient = useQueryClient();
  const [editing, setEditing] = useState<{ measurement: Measurement; revertTo?: Version } | null>(null);
  const [saved, setSaved] = useState(false);
  const { history, measurements, newest, complete, older, toNewest } = useMeasurements(slug, entityId, attribute);
  const target: Target = {
    slug,
    entityId,
    attribute,
    sample: newest?.value,
    unitCode: newest?.unitCode,
    written: async () => {
      await queryClient.invalidateQueries({ queryKey: ["attribute-history", slug, entityId, attribute] });
      setSaved(true);
    },
  };

  const form = editing
    ? {
        measurement: editing.measurement,
        node: <VersionForm key={`${editing.measurement.observedAt}-${editing.revertTo?.instanceId ?? "correct"}`} target={target} measurement={editing.measurement} revertTo={editing.revertTo} close={() => setEditing(null)} />,
      }
    : undefined;

  return (
    <div className="flex flex-col gap-4 text-body">
      <TimelinePanel
        history={history}
        measurements={measurements}
        saved={saved}
        older={older}
        newest={toNewest}
        open={(measurement, revertTo) => setEditing({ measurement, revertTo })}
        form={form}
      />
      <EditCurrent key={`edit-${String(newest?.instanceId)}`} target={target} />
      <AddOldValue target={target} />
      <CsvImport target={target} measurements={measurements} complete={complete} />
    </div>
  );
}

/**
 * The row's "Correct values" tab for a person whose grant writes some attribute (T-4667, T-5326):
 * every version with who wrote it and why, and the current value edited, an old value added, a
 * measurement corrected or reverted and old values imported from CSV, each with a reason. A person
 * who only reads has the row's History tab: this one is not offered to them at all.
 */
export function AttributeHistory({ slug, entityId, attributes }: { slug: string; entityId: string; attributes: string[] }): JSX.Element {
  const { t } = useTranslation();
  const [attribute, setAttribute] = useState(attributes[0] ?? "");
  return (
    <div className="flex flex-col gap-4">
      <Field id="history-attribute" label={t("attributeHistory.attribute")} help={t("attributeHistory.attributeHint")}>
        <Select id="history-attribute" value={attribute} onChange={(e) => setAttribute(e.target.value)}>
          {attributes.map((name) => (
            <option key={name} value={name}>
              {name}
            </option>
          ))}
        </Select>
      </Field>
      <OneAttribute key={attribute} slug={slug} entityId={entityId} attribute={attribute} />
    </div>
  );
}
