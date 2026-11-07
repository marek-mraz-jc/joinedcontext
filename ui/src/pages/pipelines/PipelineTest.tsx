import { useId, useState } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { testPipeline } from "../../api/pipelineTest";
import type { SampleFormat, Trace } from "../../api/pipelineTest";
import { CodeBlock, RecordView, Alert, Button, Checkbox, FilePicker } from "../../components/ui";
import type { DebugEntry } from "./DebugPanel";
import { messagesOf } from "./pipelineDebug";
import type { PipelineForm } from "./PipelineEditor";
import type { Manifest } from "../../api/manifest";
import { FormHeading } from "../../components/forms/FormRoute";

/**
 * A candidate pipeline tried on a sample before it is proposed (PL-43, PL-44, UI-32): a dropped
 * CSV or JSON drafts the type and the Bloblang mapping, one click runs the mapping where it
 * will run (the project's runner, through the Portal), and the trace comes back as stages:
 * the input, what the mapping made of it, whether that is an entity, and every error at its
 * line. The sample stays in memory and in this request; nothing is written anywhere.
 */

/** The largest sample the route takes (API/01 §7a). */
export const MAX_SAMPLE_BYTES = 5 * 1024 * 1024;

export type { SampleFormat, Trace } from "../../api/pipelineTest";

export interface Draft {
  format: SampleFormat;
  type: string;
  idColumn: string;
  columns: string[];
  bloblang: string;
}

/**
 * How the `http` DataSource the draft reads hands a poll's body to the steps (PL-114): `csv`, one
 * message per row, when its `http.format` says so, else `whole`, the body as one message.
 * Undefined when the draft reads no `http` DataSource: then the sample's own format holds.
 */
export function sourceBodyOf(draft: PipelineForm | undefined, dataSources: Manifest[]): "csv" | "whole" | undefined {
  const name = draft?.source?.dataSourceRef;
  const source = name ? dataSources.find((candidate) => candidate.metadata.name === name) : undefined;
  if (source?.spec.type !== "http") {
    return undefined;
  }
  return (source.spec.http as { format?: unknown } | undefined)?.format === "csv" ? "csv" : "whole";
}

/**
 * The split a test runs: the source's, so the test sees the messages the live run sends. A CSV
 * sample of a source that sends its whole body stays one message, as it will live (T-5350).
 */
export function testFormatOf(format: SampleFormat, body: "csv" | "whole" | undefined): SampleFormat {
  if (body === "csv") {
    return "csv";
  }
  return body === "whole" && format === "csv" ? "text" : format;
}

/** The format a file name says, then what its first bytes say. */
export function formatOf(name: string, text: string): SampleFormat {
  const extension = name.toLowerCase().split(".").pop() ?? "";
  if (extension === "csv") {
    return "csv";
  }
  if (extension === "json") {
    return "json";
  }
  const head = text.trimStart();
  if (head.startsWith("{") || head.startsWith("[")) {
    return "json";
  }
  return text.split("\n", 1)[0]?.includes(",") ? "csv" : "text";
}

/** `air_quality-2026.csv` becomes `AirQuality2026`: an entity type short name (PF-42). */
export function typeOf(name: string): string {
  const stem = name.replace(/\.[^.]*$/, "");
  const words = stem.split(/[^A-Za-z0-9]+/).filter(Boolean);
  const type = words.map((w) => w[0].toUpperCase() + w.slice(1)).join("");
  return /^[A-Z]/.test(type) ? type : `Sample${type}`;
}

/** The NGSI-LD attribute kinds an object literal names as its `"type"`: no entity type. */
const ATTRIBUTE_KINDS = new Set([
  "Property",
  "Relationship",
  "GeoProperty",
  "LanguageProperty",
  "VocabProperty",
  "JsonProperty",
  "ListProperty",
  "ListRelationship",
]);

/**
 * The entity type a mapping writes when it says it as a literal, `root.type = "Vehicle"` or
 * `"type": "Vehicle"` in the object it builds (T-3971); `undefined` when it does not.
 */
export function literalTypeOf(bloblang: string): string | undefined {
  const assigned = /^\s*root\.type\s*=\s*"([A-Za-z][A-Za-z0-9_]*)"/m.exec(bloblang);
  if (assigned) {
    return assigned[1];
  }
  for (const match of bloblang.matchAll(/"type"\s*:\s*"([A-Za-z][A-Za-z0-9_]*)"/g)) {
    if (!ATTRIBUTE_KINDS.has(match[1])) {
      return match[1];
    }
  }
  return undefined;
}

