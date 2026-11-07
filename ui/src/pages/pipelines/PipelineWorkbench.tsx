import { SpaceChanges } from "./SpaceChanges";
import { useEffect, useMemo, useRef, useState } from "react";
import type { JSX, ReactNode } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { callOperation } from "../../api/operations";
import { asManifests, refName } from "../../api/manifest";
import type { Manifest } from "../../api/manifest";
import { api, queryKeys, unwrap } from "../../api/client";
import { useModelSource } from "../../components/entities/filters";
import { FormHeading } from "../../components/forms/FormRoute";
import { Alert, Badge, Button, Card, Checkbox, Field, FilePicker, RadioGroup, Select, Table, TableBody, TableCell, TableHead, TableHeaderCell, TableRow, Textarea } from "../../components/ui";
import { classSlots, parseModel, unitCode } from "../models/linkml";
import type { LinkmlModel, LinkmlSlot } from "../models/linkml";
import { entityTypesOf, spaceOf } from "../spaces/spaceFacts";
import type { PipelineForm } from "./PipelineEditor";
import { inlineEndpointSample } from "../../api/pipelineTest";
import { plainValue, sampleUrlOf } from "./PipelineStudio";
import { MAX_SAMPLE_BYTES, draftFromRecord, draftFromSample, formatOf, sourceBodyOf, testFormatOf } from "./PipelineTest";
import type { SampleFormat } from "./PipelineTest";
import { SlotUnit } from "./SlotUnit";
import { OutputMapper } from "./OutputMapper";
import { LifetimeChoice } from "./LifetimeChoice";
import { timeSlotsOf } from "./history";
import { withEntry } from "./lifetime";
import { RecordTable, TargetCard, grantedNames, useTargetType } from "./WorkbenchTarget";
import type { FindingAccess } from "./WorkbenchTarget";
import { useAccessAsk } from "./AccessRequests";
import { HEADER } from "./outputMapper";

/** How long the mapping rests before the workbench tries it again: one pause in typing. */
export const QUIET_MS = 600;
/** The most enum values a slot's hint lists before it says how many more. */
const ENUM_SHOWN = 8;

type Sample = { text: string; format: SampleFormat } | { url: string; format: SampleFormat };
type Source = { dataSource: string } | { sample: Sample };

interface SampleAnswer {
  records: Record<string, unknown>[];
  fields: string[];
  count: number;
  truncated: boolean;
  errors: TraceError[];
  /** A streaming source's sample: the messages that arrived while it was listened to (T-4841). */
  streamed: boolean;
  /** How long a streaming source was listened to, at most. */
  seconds: number;
  /** Every message of a streamed sample as it arrived, the mapping's input one message each. */
  messages: unknown[];
}

/** The most messages a streamed sample holds (jcctl's MAX_MESSAGES). */
export const STREAM_MESSAGES = 20;

interface TraceError {
  stage: string;
  step?: number | null;
  line?: number | null;
  message: string;
}

interface MappingAnswer {
  records: unknown[];
  errors: TraceError[];
}

interface Problem {
  rule: string;
  path: string;
  message: string;
}

interface ValidationAnswer {
  space: string;
  model: string;
  version: string;
  valid: number;
  rejected: number;
  verdicts: RecordVerdict[];
}

/** Where a record's write is judged (T-4840): the space's model, the target Endpoint, the lifetime. */
export type FindingSource = "model" | "endpoint" | "lifetime" | "history";

/** One reason a record would not be written, from the check of its write (T-4840). */
interface Finding {
  source: FindingSource;
  rule: string;
  field: string;
  reason: string;
  /** The access request that would cure a missing grant (PL-90, T-5369). */
  access?: FindingAccess;
}

interface RecordVerdict {
  index: number;
  ok: boolean;
  problems: Problem[];
  /** Absent from an answer older than the record check: the problems are then the model's alone. */
  findings?: Finding[];
  /** The `expiresAt` the record would be written with: the entity's and each attribute's. */
  expires?: { entity?: string; attributes: Record<string, string> };
}

const SOURCES: readonly FindingSource[] = ["model", "endpoint", "lifetime", "history"];

/** The value a mapping writes most often for a slot's range, shown as its example. */
export function exampleOf(slot: Pick<LinkmlSlot, "range" | "kind" | "multivalued">, enumFirst?: string): string {
  if (enumFirst) {
    return JSON.stringify(enumFirst);
  }
  if (slot.kind === "Relationship") {
    // The target's own id (PF-10): a mapping builds it from the source's key for that entity, and
    // an id of another class is refused as target-wrong-type (DM-70).
    const urn = `"urn:ngsi-ld:${slot.range ?? "…"}:…"`;
    return slot.multivalued ? `[${urn}]` : urn;
  }
  switch ((slot.range ?? "string").toLowerCase()) {
    case "integer":
      return "12";
    case "float":
    case "double":
    case "decimal":
      return "3.5";
    case "boolean":
      return "true";
    case "date":
      return '"2026-09-25"';
    case "datetime":
      return '"2026-09-25T08:00:00Z"';
    default:
      return '"…"';
  }
}

/**
 * The class a mapping writes: the type the pipeline's output declares, else the first of the
 * model's classes its `root.type = "…"` or a `"type": "…"` member names (a `bento.yaml` builds
 * whole entities, T-4818), else the model's only class. Never a guess among several: a Vehicle
 * pipeline once showed the slots of Event, the model's first class (T-4842).
 */
export function classOfMapping(bloblang: string, classes: string[], declared?: string): string | undefined {
  if (declared && classes.includes(declared)) return declared;
  const named = [...bloblang.matchAll(/(?:root\.type\s*=|"type"\s*:)\s*"([A-Za-z][A-Za-z0-9_]*)"/g)]
    .map((match) => match[1])
    .find((name) => classes.includes(name));
  return named ?? (classes.length === 1 ? classes[0] : undefined);
}

/** The type the output mapper's own step writes, from its `# jc-mapper` header. */
export function mapperType(bloblang: string): string | undefined {
  if (!bloblang.startsWith(HEADER)) return undefined;
  try {
    const type = (JSON.parse(bloblang.slice(HEADER.length).split("\n", 1)[0]) as { type?: unknown }).type;
    return typeof type === "string" ? type : undefined;
  } catch {
    return undefined;
  }
}

