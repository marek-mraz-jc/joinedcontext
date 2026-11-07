/**
 * The KPI editor's model (PL-102…PL-105, T-4935): two to five sources, how their rows meet, and a
 * formula picked from plain choices or typed under Advanced, written out as the
 * `joinedcontext.com/v1alpha2` Pipeline jc-core admits. The server checks the draft
 * (`POST …/kpis/check`); this module only writes it and places each refusal beside its field.
 */
import type { Manifest } from "../../api/manifest";

export const MIN_SOURCES = 2;
export const MAX_SOURCES = 5;
/** The rows of each source the preview reads; the run reads them all (PL-31). */
export const PREVIEW_PAGE = 100;

export type JoinBy = "time" | "area" | "entity";

/**
 * The formula choices (PL-105). `per` is the first source's sum per row of the second, `rest` the
 * share left: spaces in use from free spaces and all spaces, `change` the first source's sum now
 * minus the same sum a week ago, read from its history (PL-117).
 */
export type FormulaChoice = "count" | "sum" | "average" | "share" | "rest" | "per" | "change";
export const FORMULA_CHOICES: FormulaChoice[] = ["count", "sum", "average", "share", "rest", "per", "change"];

/** How often the indicator is computed, as the cron the schedule takes. */
export const SCHEDULES = { quarter: "*/15 * * * *", hourly: "0 * * * *", daily: "0 3 * * *" } as const;
export type ScheduleChoice = keyof typeof SCHEDULES;

export interface KpiSourceDraft {
  /** The input's name in the formula, a DNS label. */
  name: string;
  /** The source Endpoint of this project. */
  endpoint: string;
  type: string;
  /** The attribute the formula reads on this source (its numeric value). */
  attribute: string;
  /** The attribute naming the area or entity a row is about; `id` for the entity itself. */
  key: string;
}

export interface KpiDraft {
  name: string;
  title: string;
  sources: KpiSourceDraft[];
  join: JoinBy;
  choice: FormulaChoice;
  /** Advanced: the formula as typed, in place of the choice. */
  advanced: boolean;
  typed: string;
  unitCode: string;
  schedule: ScheduleChoice;
  /** The URN of the `-kpi` Endpoint the indicator is written through. */
  target: string;
}

export function emptySource(at: number): KpiSourceDraft {
  return { name: `source-${at + 1}`, endpoint: "", type: "", attribute: "", key: "" };
}

export function emptyDraft(target = ""): KpiDraft {
  return {
    name: "",
    title: "",
    sources: [emptySource(0), emptySource(1)],
    join: "time",
    choice: "count",
    advanced: false,
    typed: "",
    unitCode: "C62",
    schedule: "hourly",
    target,
  };
}

/** The formula the choice writes over the first two sources. */
export function choiceFormula(choice: FormulaChoice, sources: KpiSourceDraft[]): string {
  const [a, b] = sources;
  const of = (s: KpiSourceDraft | undefined) => `${s?.name ?? ""}.${s?.attribute ?? ""}`;
  switch (choice) {
    case "count":
      return `count(${a?.name ?? ""})`;
    case "sum":
      return `sum(${of(a)})`;
    case "average":
      return `avg(${of(a)})`;
    case "share":
      return `sum(${of(a)}) / sum(${of(b)})`;
    case "rest":
      return `1 - sum(${of(a)}) / sum(${of(b)})`;
    case "per":
      return `sum(${of(a)}) / count(${b?.name ?? ""})`;
    case "change":
      return `sum(${of(a)}) - prev(sum(${of(a)}), P7D)`;
  }
}