/**
 * The form a dropped sample leaves (T-3971): an empty compute node takes the drafted mapping,
 * a typed one is kept; the output type follows the mapping's literal type, then the draft's,
 * and a type the form already holds is kept beside a kept mapping.
 */
export function withSample(form: PipelineForm, drafted: Draft): PipelineForm {
  const typed = form.compute?.bloblang ?? "";
  const keep = typed.trim() !== "";
  const bloblang = keep ? typed : drafted.bloblang;
  const type = literalTypeOf(bloblang) ?? (keep ? form.output?.type : undefined) ?? drafted.type;
  return {
    ...form,
    compute: keep ? form.compute : { kind: "bloblang", bloblang },
    output: { ...form.output, type, mode: form.output?.mode ?? "upsert" },
  };
}

/** The column a local id comes from (PL-44): `id`, `identifier`, one ending in `_id`/`Id`, or the first. */
export function idColumnOf(columns: string[]): string {
  const lower = columns.map((c) => c.toLowerCase());
  const exact = lower.findIndex((c) => c === "id" || c === "identifier");
  if (exact >= 0) {
    return columns[exact];
  }
  const suffix = columns.findIndex((c) => /(_id|Id|ID)$/.test(c));
  return columns[suffix >= 0 ? suffix : 0] ?? "id";
}

function isNumeric(value: unknown): boolean {
  return typeof value === "number" || (typeof value === "string" && value.trim() !== "" && !Number.isNaN(Number(value)));
}

function splitCsvLine(line: string): string[] {
  const delimiter = line.includes(";") && !line.includes(",") ? ";" : ",";
  return line.split(delimiter).map((cell) => cell.trim().replace(/^"(.*)"$/, "$1"));
}

/** The first record of the sample: column names and one value each, to see which are numbers. */
export function firstRecordOf(text: string, format: SampleFormat): Record<string, unknown> | null {
  if (format === "csv") {
    const [header, first] = text.split(/\r?\n/).filter((line) => line.trim() !== "");
    if (!header) {
      return null;
    }
    const columns = splitCsvLine(header);
    const values = first ? splitCsvLine(first) : [];
    return Object.fromEntries(columns.map((column, i) => [column, values[i] ?? ""]));
  }
  if (format === "json") {
    try {
      const parsed: unknown = JSON.parse(text);
      const record = Array.isArray(parsed) ? parsed[0] : parsed;
      return typeof record === "object" && record !== null && !Array.isArray(record)
        ? (record as Record<string, unknown>)
        : null;
    } catch {
      return null;
    }
  }
  return null;
}

/**
 * The mapping a sample drafts (PL-44): the id minted from the id column under the
 * organization's domain and the target space, the type, and one Property per other column,
 * as a number where the sample holds one.
 */
export function draftFromSample(name: string, text: string, space: string): Draft | null {
  const format = formatOf(name, text);
  const record = firstRecordOf(text, format);
  return record ? draftFromRecord(name, record, space, format) : null;
}

/**
 * The same draft from one record a source already answered (T-3211): a data source or an
 * endpoint picked as the source drafts like a dropped file does, so a person new to Bloblang
 * starts from a mapping that runs. `type` names the entity type when the target model gives one;
 * otherwise it comes from `name`.
 */