/** The sample the mapping is tried on, from the file the person gave or from the source. */
export function sampleFor(
  draft: PipelineForm | undefined,
  file: { text: string; format: SampleFormat } | null,
  dataSources: Manifest[],
  endpoints: Manifest[],
): Sample | undefined {
  // The source decides the split, so the sample reaches the steps as the live run sends it (PL-114).
  const body = sourceBodyOf(draft, dataSources);
  if (file) {
    return { text: file.text, format: testFormatOf(file.format, body) };
  }
  const url = sampleUrlOf(draft, dataSources, endpoints);
  if (!url) {
    return undefined;
  }
  const path = url.split(/[?#]/)[0].toLowerCase();
  return { url, format: testFormatOf(path.endsWith(".csv") ? "csv" : "json", body) };
}

/** What the sample step reads: a DataSource by name, the endpoint's page, or the file. */
function sourceFor(
  draft: PipelineForm | undefined,
  file: { text: string; format: SampleFormat } | null,
  sample: Sample | undefined,
): Source | undefined {
  if (file) {
    return { sample: { text: file.text, format: file.format } };
  }
  const dataSource = draft?.source?.dataSourceRef;
  if (dataSource) {
    return { dataSource };
  }
  return sample ? { sample } : undefined;
}

function useQuiet<T>(value: T, ms: number): T {
  const [quiet, setQuiet] = useState(value);
  useEffect(() => {
    const timer = setTimeout(() => setQuiet(value), ms);
    return () => clearTimeout(timer);
  }, [value, ms]);
  return quiet;
}

const isObject = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);
const listOf = (value: unknown): unknown[] => (Array.isArray(value) ? value : []);
const numberOf = (value: unknown): number => (typeof value === "number" && Number.isFinite(value) ? value : 0);
const textOf = (value: unknown): string => (typeof value === "string" ? value : "");

function traceErrors(value: unknown): TraceError[] {
  return listOf(value)
    .filter(isObject)
    .map((error) => ({
      stage: textOf(error.stage),
      step: typeof error.step === "number" ? error.step : null,
      line: typeof error.line === "number" ? error.line : null,
      message: textOf(error.message),
    }));
}

/** The sample step's answer, read member by member: an answer of another shape is no sample. */
export function readSample(output: unknown): SampleAnswer {
  const answer = isObject(output) ? output : {};
  const records = listOf(answer.records).filter(isObject);
  const streamed = answer.streamed === true;
  return {
    records,
    fields: listOf(answer.fields).filter((field): field is string => typeof field === "string"),
    count: numberOf(answer.count) || records.length,
    truncated: answer.truncated === true,
    errors: traceErrors(answer.errors),
    streamed,
    seconds: numberOf(answer.seconds),
    messages: streamed ? listOf(answer.records) : [],
  };
}

/** Whether the DataSource named streams (an MQTT topic, a queue) rather than answers a request. */
export function streamsSource(dataSources: Manifest[], name: string | undefined): boolean {
  const spec = dataSources.find((source) => source.metadata.name === name)?.spec as Record<string, unknown> | undefined;
  return typeof spec?.type === "string" && spec.type !== "http";
}

/** Whole seconds since `active` turned true, ticking while it stays so; 0 while it is false. */
function useElapsed(active: boolean): number {
  const [elapsed, setElapsed] = useState(0);
  useEffect(() => {
    if (!active) return;
    const started = Date.now();
    const timer = setInterval(() => setElapsed(Math.floor((Date.now() - started) / 1000)), 1000);
    return () => {
      clearInterval(timer);
      setElapsed(0);
    };
  }, [active]);
  return active ? elapsed : 0;
}

/** The mapping step's answer, read member by member. */
export function readMapping(output: unknown): MappingAnswer {
  const answer = isObject(output) ? output : {};
  return { records: listOf(answer.records), errors: traceErrors(answer.errors) };
}

/** The validation step's answer, read member by member. */
export function readValidation(output: unknown): ValidationAnswer {
  const answer = isObject(output) ? output : {};
  const verdicts = listOf(answer.verdicts)
    .filter(isObject)
    .map((verdict) => ({
      index: numberOf(verdict.index),
      ok: verdict.ok === true,
      problems: listOf(verdict.problems)
        .filter(isObject)
        .map((problem) => ({ rule: textOf(problem.rule), path: textOf(problem.path), message: textOf(problem.message) })),
      ...readCheck(verdict),
    }));
  return {
    space: textOf(answer.space),
    model: textOf(answer.model),
    version: textOf(answer.version),
    valid: numberOf(answer.valid),
    rejected: numberOf(answer.rejected),
    verdicts,
  };
}

/** A finding's access request, when the server names one whole (T-5369). */
function accessOf(access: unknown): { access?: FindingAccess } {
  if (!isObject(access) || typeof access.project !== "string" || typeof access.name !== "string") return {};
  if (access.operations !== "read" && access.operations !== "write") return {};
  const type = typeof access.type === "string" && access.type ? { type: access.type } : {};
  return { access: { project: access.project, name: access.name, operations: access.operations, ...type } };
}

/** A verdict's record check (T-4840), when the answer carries one. */
function readCheck(verdict: Record<string, unknown>): Pick<RecordVerdict, "findings" | "expires"> {
  const out: Pick<RecordVerdict, "findings" | "expires"> = {};
  if (Array.isArray(verdict.findings)) {
    out.findings = verdict.findings.filter(isObject).map((finding) => ({
      source: SOURCES.find((source) => source === finding.source) ?? "model",
      rule: textOf(finding.rule),
      field: textOf(finding.field),
      reason: textOf(finding.reason),
      ...accessOf(finding.access),
    }));
  }
  if (isObject(verdict.expires)) {
    const { entity, attributes } = verdict.expires;
    out.expires = {
      ...(typeof entity === "string" ? { entity } : {}),
      attributes: Object.fromEntries(
        Object.entries(isObject(attributes) ? attributes : {}).filter(
          (entry): entry is [string, string] => typeof entry[1] === "string",
        ),
      ),
    };
  }
  return out;
}

async function run<T>(
  project: string,
  name: string,
  input: Record<string, unknown>,
  failed: string,
  read: (output: unknown) => T,
): Promise<T> {
  const answer = await callOperation(project, name, input);
  if (!answer.ok) {
    throw new StepFailed(answer.status, answer.reason ?? failed);
  }
  return read(answer.output);
}

/** A step the Portal refused, with the status it refused with. */
export class StepFailed extends Error {
  constructor(
    readonly status: number,
    message: string,
  ) {
    super(message);
  }
}

/** A target space that names no model answers 409: nothing narrows what lands there (DM-61). */
const unmodelled = (error: unknown): boolean => error instanceof StepFailed && error.status === 409;

/** The engine's JSON read hit markup: the source answered a web page, not data (T-4165). */
export function answeredHtml(message: string): boolean {
  return /invalid character '<'|unexpected token '?</i.test(message);
}

function cell(value: unknown): string {
  const text = plainValue(value);
  return text.length > 60 ? `${text.slice(0, 59)}…` : text;
}

/** `record`'s members by dotted path, objects opened two levels deep; lists stay whole. */
export function flatten(record: Record<string, unknown>, prefix = "", depth = 0): Record<string, unknown> {
  return Object.fromEntries(
    Object.entries(record).flatMap(([key, value]) =>
      isObject(value) && depth < 2 ? Object.entries(flatten(value, `${prefix}${key}.`, depth + 1)) : [[`${prefix}${key}`, value]],
    ),
  );
}

/**
 * A streaming source's sample (T-4841): the messages that arrived while it was listened to, how
 * many of the most it takes, the ones it could not keep, and a way to listen again.
 */
function StreamedSample({ answer, onAgain }: { answer: SampleAnswer; onAgain: () => void }): JSX.Element {
  const { t } = useTranslation();
  const again = (
    <Button size="sm" variant="secondary" onClick={onAgain}>
      {t("pipelines.workbench.sample.listenAgain")}
    </Button>
  );
  // An MQTT message is often one object under one key (HSL's `{"VP": {…}}`): its members are the
  // columns, by path, so the table shows values rather than the message's JSON.
  const rows = answer.messages.map((message) => (isObject(message) ? flatten(message) : { message }));
  const fields = [...new Set(rows.flatMap((row) => Object.keys(row)))].sort();
  return (
    <>
      {answer.errors.length > 0 ? (
        <Alert tone="warning" title={t("pipelines.workbench.sample.streamSkipped", { count: answer.errors.length })}>
          <ul className="list-disc pl-4">
            {answer.errors.slice(0, 3).map((error, index) => (
              <li key={index}>{error.message}</li>
            ))}
          </ul>
        </Alert>
      ) : null}
      {rows.length === 0 ? (
        <Alert tone="info" title={t("pipelines.workbench.sample.streamEmptyTitle", { seconds: answer.seconds })} actions={again}>
          {t("pipelines.workbench.sample.streamEmpty")}
        </Alert>
      ) : (
        <>
          <div className="flex flex-wrap items-center justify-between gap-2">
            <p role="status" className="text-caption text-fg-subtle">
              {t("pipelines.workbench.sample.streamCount", {
                count: rows.length,
                max: STREAM_MESSAGES,
                seconds: answer.seconds,
                fields: fields.length,
              })}
            </p>
            {again}
          </div>
          <Table caption={t("pipelines.workbench.sample.caption")} maxHeight="max-h-64">
            <TableHead>
              {fields.map((field) => (
                <TableHeaderCell key={field}>
                  <span className="font-mono">{field}</span>
                </TableHeaderCell>
              ))}
            </TableHead>
            <TableBody>
              {rows.map((record, index) => (
                <TableRow key={index}>
                  {fields.map((field) => (
                    <TableCell key={field}>
                      <span className="font-mono text-caption">{cell(record[field])}</span>
                    </TableCell>
                  ))}
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </>
      )}
    </>
  );
}

function Step({
  number,
  id,
  title,
  hint,
  children,
}: {
  number: number;
  id: string;
  title: string;
  hint: string;
  children: ReactNode;
}): JSX.Element {
  return (
    <Card as="section" padding="md" aria-labelledby={`${id}-title`} className="flex flex-col gap-3">
      <div>
        <FormHeading id={`${id}-title`} className="text-body font-semibold text-fg">
          <span className="mr-2 tabular-nums text-fg-subtle">{number}.</span>
          {title}
        </FormHeading>
        <p className="text-caption text-fg-muted">{hint}</p>
      </div>
      {children}
    </Card>
  );
}

export interface PipelineWorkbenchProps {
  project: string;
  draft: PipelineForm | undefined;
  onChange: (form: PipelineForm) => void;
  dataSources: Manifest[];
  endpoints: Manifest[];
  /** The target endpoints a pipeline may write through, as the form offers them. */
  targets: { name: string; urn?: string }[];
  /** The manifest the form is right now, for the steps that run it (PL-43). */
  toManifest: (form: PipelineForm) => unknown;
  /** Whether the mapping in the editor wrote only valid records the last time it ran (PL-49). */
  onVerdict: (ok: boolean, bloblang: string) => void;
  /** More of the dialog under the six steps: the folded advanced editor. */
  children?: ReactNode;
  /**
   * The `bento.yaml` the pipeline runs when it has no compute step (PL-41, T-4818): step 4 shows
   * it in place of the Bloblang box, read-only until the person switches Edit on.
   */
  mappingFile?: MappingFile;
  /**
   * The Portal draft the dialog edits: the validation step's verdict is kept on it, and the
   * proposal needs that verdict green for the spec it proposes (T-4840, PF-57).
   */
  draftName?: string;
  /** The proposed pipeline the dialog edits, whose account an access request asks for (T-5369). */
  pipeline?: string;
}

/** The `bento.yaml` beside an edited pipeline, as the editor loaded it (T-4818). */
export interface MappingFile {
  /** `loading` until the forge answered, `absent` for a pipeline with no such file. */
  state: "loading" | "loaded" | "absent" | "failed";
  text: string;
  editing: boolean;
  onEditing: (editing: boolean) => void;
  onText: (text: string) => void;
  /** Why the file could not be read, with a retry, when `state` is `failed`. */
  failure?: ReactNode;
}

/**
 * The pipeline workbench (ADR-N-034, PL-58): source, sample, target, mapping, mapped output and
 * validation, in that order; the target comes before the mapping so the mapping opens knowing the
 * model it has to produce (T-3296). Each step shows its output and a one-line hint, and runs
 * the operation an agent or the assistant runs for the same step (PL-63): the sample and the
 * mapping on the project's runner, the verdict against the target space's model. Nothing here is
 * written; Propose, under the steps, opens the Change.
 */
export function PipelineWorkbench({
  project,
  draft,
  onChange,
  dataSources,
  endpoints,
  targets,
  toManifest,
  onVerdict,
  children,
  mappingFile,
  draftName,
  pipeline,
}: PipelineWorkbenchProps): JSX.Element {
  const { t } = useTranslation();
  const [file, setFile] = useState<{ name: string; text: string; format: SampleFormat } | null>(null);
  const [fileError, setFileError] = useState<string | null>(null);

  const spaces = useQuery({
    queryKey: queryKeys.list(project, "spaces"),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "spaces" } },
        }),
      ),
  });
  const models = useQuery({
    queryKey: queryKeys.list(project, "datamodels"),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "datamodels" } },
        }),
      ),
  });
  const spaceList = useMemo(() => asManifests(spaces.data?.items ?? []), [spaces.data]);
  const modelList = useMemo(() => asManifests(models.data?.items ?? []), [models.data]);

  /** The model the space of an endpoint names (DM-61), by the endpoint's name. */
  const modelOfEndpoint = (name: string | undefined): Manifest | undefined => {
    const endpoint = endpoints.find((candidate) => candidate.metadata.name === name);
    const space = endpoint ? spaceList.find((s) => s.metadata.name === spaceOf(endpoint)) : undefined;
    const model = refName(space?.spec.dataModelRef);
    return model ? modelList.find((m) => m.metadata.name === model) : undefined;
  };

  // Step 1: where the records come from.
  const sourceValue = draft?.source?.dataSourceRef
    ? `datasource:${draft.source.dataSourceRef}`
    : draft?.source?.endpointRef
      ? `endpoint:${draft.source.endpointRef}`
      : "";
  const pickSource = (value: string) => {
    const [kind, ...rest] = value.split(":");
    const name = rest.join(":");
    const source =
      kind === "datasource" ? { dataSourceRef: name } : kind === "endpoint" ? { endpointRef: name } : undefined;
    onChange({ ...(draft ?? {}), source });
  };
  const readModel = modelOfEndpoint(draft?.source?.endpointRef);
  const readTypes = readModel ? entityTypesOf(readModel) : [];
  const queried = draft?.source?.query?.type;
  const readType = typeof queried === "string" && queried !== "" ? queried : undefined;
  const pickReadType = (type: string) =>
    onChange({
      ...(draft ?? {}),
      source: { ...(draft?.source ?? {}), query: { ...(draft?.source?.query ?? {}), type: type || undefined } },
    });

  const takeFile = async (picked: File) => {
    setFileError(null);
    if (picked.size > MAX_SAMPLE_BYTES) {
      setFileError(t("pipelines.test.tooLarge"));
      return;
    }
    const text = await picked.text();
    const format = formatOf(picked.name, text);
    setFile({ name: picked.name, text, format });
    // An empty mapping is drafted from the file, so the first try runs before anything is typed (PL-44).
    const target = targets.find((candidate) => candidate.urn === draft?.targetEndpoint);
    const space = target?.urn?.split(":")[4] ?? "";
    if (!draft?.compute?.bloblang?.trim()) {
      const drafted = draftFromSample(picked.name, text, space);
      if (drafted) {
        onChange({ ...(draft ?? {}), compute: { kind: "bloblang", bloblang: drafted.bloblang } });
      }
    }
  };

  // Step 2: the sample.
  const sample = sampleFor(draft, file, dataSources, endpoints);
  const source = sourceFor(draft, file, sample);
  // A page of one of this platform's endpoints is read here, once, with the person's session
  // (T-3088): the runner fetches with no credential and the endpoint answered it 401.
  const inlined = useQuery({
    queryKey: ["pipeline-workbench", project, "inline", sample],
    enabled: sample !== undefined && !file,
    retry: false,
    refetchOnWindowFocus: false,
    queryFn: async () => {
      const answer = await inlineEndpointSample(sample as Sample);
      if (!("sample" in answer)) throw new StepFailed(answer.status, answer.detail);
      return answer.sample;
    },
  });
  const sentSample = file ? sample : inlined.data;
  const readsSample = source !== undefined && "sample" in source;
  const sampled = useQuery({
    queryKey: ["pipeline-workbench", project, "sample", source, readsSample ? sentSample : null],
    enabled: source !== undefined && (!readsSample || sentSample !== undefined),
    retry: false,
    // Each run reaches the project's runner: a person coming back to the tab asks it nothing new.
    refetchOnWindowFocus: false,
    queryFn: () =>
      run(
        project,
        "jc_pipeline_sample_source",
        readsSample ? { sample: sentSample } : (source ?? {}),
        t("pipelines.workbench.failed"),
        readSample,
      ),
  });

  const listening = streamsSource(dataSources, draft?.source?.dataSourceRef) && !file;
  const elapsed = useElapsed(listening && sampled.isFetching);
  // A streamed sample is tried as its messages, one each, as the live input hands them over.
  const messages = sampled.data?.streamed && !file ? sampled.data.messages : undefined;
  const tryInput = messages && messages.length > 0 ? { messages } : sentSample !== undefined ? { sample: sentSample } : undefined;

  // Steps 4 and 5: the mapping, tried once typing rests.
  const bloblang = draft?.compute?.kind === "bloblang" || draft?.compute?.kind === undefined ? (draft?.compute?.bloblang ?? "") : "";
  const quiet = useQuiet(bloblang, QUIET_MS);
  // Fields mapped onto the type's attributes, or Bloblang written by hand (T-3224): the mapper for a
  // step it wrote, the code for any other, an empty one and a draft from a dropped file included,
  // until the person picks one.
  const [chosenMode, setMappingMode] = useState<"map" | "code" | null>(null);
  const mappingMode = chosenMode ?? (bloblang.startsWith(HEADER) ? "map" : "code");
  // The manifest carries the rested mapping, not every keystroke: a query key is compared by
  // value, so a draft that changed nowhere else asks the runner nothing new.
  const manifest = useMemo(
    () => (draft ? toManifest({ ...draft, compute: { ...(draft.compute ?? {}), kind: "bloblang", bloblang: quiet } }) : undefined),
    [draft, quiet, toManifest],
  );
  const tried = useQuery({
    queryKey: ["pipeline-workbench", project, "mapping", manifest, tryInput, draftName],
    // After the sample step, never beside it: a project runs one test at a time, and the two
    // started together answered "a pipeline test is already running" on every page load.
    enabled: quiet.trim() !== "" && tryInput !== undefined && manifest !== undefined && !sampled.isFetching,
    retry: false,
    refetchOnWindowFocus: false,
    queryFn: () =>
      run(
        project,
        "jc_pipeline_try_mapping",
        // The record check of what the mapping makes is kept on the dialog's draft (T-4840).
        { pipeline: manifest, ...tryInput, ...(draftName ? { draft: { kind: "Pipeline", name: draftName } } : {}) },
        t("pipelines.workbench.failed"),
        readMapping,
      ),
  });
  const records = (tried.data?.records ?? []).filter(
    (record): record is Record<string, unknown> => typeof record === "object" && record !== null && !Array.isArray(record),
  );

  // Step 6: the verdicts against the target space's model.
  const validated = useQuery({
    queryKey: ["pipeline-workbench", project, "validate", manifest, records],
    enabled: records.length > 0 && Boolean(draft?.targetEndpoint),
    retry: false,
    refetchOnWindowFocus: false,
    queryFn: () =>
      run(
        project,
        "jc_pipeline_validate",
        {
          pipeline: manifest,
          records: records.slice(0, 100),
        },
        t("pipelines.workbench.failed"),
        readValidation,
      ),
  });
  const verdictOf = (index: number) => validated.data?.verdicts.find((verdict) => verdict.index === index);
  const access = useAccessAsk(project, pipeline, validated.data?.verdicts.some((verdict) => verdict.findings?.some((f) => f.access)) ?? false);

  const noModel = validated.isError && unmodelled(validated.error);
  const settled = tried.isSuccess && (validated.isSuccess || noModel) && quiet === bloblang;
  const ok =
    settled &&
    tried.data?.errors.length === 0 &&
    records.length > 0 &&
    (noModel || validated.data?.rejected === 0);
  // The verdict belongs to the mapping it ran on; a new callback from the dialog is no new verdict.
  const report = useRef(onVerdict);
  useEffect(() => {
    report.current = onVerdict;
  }, [onVerdict]);
  useEffect(() => {
    if (settled) {
      report.current(ok, quiet);
    }
  }, [settled, ok, quiet]);

  // Step 4's hints: the slots of the class the mapping writes, from the target space's model.
  const targetName = targets.find((candidate) => candidate.urn === draft?.targetEndpoint)?.name;
  const targetEndpoint = endpoints.find((candidate) => candidate.metadata.name === targetName);
  const targetSpace = targetEndpoint ? spaceOf(targetEndpoint) : undefined;
  const targetModel = modelOfEndpoint(targetName);
  const modelText = useModelSource(project, targetModel);
  const parsed = useMemo<LinkmlModel | undefined>(() => {
    if (!modelText) {
      return undefined;
    }
    try {
      return parseModel(modelText);
    } catch {
      return undefined;
    }
  }, [modelText]);
  const classNames = parsed?.classes.map((c) => c.name) ?? [];
  // The mapping file stands in for the Bloblang box while the pipeline has no compute step.
  const fileShown = mappingFile !== undefined && bloblang.trim() === "" && mappingFile.state !== "absent";
  const [pickedClass, setPickedClass] = useState<string | undefined>(undefined);
  const writtenClass =
    pickedClass && classNames.includes(pickedClass)
      ? pickedClass
      : classOfMapping(fileShown ? mappingFile.text : bloblang, classNames, draft?.output?.type);
  const klass = parsed?.classes.find((c) => c.name === writtenClass);
  // The type the Target card and the Lifetime step are about: the model's class above, else the
  // output's declared type or the mapper's own, while the model's text is not read (T-4842).
  const writtenType = writtenClass ?? draft?.output?.type ?? mapperType(bloblang);
  const target = useTargetType(project, targetSpace, writtenType, manifest, targetName);
  // The History column's verdicts: the record check's history findings, once per attribute and rule.
  const historyFindings = validated.data
    ? [
        ...new Map(
          validated.data.verdicts
            .flatMap((verdict) => verdict.findings ?? [])
            .filter((finding) => finding.source === "history")
            .map((finding) => [`${finding.field} ${finding.rule}`, finding] as const),
        ).values(),
      ]
    : undefined;
  const slots = parsed && klass ? classSlots(parsed, klass).filter((slot) => !["id", "type"].includes(slot.name)) : [];

  // T-3211: a source picked from the project drafts the mapping from its first record, once, as a
  // dropped file does, so a person new to Bloblang starts from a mapping that runs; whatever the
  // person has typed is never replaced. The type is the target model's when it has only one class.
  const firstRecord = file ? undefined : sampled.data?.records[0];
  const sourceLabel = draft?.source?.dataSourceRef ?? draft?.source?.endpointRef;
  const onlyClass = classNames.length === 1 ? classNames[0] : undefined;
  const targetSpaceOfUrn = targets.find((candidate) => candidate.urn === draft?.targetEndpoint)?.urn?.split(":")[4] ?? "";
  const draftedFor = useRef<string | null>(null);
  useEffect(() => {
    if (!firstRecord || !sourceLabel || bloblang.trim() !== "" || draftedFor.current === sourceLabel) return;
    draftedFor.current = sourceLabel;
    const drafted = draftFromRecord(sourceLabel, firstRecord, targetSpaceOfUrn, "json", onlyClass);
    if (drafted) onChange({ ...(draft ?? {}), compute: { kind: "bloblang", bloblang: drafted.bloblang } });
  }, [firstRecord, sourceLabel, bloblang, onlyClass, targetSpaceOfUrn, draft, onChange]);
  const addLine = (line: string) =>
    onChange({
      ...(draft ?? {}),
      compute: {
        ...(draft?.compute ?? {}),
        kind: "bloblang",
        bloblang: bloblang.trimEnd() === "" ? line : `${bloblang.trimEnd()}\n${line}`,
      },
    });

  const failedText = (error: unknown) => (error instanceof Error ? error.message : t("pipelines.workbench.failed"));

  return (
    <div className="flex flex-col gap-3" data-testid="pipeline-workbench">
      <Step number={1} id="workbench-source" title={t("pipelines.workbench.source.title")} hint={t("pipelines.workbench.source.hint")}>
        <Field id="workbench-source-pick" label={t("pipelines.workbench.source.pick")} description={t("pipelines.workbench.source.pickHint")}>
          <Select id="workbench-source-pick" data-field="source" value={sourceValue} onChange={(event) => pickSource(event.target.value)}>
            <option value="">{t("pipelines.workbench.source.none")}</option>
            {dataSources.length > 0 ? (
              <optgroup label={t("nav.datasources")}>
                {dataSources.map((candidate) => (
                  <option key={candidate.metadata.name} value={`datasource:${candidate.metadata.name}`}>
                    {candidate.metadata.name}
                  </option>
                ))}
              </optgroup>
            ) : null}
            {endpoints.length > 0 ? (
              <optgroup label={t("nav.endpoints")}>
                {endpoints.map((candidate) => (
                  <option key={candidate.metadata.name} value={`endpoint:${candidate.metadata.name}`}>
                    {candidate.metadata.name}
                  </option>
                ))}
              </optgroup>
            ) : null}
          </Select>
        </Field>
        {draft?.source?.endpointRef ? (
          <Field id="workbench-read-type" label={t("pipelines.workbench.source.type")} description={t("pipelines.workbench.source.typeHint")}>
            <Select id="workbench-read-type" value={readType ?? ""} onChange={(event) => pickReadType(event.target.value)}>
              <option value="">{t("pipelines.workbench.source.pickType")}</option>
              {[...new Set([...readTypes, ...(readType ? [readType] : [])])].map((type) => (
                <option key={type} value={type}>
                  {type}
                </option>
              ))}
            </Select>
          </Field>
        ) : null}
        <div className="flex flex-wrap items-center gap-2">
          <FilePicker
            label={t("pipelines.workbench.source.file")}
            accept=".csv,.json,.txt,text/csv,application/json,text/plain"
            onFile={(picked) => void takeFile(picked)}
            className="text-body"
          >
            <span className="rounded-md border border-dashed border-border px-3 py-2">
              {file ? t("pipelines.workbench.source.fileTaken", { name: file.name }) : t("pipelines.workbench.source.fileDrop")}
            </span>
          </FilePicker>
        </div>
        {fileError ? (
          <Alert role="alert" tone="danger">
            {fileError}
          </Alert>
        ) : null}
      </Step>

      <Step number={2} id="workbench-sample" title={t("pipelines.workbench.sample.title")} hint={t("pipelines.workbench.sample.hint")}>
        {source === undefined ? (
          <p className="text-caption text-fg-subtle">{t("pipelines.workbench.sample.waiting")}</p>
        ) : inlined.isError ? (
          <Alert role="alert" tone="danger">
            {failedText(inlined.error)}
          </Alert>
        ) : listening && sampled.isFetching ? (
          <p role="status" className="text-caption text-fg-subtle">
            {t("pipelines.workbench.sample.listening", {
              source: draft?.source?.dataSourceRef ?? "",
              max: STREAM_MESSAGES,
              elapsed,
              seconds: sampled.data?.seconds || 5,
            })}
          </p>
        ) : sampled.isPending ? (
          <p role="status" className="text-caption text-fg-subtle">
            {t("pipelines.workbench.running")}
          </p>
        ) : sampled.isError ? (
          <Alert role="alert" tone="danger">
            {failedText(sampled.error)}
          </Alert>
        ) : sampled.data.streamed ? (
          <StreamedSample answer={sampled.data} onAgain={() => void sampled.refetch()} />
        ) : sampled.data.errors.length > 0 ? (
          <Alert role="alert" tone="danger">
            {/* A source that answered a web page fails the engine's JSON read with "invalid
                character '<'": said in words, the engine's line kept behind a disclosure
                (T-4165, photo audit T-4159). */}
            {answeredHtml(sampled.data.errors[0].message) ? (
              <>
                {t("pipelines.workbench.sample.notJson")}
                <details className="mt-1 text-caption">
                  <summary className="cursor-pointer">{t("pipelines.workbench.sample.details")}</summary>
                  <p className="mt-0.5 break-all font-mono">{sampled.data.errors[0].message}</p>
                </details>
              </>
            ) : (
              sampled.data.errors[0].message
            )}
          </Alert>
        ) : sampled.data.count === 0 ? (
          <p className="text-caption text-fg-subtle">{t("pipelines.workbench.sample.empty")}</p>
        ) : (
          <>
            <p className="text-caption text-fg-subtle">
              {t(sampled.data.truncated ? "pipelines.workbench.sample.countFirst" : "pipelines.workbench.sample.count", {
                count: sampled.data.count,
                fields: sampled.data.fields.length,
              })}
            </p>
            <Table caption={t("pipelines.workbench.sample.caption")} maxHeight="max-h-64">
              <TableHead>
                {sampled.data.fields.map((field) => (
                  <TableHeaderCell key={field}>
                    <span className="font-mono">{field}</span>
                  </TableHeaderCell>
                ))}
              </TableHead>
              <TableBody>
                {sampled.data.records.map((record, index) => (
                  <TableRow key={index}>
                    {sampled.data.fields.map((field) => (
                      <TableCell key={field}>
                        <span className="font-mono text-caption">{cell(record[field])}</span>
                      </TableCell>
                    ))}
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          </>
        )}
      </Step>

      <Step number={3} id="workbench-target" title={t("pipelines.workbench.target.title")} hint={t("pipelines.workbench.target.hint")}>
        <Field id="workbench-target-pick" label={t("pipelines.field.targetEndpoint")} description={t("pipelines.field.targetEndpointHint")}>
          <Select
            id="workbench-target-pick"
            data-field="targetEndpoint"
            value={draft?.targetEndpoint ?? ""}
            onChange={(event) => onChange({ ...(draft ?? {}), targetEndpoint: event.target.value || undefined })}
          >
            <option value="">{t("form.choose")}</option>
            {targets
              .filter((candidate) => candidate.urn)
              .map((candidate) => (
                <option key={candidate.urn} value={candidate.urn}>
                  {candidate.name}
                </option>
              ))}
          </Select>
        </Field>
        {targetSpace ? (
          <p className="text-caption text-fg-muted">
            {targetModel
              ? t("pipelines.workbench.target.lands", { space: targetSpace, model: targetModel.metadata.name })
              : t("pipelines.workbench.target.landsUnmodelled", { space: targetSpace })}
          </p>
        ) : null}
        {targetSpace && (targetModel || target.model) ? <TargetCard
            space={targetSpace}
            type={writtenType}
            target={target}
            history={draft?.history}
            onHistory={(history) => onChange({ ...(draft ?? {}), history })}
            timeSlots={timeSlotsOf(target.attributes.filter((a) => a.format === "date-time").map((a) => a.name), records)}
            historyFindings={historyFindings}
          /> : null}
      </Step>

      <Step number={4} id="workbench-mapping" title={t("pipelines.workbench.mapping.title")} hint={t("pipelines.workbench.mapping.hint")}>
        {fileShown ? null : (
        <RadioGroup
          name="workbench-mapping-mode"
          legend={t("pipelines.mapper.mode")}
          value={mappingMode}
          layout="row"
          options={[
            { value: "map", label: t("pipelines.mapper.modeMap"), description: t("pipelines.mapper.modeMapHint") },
            { value: "code", label: t("pipelines.mapper.modeCode"), description: t("pipelines.mapper.modeCodeHint") },
          ]}
          onChange={setMappingMode}
        />
        )}
        {mappingMode === "map" && !fileShown ? (
          targetSpace ? (
            <OutputMapper
              project={project}
              space={targetSpace}
              orgDomain={draft?.targetEndpoint?.split(":")[3] ?? ""}
              records={sampled.data?.records ?? []}
              bloblang={bloblang}
              onBloblang={(next) =>
                onChange({ ...(draft ?? {}), compute: { ...(draft?.compute ?? {}), kind: "bloblang", bloblang: next } })
              }
              expiry={draft?.expiry}
              lifetime={draft?.lifetime}
              onEnds={({ expiry, lifetime, bloblang: next }) =>
                onChange({
                  ...(draft ?? {}),
                  expiry,
                  lifetime,
                  ...(next === undefined ? {} : { compute: { ...(draft?.compute ?? {}), kind: "bloblang", bloblang: next } }),
                })
              }
            />
          ) : (
            <p className="text-caption text-fg-subtle">{t("pipelines.workbench.mapping.pickTarget")}</p>
          )
        ) : (
        <div className="grid grid-cols-1 gap-3 lg:grid-cols-[minmax(0,3fr)_minmax(0,2fr)]">
          {fileShown ? (
            <div className="flex flex-col gap-2" data-testid="pipeline-mapping-file">
              {mappingFile.state === "failed" ? mappingFile.failure : null}
              <Field id="workbench-bento" label={t("pipelines.workbench.mapping.file")} description={t("pipelines.workbench.mapping.fileHint")}>
                <Textarea
                  id="workbench-bento"
                  rows={18}
                  spellCheck={false}
                  className="font-mono"
                  readOnly={!mappingFile.editing}
                  aria-busy={mappingFile.state === "loading"}
                  value={mappingFile.state === "loading" ? t("app.loading") : mappingFile.text}
                  onChange={(event) => mappingFile.onText(event.target.value)}
                />
              </Field>
              <Checkbox
                label={t("pipelines.workbench.mapping.fileEdit")}
                checked={mappingFile.editing}
                disabled={mappingFile.state !== "loaded"}
                disabledReason={t("pipelines.workbench.mapping.fileEditDisabled")}
                onChange={(event) => mappingFile.onEditing(event.target.checked)}
              />
            </div>
          ) : (
          <Field id="workbench-bloblang" label={t("pipelines.field.bloblang")} description={t("pipelines.workbench.mapping.bloblangHint")}>
            <Textarea
              id="workbench-bloblang"
              data-field="compute.bloblang"
              rows={14}
              spellCheck={false}
              className="font-mono"
              value={bloblang}
              onChange={(event) =>
                onChange({ ...(draft ?? {}), compute: { ...(draft?.compute ?? {}), kind: "bloblang", bloblang: event.target.value } })
              }
            />
          </Field>
          )}
          <div aria-labelledby="workbench-slots-title" role="region" className="flex flex-col gap-2">
            <FormHeading sub id="workbench-slots-title" className="text-caption font-semibold text-fg">
              {writtenClass
                ? t("pipelines.workbench.mapping.slotsOf", { type: writtenClass })
                : t("pipelines.workbench.mapping.slots")}
            </FormHeading>
            {classNames.length > 1 ? (
              <Field id="workbench-slots-type" label={t("pipelines.workbench.mapping.type")} description={t("pipelines.workbench.mapping.typeHint")}>
                <Select id="workbench-slots-type" value={writtenClass ?? ""} onChange={(event) => setPickedClass(event.target.value || undefined)}>
                  {writtenClass ? null : <option value="">{t("form.choose")}</option>}
                  {classNames.map((name) => (
                    <option key={name} value={name}>
                      {name}
                    </option>
                  ))}
                </Select>
              </Field>
            ) : null}
            {slots.length === 0 ? (
              <p className="text-caption text-fg-subtle">
                {draft?.targetEndpoint ? t("pipelines.workbench.mapping.noModel") : t("pipelines.workbench.mapping.pickTarget")}
              </p>
            ) : (
              // A list that scrolls is a stop of its own, so a keyboard scrolls it too (WCAG 2.1.1, T-3144).
              <ul
                tabIndex={0}
                aria-label={writtenClass ? t("pipelines.workbench.mapping.slotsOf", { type: writtenClass }) : t("pipelines.workbench.mapping.slots")}
                className="focus-ring flex max-h-80 flex-col gap-2 overflow-y-auto"
              >
                {slots.map((slot) => {
                  const values = parsed?.enums.find((e) => e.name === slot.range)?.permissible_values.map((v) => v.name) ?? [];
                  const unit = unitCode(slot.unit);
                  return (
                    <li key={slot.name} className="rounded border border-border p-2 text-caption">
                      <div className="flex flex-wrap items-center gap-1.5">
                        <span className="font-mono font-medium text-fg">{slot.name}</span>
                        <Badge mono>{values.length > 0 ? "enum" : (slot.range ?? "string")}</Badge>
                        {slot.kind !== "Property" ? <Badge>{slot.kind}</Badge> : null}
                        {slot.required ? <Badge tone="warning">{t("pipelines.workbench.mapping.required")}</Badge> : null}
                        {slot.multivalued ? <Badge>{t("pipelines.workbench.mapping.many")}</Badge> : null}
                      </div>
                      {slot.description ? <p className="text-fg-muted">{slot.description}</p> : null}
                      {values.length > 0 ? (
                        <p className="text-fg-muted">
                          {t("pipelines.workbench.mapping.values", {
                            values: values.slice(0, ENUM_SHOWN).join(", "),
                            count: Math.max(values.length - ENUM_SHOWN, 0),
                          })}
                        </p>
                      ) : null}
                      {(() => {
                        const line = `root.${slot.name} = { "type": "${slot.kind}", "${slot.kind === "Relationship" ? "object" : "value"}": ${exampleOf(slot, values[0])}${unit && slot.kind === "Property" ? `, "unitCode": "${unit}"` : ""} }`;
                        return (
                          <div className="flex flex-wrap items-start gap-2">
                            {/* The line is code the person adds to the mapping; the label is words (T-3851). */}
                            <p className="min-w-0 flex-1 text-fg-subtle [overflow-wrap:anywhere]">
                              {t("pipelines.workbench.mapping.example")} <code className="font-mono">{line}</code>
                            </p>
                            {/* A line added would start a compute step beside the file; the file is edited in place. */}
                            {fileShown ? null : (
                              <Button size="xs" variant="secondary" onClick={() => addLine(line)} aria-label={t("pipelines.workbench.mapping.addLineOf", { slot: slot.name })}>
                                {t("pipelines.workbench.mapping.addLine")}
                              </Button>
                            )}
                          </div>
                        );
                      })()}
                      {unit && slot.kind === "Property" && !fileShown ? (
                        <SlotUnit
                          slot={slot.name}
                          code={unit}
                          onAdd={addLine}
                        />
                      ) : null}
                    </li>
                  );
                })}
              </ul>
            )}
          </div>
        </div>
        )}
      </Step>

      <Step number={5} id="workbench-output" title={t("pipelines.workbench.output.title")} hint={t("pipelines.workbench.output.hint")}>
        {!tried.isFetched && !tried.isFetching ? (
          <p className="text-caption text-fg-subtle">{t("pipelines.workbench.output.waiting")}</p>
        ) : tried.isFetching ? (
          <p role="status" className="text-caption text-fg-subtle">
            {t("pipelines.workbench.running")}
          </p>
        ) : tried.isError ? (
          <Alert role="alert" tone="danger">
            {failedText(tried.error)}
          </Alert>
        ) : (
          <>
            {(tried.data?.errors ?? []).map((error, index) => (
              <Alert key={index} role="alert" tone="danger">
                {[
                  error.step != null ? t("pipelines.rejected.step", { step: error.step + 1 }) : null,
                  error.line != null ? t("pipelines.workbench.output.line", { line: error.line }) : null,
                  error.message,
                ]
                  .filter(Boolean)
                  .join(" · ")}
              </Alert>
            ))}
            <p className="text-caption text-fg-subtle">{t("pipelines.workbench.output.count", { count: records.length })}</p>
            {records.length > 0 ? (
              <div className="flex max-h-96 flex-col gap-2 overflow-y-auto" role="region" tabIndex={0} aria-label={t("pipelines.workbench.output.caption")}>
                {records.map((record, index) => {
                  const verdict = verdictOf(index);
                  return (
                    <RecordTable
                      key={index}
                      index={index}
                      record={record}
                      ask={access.ask}
                      verdict={
                        verdict
                          ? {
                              ok: verdict.ok,
                              findings:
                                verdict.findings ??
                                verdict.problems.map((problem) => ({ source: "model", rule: problem.rule, field: problem.path, reason: problem.message })),
                              ...(verdict.expires ? { expires: verdict.expires } : {}),
                            }
                          : undefined
                      }
                    />
                  );
                })}
              </div>
            ) : null}
            {access.form}
          </>
        )}
      </Step>

      <Step number={6} id="workbench-lifetime" title={t("pipelines.workbench.lifetime.title")} hint={t("pipelines.workbench.lifetime.hint")}>
        {targetSpace && writtenType ? (
          <LifetimeChoice
            project={project}
            space={targetSpace}
            type={writtenType}
            timeSlots={target.attributes.filter((a) => a.format === "date-time").map((a) => a.name)}
            attributes={grantedNames(target)}
            entry={draft?.lifetime?.find((one) => one.type === writtenType)}
            expiry={draft?.expiry}
            sweep
            onChange={(entry, expiry) => onChange({ ...(draft ?? {}), lifetime: withEntry(draft?.lifetime, writtenType, entry), expiry })}
          />
        ) : (
          <p className="text-caption text-fg-subtle">{t("pipelines.workbench.lifetime.pickType")}</p>
        )}
      </Step>

      <Step number={7} id="workbench-validation" title={t("pipelines.workbench.validation.title")} hint={t("pipelines.workbench.validation.hint")}>
        {!draft?.targetEndpoint ? (
          <p className="text-caption text-fg-subtle">{t("pipelines.workbench.mapping.pickTarget")}</p>
        ) : records.length === 0 ? (
          <p className="text-caption text-fg-subtle">{t("pipelines.workbench.validation.waiting")}</p>
        ) : validated.isFetching ? (
          <p role="status" className="text-caption text-fg-subtle">
            {t("pipelines.workbench.running")}
          </p>
        ) : noModel ? (
          <Alert role="status" tone="info">
            {failedText(validated.error)}
          </Alert>
        ) : validated.isError ? (
          <Alert role="alert" tone="danger">
            {failedText(validated.error)}
          </Alert>
        ) : validated.data ? (
          <Alert role="status" tone={validated.data.rejected === 0 ? "success" : "warning"}>
            {validated.data.rejected === 0
              ? t("pipelines.workbench.validation.allValid", { count: validated.data.valid, model: validated.data.model, version: validated.data.version })
              : t("pipelines.workbench.validation.someRejected", {
                  count: validated.data.rejected,
                  valid: validated.data.valid,
                  model: validated.data.model,
                  version: validated.data.version,
                })}
          </Alert>
        ) : null}
        {draft?.targetEndpoint && targetSpace && records.length > 0 ? (
          <SpaceChanges key={targetSpace} space={targetSpace} records={records} />
        ) : null}
      </Step>
      <p className="text-caption text-fg-subtle">{t("pipelines.workbench.target.save")}</p>


      {children}
    </div>
  );
}