/** The fewest sources the draft's formula takes: a `prev` term is a second read of one source. */
export function leastSources(draft: KpiDraft): number {
  return /\bprev\s*\(/.test(formulaOf(draft)) ? 1 : MIN_SOURCES;
}

/**
 * The draft after picking `choice`: a change reads one source over all its rows, so it drops the
 * sources it does not read and the join; any other choice gets its second source back.
 */
export function withChoice(draft: KpiDraft, choice: FormulaChoice): KpiDraft {
  const sources =
    choice === "change"
      ? draft.sources.slice(0, 1)
      : draft.sources.length < MIN_SOURCES
        ? [...draft.sources, emptySource(draft.sources.length)]
        : draft.sources;
  return { ...draft, choice, sources, join: choice === "change" ? "time" : draft.join };
}

export function formulaOf(draft: KpiDraft): string {
  return draft.advanced ? draft.typed.trim() : choiceFormula(draft.choice, draft.sources);
}

/** The attributes a source reads: those the formula names on it, and its join key. */
function attrsOf(source: KpiSourceDraft, formula: string, join: JoinBy): string[] {
  const named = [...formula.matchAll(/\b([a-z0-9-]+)\.([A-Za-z_][A-Za-z0-9_]*)/g)]
    .filter((match) => match[1] === source.name)
    .map((match) => match[2]);
  const key = join !== "time" && source.key && source.key !== "id" ? [source.key] : [];
  return [...new Set([...named, ...key])];
}

/** The v1alpha2 Pipeline the draft writes (Architecture/08, "Indicators over several sources"). */
export function kpiManifest(project: string, draft: KpiDraft): Manifest {
  const formula = formulaOf(draft);
  const keys =
    draft.join === "time" ? undefined : Object.fromEntries(draft.sources.map((s) => [s.name, s.key]));
  return {
    apiVersion: "joinedcontext.com/v1alpha2",
    kind: "Pipeline",
    metadata: { name: draft.name, namespace: project },
    spec: {
      class: "scheduled",
      schedule: SCHEDULES[draft.schedule],
      sources: draft.sources.map((source) => ({
        name: source.name,
        endpointRef: { kind: "Endpoint", name: source.endpoint },
        query: { type: source.type, attrs: attrsOf(source, formula, draft.join) },
      })),
      steps: [
        {
          kind: "kpi",
          kpi: {
            name: draft.name,
            ...(draft.title.trim() ? { title: draft.title.trim() } : {}),
            formula,
            unitCode: draft.unitCode.trim(),
            join: keys ? { by: draft.join, keys } : { by: "time" },
          },
        },
      ],
      outputs: [{ targetEndpoint: draft.target, type: "KeyPerformanceIndicator" }],
    },
  } as Manifest;
}

/** Where the editor shows a refusal of the check route. */
export type ProblemPlace =
  | { at: "name" | "formula" | "unit" | "join" | "schedule" | "target" | "general" }
  | { at: "source"; index: number };

export function placeOf(field: string): ProblemPlace {
  const source = /^spec\.sources\[(\d+)\]/.exec(field);
  if (source) return { at: "source", index: Number(source[1]) };
  if (field.startsWith("spec.steps.kpi.formula")) return { at: "formula" };
  if (field.startsWith("spec.steps.kpi.unitCode")) return { at: "unit" };
  if (field.startsWith("spec.steps.kpi.join")) return { at: "join" };
  if (field.startsWith("spec.steps.kpi.name") || field === "metadata.name") return { at: "name" };
  if (field.startsWith("spec.schedule")) return { at: "schedule" };
  if (field.startsWith("spec.outputs")) return { at: "target" };
  return { at: "general" };
}

/** What the editor still lacks before a check is worth sending, as a key under `pipelines.kpiEditor.missing`. */
export function missingOf(draft: KpiDraft): string | undefined {
  if (!/^[a-z0-9]([a-z0-9-]{0,46}[a-z0-9])?$/.test(draft.name)) return "name";
  if (draft.sources.some((s) => !s.endpoint || !s.type)) return "source";
  if (!draft.target) return "target";
  return undefined;
}

/** The `-kpi` Endpoints of the project, as targets, by URN. */
export function kpiTargets(endpoints: Manifest[], spaceOf: (m: Manifest) => string | undefined, orgDomain?: string) {
  // A target URN names the organization's domain: none is offered before it is known.
  if (!orgDomain) return [];
  return endpoints
    .map((endpoint) => ({ endpoint, space: spaceOf(endpoint) }))
    .filter((one): one is { endpoint: Manifest; space: string } => !!one.space?.endsWith("-kpi"))
    .map(({ endpoint, space }) => ({
      name: endpoint.metadata.name,
      urn: `urn:ngsi-ld:Endpoint:${orgDomain}:${space}:${endpoint.metadata.name}`,
    }));
}