export function draftFromRecord(
  name: string,
  record: Record<string, unknown>,
  space: string,
  format: SampleFormat = "json",
  type: string = typeOf(name),
): Draft | null {
  const columns = Object.keys(record).filter((c) => c !== "" && c !== "@context");
  if (columns.length === 0) {
    return null;
  }
  const idColumn = idColumnOf(columns);
  const path = (column: string) => (/^[A-Za-z_][A-Za-z0-9_]*$/.test(column) ? `this.${column}` : `this.${JSON.stringify(column)}`);
  // Two columns that fold to one property name (`a-b`, `a b`) each keep their own: `a_b`, `a_b_2`
  // (F-R2UI4-016).
  const taken = new Set<string>();
  const propertyOf = (column: string) => {
    const base = column.replace(/[^A-Za-z0-9_]/g, "_");
    let property = base;
    for (let n = 2; taken.has(property); n += 1) property = `${base}_${n}`;
    taken.add(property);
    return property;
  };
  const lines = [
    `# drafted from ${name}: ${columns.length} columns, id from ${idColumn} (PL-44)`,
    `let domain = env("JC_ORG_DOMAIN")`,
    `root.id = "urn:ngsi-ld:%v:%v:%v:%v".format(${JSON.stringify(type)}, $domain, ${JSON.stringify(space)}, ${path(idColumn)}.string())`,
    `root.type = ${JSON.stringify(type)}`,
    ...columns
      .filter((column) => column !== idColumn && column !== "type")
      .map((column) => {
        const value = isNumeric(record[column]) ? `${path(column)}.number().catch(${path(column)})` : path(column);
        const property = propertyOf(column);
        return `root.${property} = { "type": "Property", "value": ${value} }`;
      }),
    "",
  ];
  return { format, type, idColumn, columns, bloblang: lines.join("\n") };
}

/** The space of `urn:ngsi-ld:Endpoint:{org}:{space}:{name}`, or the fallback. */
export function spaceOfTarget(target: string | undefined, fallback: string): string {
  const parts = target?.split(":") ?? [];
  return parts.length === 6 && parts[2] === "Endpoint" && parts[4] ? parts[4] : fallback;
}

/** The lines of the mapping an error names, 1-based. */
export function errorLines(trace: Trace): Set<number> {
  return new Set(trace.errors.flatMap((e) => (typeof e.line === "number" && e.line > 0 ? [e.line] : [])));
}

function readText(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(typeof reader.result === "string" ? reader.result : "");
    reader.onerror = () => reject(reader.error ?? new Error("read failed"));
    reader.readAsText(file);
  });
}

export interface PipelineTestProps {
  project: string;
  draft: PipelineForm | undefined;
  onChange: (form: PipelineForm) => void;
  /** The manifest the form is right now, as the dialog proposes it. */
  toManifest: (form: PipelineForm) => unknown;
  /** The `http` DataSource's own URL, offered as the sample the runner fetches (PL-48). */
  sampleUrl?: string;
  /** How that DataSource hands its body to the steps (`sourceBodyOf`, PL-114). */
  sourceBody?: "csv" | "whole";
  /** The test's answer for the mapping it ran (PL-49). */
  onVerdict?: (ok: boolean, bloblang: string) => void;
  /** Notifies whenever the test trace is produced or cleared. */
  onTrace?: (trace: Trace | null) => void;
  /** Every finished run, and the message it ran on in step mode, for the Debug sidebar (T-3222). */
  onDebug?: (entry: DebugEntry) => void;
}

/** A sample is a file held in memory, or a URL the runner fetches itself (PL-43, PL-48). */
type SampleSource = { name: string; format: SampleFormat } & ({ text: string; url?: undefined } | { url: string; text?: undefined });

export function PipelineTest({ project, draft, onChange, toManifest, sampleUrl, sourceBody, onVerdict, onTrace, onDebug }: PipelineTestProps): JSX.Element {
  const { t } = useTranslation();
  const [sample, setSample] = useState<SampleSource | null>(null);
  // The mapping a sample drafted while the compute node already held one: offered, never applied
  // unasked (T-3971).
  const [offered, setOffered] = useState<Draft | null>(null);
  const [trace, setTrace] = useState<Trace | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [running, setRunning] = useState(false);
  const bloblang = draft?.compute?.bloblang ?? "";
  // Step mode: one message of the sample per run, in order (T-3222).
  const [stepping, setStepping] = useState(false);
  const stepHint = useId();
  const [at, setAt] = useState(0);
  const messages = stepping && sample?.text !== undefined ? messagesOf(sample.text, sample.format) : [];

  async function takeFile(file: File | undefined) {
    if (!file) {
      return;
    }
    setTrace(null);
    onTrace?.(null);
    setError(null);
    if (file.size > MAX_SAMPLE_BYTES) {
      setSample(null);
      setError(t("pipelines.test.tooLarge"));
      return;
    }
    setAt(0);
    const text = await readText(file);
    const space = spaceOfTarget(draft?.targetEndpoint, project);
    const drafted = draftFromSample(file.name, text, space);
    setSample({ name: file.name, text, format: testFormatOf(drafted?.format ?? formatOf(file.name, text), sourceBody) });
    setOffered(drafted && bloblang.trim() !== "" ? drafted : null);
    if (drafted) {
      onChange(withSample(draft ?? { class: "auto" }, drafted));
    }
  }

  async function run(message?: number) {
    if (!sample || !draft) {
      return;
    }
    const text = message === undefined ? sample.text : messages[message];
    setRunning(true);
    setError(null);
    try {
      const answered = await testPipeline(
        project,
        toManifest(draft),
        sample.url !== undefined
          ? { url: sample.url, format: sample.format }
          : { text: text ?? "", format: sample.format },
      );
      if (!("trace" in answered)) {
        setTrace(null);
        onTrace?.(null);
        setError(answered.detail ?? t("pipelines.test.failed", { status: answered.status }));
        return;
      }
      const answer = answered.trace;
      setTrace(answer);
      onTrace?.(answer);
      onDebug?.({ message: message ?? null, trace: answer });
      if (message !== undefined) setAt(Math.min(message + 1, messages.length - 1));
      onVerdict?.(
        answer.errors.length === 0 && answer.validation.length > 0 && answer.validation.every((v) => v.ok),
        bloblang,
      );
    } catch {
      setTrace(null);
      onTrace?.(null);
      setError(t("pipelines.test.failed", { status: 0 }));
    } finally {
      setRunning(false);
    }
  }

  const lines = trace ? errorLines(trace) : new Set<number>();
  const ready = trace !== null && trace.errors.length === 0 && trace.validation.length > 0 && trace.validation.every((v) => v.ok);
  const panel = "flex min-w-0 flex-col gap-1 rounded-md border border-border bg-surface p-2";
  const pre = "max-h-48 overflow-auto whitespace-pre-wrap break-words font-mono text-caption";

  return (
    <section
      className="flex flex-col gap-3 rounded-md border border-border bg-surface-subtle p-3"
      aria-labelledby="studio-test"
      data-testid="pipeline-test"
      onDragOver={(event) => event.preventDefault()}
      onDrop={(event) => {
        event.preventDefault();
        void takeFile(event.dataTransfer.files[0]);
      }}
    >
      <FormHeading id="studio-test" className="text-body font-semibold text-fg">
        {t("pipelines.test.title")}
      </FormHeading>
      <p className="text-caption text-fg-muted">{t("pipelines.test.lead")}</p>
      {sourceBody === "whole" && sample?.text !== undefined && formatOf(sample.name, sample.text) === "csv" ? (
        <Alert tone="info" data-testid="test-whole-body">
          {t("pipelines.test.wholeBody")}
        </Alert>
      ) : null}
      <div className="flex flex-wrap items-center gap-2">
        <FilePicker
          label={t("pipelines.test.chooseFile")}
          accept=".csv,.json,.txt,text/csv,application/json,text/plain"
          onFile={(file) => void takeFile(file)}
          className="text-body"
        >
          <span className="rounded-md border border-dashed border-border px-3 py-2">{t("pipelines.test.drop")}</span>
        </FilePicker>
        {offered ? (
          <Button
            variant="secondary"
            size="sm"
            data-testid="replace-drafted"
            onClick={() => {
              onChange(withSample({ ...(draft ?? { class: "auto" }), compute: undefined }, offered));
              setOffered(null);
            }}
          >
            {t("pipelines.test.replaceDrafted")}
          </Button>
        ) : null}
        {sampleUrl ? (
          <Button
            size="sm"
            variant="secondary"
            onClick={() => {
              setTrace(null);
              onTrace?.(null);
              setError(null);
              setSample({ name: sampleUrl, url: sampleUrl, format: sourceBody === "csv" ? "csv" : "json" });
            }}
          >
            {t("pipelines.test.useUrl")}
          </Button>
        ) : null}
        {sample ? (
          <span className="font-mono text-caption text-fg-muted">
            {sample.url !== undefined
              ? t("pipelines.test.urlSample", { url: sample.url, format: sample.format })
              : t("pipelines.test.sample", { name: sample.name, format: sample.format, bytes: sample.text.length })}
          </span>
        ) : null}
        <Button
          size="sm"
          variant="primary"
          data-shortcut="test"
          loading={running}
          disabled={!sample || !bloblang}
          // Why the run is refused, where it is refused: a sentence beside the button said
          // "write a mapping first" only for the one case of two, and a button greyed for the
          // other told nobody anything (UI-44).
          disabledReason={
            !sample
              ? t("pipelines.test.noSample")
              : !bloblang
                ? t("pipelines.test.noMapping")
                : undefined
          }
          onClick={() => {
            void run(stepping && messages.length > 0 ? at : undefined);
          }}
        >
          {running
            ? t("pipelines.test.running")
            : stepping && messages.length > 0
              ? t("pipelines.debug.runMessage", { number: at + 1, total: messages.length })
              : t("pipelines.test.run")}
        </Button>
        <Checkbox
          label={t("pipelines.debug.stepMode")}
          aria-describedby={stepHint}
          checked={stepping}
          disabled={sample?.url !== undefined}
          disabledReason={sample?.url !== undefined ? t("pipelines.debug.stepModeFile") : undefined}
          onChange={(e) => {
            setStepping(e.target.checked);
            setAt(0);
          }}
        />
        <span id={stepHint} className="text-caption text-fg-muted">
          {t("pipelines.debug.stepModeHint")}
        </span>
        {stepping && messages.length > 0 ? (
          <>
            <Button size="sm" variant="ghost" disabled={at === 0} disabledReason={t("pipelines.debug.atFirst")} onClick={() => setAt(0)}>
              {t("pipelines.debug.startOver")}
            </Button>
            <span className="text-caption text-fg-muted" aria-live="polite">
              {t("pipelines.debug.position", { number: at + 1, total: messages.length })}
            </span>
          </>
        ) : null}
      </div>
      {error ? (
        <Alert role="alert" tone="danger">
          {error}
        </Alert>
      ) : null}
      {trace ? (
        <div className="flex flex-col gap-2" data-testid="stage-inspector">
          <div className="grid gap-2 md:grid-cols-3">
            <div className={panel}>
              <FormHeading sub className="text-caption font-semibold">{t("pipelines.test.input")}</FormHeading>
              <p className="text-caption text-fg-muted">
                {t("pipelines.test.events", { events: trace.input.events, bytes: trace.input.bytes })}
              </p>
              {trace.input.sample !== undefined ? <RecordView value={trace.input.sample} label={t("pipelines.test.input")} className="max-h-48" /> : null}
            </div>
            <div className={panel}>
              <FormHeading sub className="text-caption font-semibold">{t("pipelines.test.mapping")}</FormHeading>
              <p className="text-caption text-fg-muted">{t("pipelines.test.entities", { count: trace.mapping.length })}</p>
              {trace.mapping.length > 0 ? <RecordView value={trace.mapping[0]} label={t("pipelines.test.mapping")} className="max-h-48" /> : null}
            </div>
            <div className={panel}>
              <FormHeading sub className="text-caption font-semibold">{t("pipelines.test.validation")}</FormHeading>
              <ul className="text-caption">
                {trace.validation.map((v) => (
                  <li key={v.index} className={v.ok ? "text-success" : "text-danger"}>
                    #{v.index + 1} {v.ok ? t("pipelines.test.ok") : v.problems.join("; ")}
                  </li>
                ))}
              </ul>
            </div>
          </div>
          {trace.errors.length > 0 ? (
            <Alert tone="danger" title={t("pipelines.test.errors", { count: trace.errors.length })}>
              <ul className="text-caption">
                {trace.errors.map((e, i) => (
                  <li key={i}>
                    <span className="font-mono">{e.stage}</span>
                    {typeof e.line === "number" ? ` · ${t("pipelines.test.line", { line: e.line })}` : ""}: {e.message}
                  </li>
                ))}
              </ul>
            </Alert>
          ) : null}
          {lines.size > 0 && bloblang ? (
            <CodeBlock label={t("pipelines.field.bloblang")} className={pre}>
              {bloblang.split("\n").map((line, i) => (
                <span
                  key={i}
                  data-line={i + 1}
                  className={lines.has(i + 1) ? "block bg-danger/10 underline decoration-danger decoration-wavy" : "block"}
                >
                  {String(i + 1).padStart(3, " ")} {line}
                </span>
              ))}
            </CodeBlock>
          ) : null}
          {ready ? <Alert tone="success">{t("pipelines.test.ready", { count: trace.mapping.length })}</Alert> : null}
        </div>
      ) : null}
    </section>
  );
}
