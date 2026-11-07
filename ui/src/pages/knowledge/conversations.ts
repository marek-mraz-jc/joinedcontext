// The stored conversations of a project's assistants (API/01 §34, API/05 §3.1, AG-99, T-4670):
// the list, one conversation with its steps, the erasure, and a turn as an eval case.
import { api, unwrap } from "../../api/client";

export interface ConversationRow {
  id: string;
  deployment: string;
  origin: string | null;
  startedAt: string;
  lastAt: string;
  firstQuestion: string;
  turns: number;
  routes: string[];
  totalMs: number;
  errors: number;
  timeouts: number;
  costUsd: number;
  uncosted: number;
  thumbsUp: number;
  thumbsDown: number;
  /** T-4728: the turns' prompt and completion tokens, the cached ones (null when none reported). */
  tokensIn?: number;
  tokensOut?: number;
  cachedTokens?: number | null;
  /** The first turn's time to its first token; null when none came. */
  firstTokenMs?: number | null;
  models?: string[];
  /** What the newest failed turn told the visitor; null when every turn answered (T-4994). */
  lastError?: string | null;
}

export interface ConversationStep {
  kind: "route" | "search" | "tool" | "model" | "answer" | "visual" | "untracked";
  name: string;
  endpoint: string | null;
  input: string | null;
  output: string | null;
  outputBytes: number | null;
  status: "ok" | "failed";
  startMs: number;
  durationMs: number;
  error: string | null;
  stage: string | null;
  model: string | null;
  provider: string | null;
  tokensIn: number | null;
  tokensOut: number | null;
  reasoningTokens: number | null;
  costUsd: number | null;
  /** T-4681: prompt tokens the provider read from its cache. */
  cachedTokens?: number | null;
  /** The model's reasoning text, up to 64 KB; never shown to visitors. */
  reasoning?: string | null;
  finishReason?: string | null;
  effort?: string | null;
  /** The first 12 hex of the system prompt's sha256: which prompt version answered. */
  promptVersion?: string | null;
  /** A tool's or a visual's row count. */
  rows?: number | null;
}

export interface ConversationTurn {
  turn: number;
  at: string;
  question: string;
  answer: string;
  route: string;
  firstTokenMs: number | null;
  totalMs: number;
  error: { status?: number; title?: string; detail?: string } | null;
  timedOut: boolean;
  costUsd: number;
  uncosted: number;
  verdict: string | null;
  steps: ConversationStep[];
}

export interface Conversation {
  id: string;
  deployment: string;
  origin: string | null;
  turns: ConversationTurn[];
}

export interface ConversationFilter {
  deployment?: string;
  /** A day (YYYY-MM-DD), from its start; `to` up to the end of its day. */
  from?: string;
  to?: string;
  status?: "error" | "timeout" | "ok";
  feedback?: "up" | "down" | "none";
  slow?: boolean;
  /** Only conversations that cost more than $0.02. */
  expensive?: boolean;
  q?: string;
  sort?: "lastAt" | "costUsd" | "tokens" | "totalMs";
  cursor?: string;
}

/** A stored turn with the same normalized question (API/05 §3.1, T-4681). */
export interface SameTurn {
  conversation: string;
  turn: number;
  at: string;
  deployment: string;
  totalMs: number;
  costUsd: number | null;
  steps: number;
}

export const conversationKeys = {
  usage: (project: string, filter: ConversationFilter) =>
    ["projects", project, "knowledge", "conversations", "usage", rangeQuery(filter)] as const,
  list: (project: string, filter: ConversationFilter) => ["projects", project, "knowledge", "conversations", filter] as const,
  one: (project: string, id: string) => ["projects", project, "knowledge", "conversations", id] as const,
  same: (project: string, id: string, turn: number) =>
    ["projects", project, "knowledge", "conversations", id, "same", turn] as const,
};

/** What the route takes (API/05 §3.1). */
export interface ConversationQuery {
  deployment?: string;
  from?: string;
  to?: string;
  status?: string;
  feedback?: string;
  slow?: boolean;
  minCostUsd?: number;
  q?: string;
  sort?: string;
  cursor?: string;
}

const DAY = /^\d{4}-\d{2}-\d{2}$/;

/** The day after `day` (YYYY-MM-DD), so `to` holds the whole of its own day. */
function dayAfter(day: string): string {
  const next = new Date(`${day}T00:00:00Z`);
  next.setUTCDate(next.getUTCDate() + 1);
  return next.toISOString().slice(0, 10);
}

/** The filter as the route takes it: empty values left out, `q` trimmed, days as UTC times. */
export function filterQuery(filter: ConversationFilter): ConversationQuery {
  const query: ConversationQuery = {};
  const deployment = filter.deployment?.trim();
  if (deployment) query.deployment = deployment;
  if (filter.from && DAY.test(filter.from)) query.from = `${filter.from}T00:00:00Z`;
  if (filter.to && DAY.test(filter.to)) query.to = `${dayAfter(filter.to)}T00:00:00Z`;
  if (filter.expensive) query.minCostUsd = 0.02;
  if (filter.status) query.status = filter.status;
  if (filter.feedback) query.feedback = filter.feedback;
  if (filter.slow) query.slow = true;
  const q = filter.q?.trim();
  if (q && q.length >= 2) query.q = q;
  if (filter.sort && filter.sort !== "lastAt") query.sort = filter.sort;
  if (filter.cursor) query.cursor = filter.cursor;
  return query;
}

export async function listConversations(
  project: string,
  filter: ConversationFilter,
): Promise<{ items: ConversationRow[]; nextCursor: string | null }> {
  const answer = (await unwrap(
    await api.GET("/api/v1/projects/{project}/knowledge/conversations", {
      params: { path: { project }, query: filterQuery(filter) },
    }),
  )) as { items?: ConversationRow[]; nextCursor?: string | null } | null;
  return { items: Array.isArray(answer?.items) ? answer.items : [], nextCursor: answer?.nextCursor ?? null };
}

/** p50, p90 and max of a time; null with no turn. */
export interface Spread {
  p50: number | null;
  p90: number | null;
  max: number | null;
}

/** What the stored turns of a range add up to (API/05 §3.1, T-4728). */
export interface UsageSummary {
  questions: number;
  timeouts: number;
  errors: number;
  /** Turns with a call the provider sent no price for: counted apart, never as $0. */
  uncostedQuestions: number;
  costUsd: { total: number | null; mean: number | null; p50: number | null; p90: number | null };
  tokens: { in: number | null; out: number | null; cached: number | null; cachedShare: number | null };
  /** Turns answered from the kept answers, and the estimated time and cost they saved (T-4731). */
  answerCache?: { hits: number; hitShare: number | null; savedMs: number | null; savedUsd: number | null };
  firstTokenMs: Spread;
  totalMs: Spread;
  phases: { route: Spread; work: Spread; answer: Spread };
  costByStage: { stage: string; calls: number; costUsd: number | null; uncostedCalls: number }[];
  days: {
    day: string;
    questions: number;
    costUsd: number | null;
    uncostedQuestions: number;
    p50TotalMs: number | null;
    timeouts: number;
    errors: number;
  }[];
}

/** The range of the summary and the export: the list's deployment and days. */
export function rangeQuery(filter: ConversationFilter): Pick<ConversationQuery, "deployment" | "from" | "to"> {
  const { deployment, from, to } = filterQuery(filter);
  return { ...(deployment ? { deployment } : {}), ...(from ? { from } : {}), ...(to ? { to } : {}) };
}

/** Whether `answer` has every part of a summary the card reads, so a body of another shape (an
 * older assistant, a proxy's page) is a failure the card says, never a page that breaks. */
function isUsage(answer: unknown): answer is UsageSummary {
  const a = answer as Partial<UsageSummary> | null;
  const spread = (s: unknown) => typeof s === "object" && s !== null;
  return (
    typeof a === "object" &&
    a !== null &&
    typeof a.questions === "number" &&
    spread(a.costUsd) &&
    spread(a.tokens) &&
    spread(a.firstTokenMs) &&
    spread(a.totalMs) &&
    spread(a.phases) &&
    spread(a.phases?.route) &&
    spread(a.phases?.work) &&
    spread(a.phases?.answer) &&
    Array.isArray(a.costByStage) &&
    Array.isArray(a.days)
  );
}

export async function conversationUsage(project: string, filter: ConversationFilter): Promise<UsageSummary> {
  const answer: unknown = await unwrap(
    await api.GET("/api/v1/projects/{project}/knowledge/conversations/usage", {
      params: { path: { project }, query: rangeQuery(filter) },
    }),
  );
  if (!isUsage(answer)) throw new Error("the knowledge assistant answered no usage summary");
  return answer;
}

/** The range's turns as the CSV the assistant wrote; a refusal (over 20,000 turns) throws its reason. */
export async function exportConversations(project: string, filter: ConversationFilter): Promise<string> {
  const csv: unknown = await unwrap(
    await api.GET("/api/v1/projects/{project}/knowledge/conversations/export", {
      params: { path: { project }, query: rangeQuery(filter) },
      parseAs: "text",
    }),
  );
  return typeof csv === "string" ? csv : "";
}

/** A cost a person can trust: "cena neznáma" when a call was unpriced and nothing priced, never $0. */
export function costText(usd: number | null | undefined, uncosted: number, unknown: string): string {
  if (uncosted > 0 && !usd) return unknown;
  return usdText(usd);
}

/** A count of tokens: 58 722, 1,2 k and so on, by the reader's locale; "–" when missing. */
export function tokensText(n: number | null | undefined, locale: string): string {
  if (n === null || n === undefined || !Number.isFinite(n)) return "–";
  return new Intl.NumberFormat(locale, { maximumFractionDigits: 0 }).format(n);
}

/** A share as a percent: 20 %; "–" when there is none. */
export function shareText(share: number | null | undefined, locale: string): string {
  if (share === null || share === undefined || !Number.isFinite(share)) return "–";
  return new Intl.NumberFormat(locale, { style: "percent", maximumFractionDigits: 0 }).format(share);
}

export async function getConversation(project: string, conversation: string): Promise<Conversation> {
  return (await unwrap(
    await api.GET("/api/v1/projects/{project}/knowledge/conversations/{conversation}", {
      params: { path: { project, conversation } },
    }),
  )) as Conversation;
}

export async function eraseConversation(project: string, conversation: string): Promise<void> {
  await unwrap(
    await api.DELETE("/api/v1/projects/{project}/knowledge/conversations/{conversation}", {
      params: { path: { project, conversation } },
    }),
  );
}

/** Makes one assistant forget the answers it keeps for repeated questions; how many it forgot (T-4731). */
export async function forgetAnswers(project: string, deployment: string): Promise<number> {
  const answer = (await unwrap(
    await api.DELETE("/api/v1/projects/{project}/knowledge/deployments/{deployment}/answers", {
      params: { path: { project, deployment } },
    }),
  )) as { forgotten?: unknown };
  return typeof answer.forgotten === "number" ? answer.forgotten : 0;
}

export async function sameQuestion(project: string, conversation: string, turn: number): Promise<SameTurn[]> {
  const answer = (await unwrap(
    await api.GET("/api/v1/projects/{project}/knowledge/conversations/{conversation}/turns/{turn}/same", {
      params: { path: { project, conversation, turn } },
    }),
  )) as { items?: SameTurn[] } | null;
  return Array.isArray(answer?.items) ? answer.items : [];
}

/**
 * A turn as a case of the assistant's eval set (conformance tests/assistant/opendata_questions.json,
 * T-4656): the question, the kind its route suggests and an empty `must` the person fills in; the
 * id is a placeholder to number.
 */
export function evalCase(turn: Pick<ConversationTurn, "question" | "route">, lang: string): string {
  const kind = turn.route === "data" ? "value" : turn.route === "declined" ? "scope" : "catalogue";
  const language = ["sk", "en", "cs"].includes(lang) ? lang : "sk";
  return JSON.stringify({ id: "od-00", lang: language, kind, q: turn.question, must: [] }, null, 2);
}

/** Milliseconds as a person reads them: 850 ms, 2.4 s. */
export function msText(ms: number | null | undefined): string {
  if (ms === null || ms === undefined || !Number.isFinite(ms)) return "–";
  return ms < 1000 ? `${Math.round(ms)} ms` : `${(ms / 1000).toFixed(1)} s`;
}

/** Dollars of model calls: $0.0046; "–" when nothing was priced. */
export function usdText(usd: number | null | undefined): string {
  if (usd === null || usd === undefined || !Number.isFinite(usd)) return "–";
  return `$${usd < 0.01 ? usd.toFixed(4) : usd.toFixed(2)}`;
}

/** The most text one step shows before "Zobraziť viac" (the store keeps 64 KB, T-4681). */
export const SHOWN_CHARS = 64 * 1024;

// Bidi overrides, isolates and marks, and the zero-width characters: they reorder or hide text.
const INVISIBLE = /[\u061c\u200b-\u200f\u202a-\u202e\u2060-\u2069\ufeff]/gu;

/** Stored text with every bidi and zero-width character shown as a visible marker such as ⟦U+202E⟧. */
export function visible(text: string): string {
  return text.replace(INVISIBLE, (c) => `⟦U+${c.codePointAt(0)?.toString(16).toUpperCase().padStart(4, "0")}⟧`);
}

/** The text up to `limit` characters, and whether more was cut. */
export function capped(text: string, limit = SHOWN_CHARS): { text: string; more: boolean } {
  return text.length > limit ? { text: text.slice(0, limit), more: true } : { text, more: false };
}

export interface TurnTotals {
  steps: number;
  tools: number;
  models: number;
  totalMs: number;
  costUsd: number;
  /** Cached prompt tokens over all prompt tokens of the model calls; null when none were counted. */
  cachedShare: number | null;
}

/** The header line of a compared turn: steps, tool calls, model calls, time, cost and cached share. */
export function totals(turn: Pick<ConversationTurn, "steps" | "totalMs" | "costUsd">): TurnTotals {
  const models = turn.steps.filter((s) => s.kind === "model");
  const prompt = models.reduce((sum, s) => sum + (s.tokensIn ?? 0), 0);
  const cached = models.reduce((sum, s) => sum + (s.cachedTokens ?? 0), 0);
  return {
    steps: turn.steps.length,
    tools: turn.steps.filter((s) => s.kind === "tool").length,
    models: models.length,
    totalMs: turn.totalMs,
    costUsd: turn.costUsd,
    cachedShare: prompt > 0 ? cached / prompt : null,
  };
}

/** The longest common subsequence of `a` and `b` under `same`, as index pairs in order. */
function lcs<T>(a: T[], b: T[], same: (x: T, y: T) => boolean): [number, number][] {
  const n = a.length;
  const m = b.length;
  const table = Array.from({ length: n + 1 }, () => new Array<number>(m + 1).fill(0));
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) {
      table[i][j] = same(a[i], b[j]) ? table[i + 1][j + 1] + 1 : Math.max(table[i + 1][j], table[i][j + 1]);
    }
  }
  const pairs: [number, number][] = [];
  let i = 0;
  let j = 0;
  while (i < n && j < m) {
    if (same(a[i], b[j])) {
      pairs.push([i, j]);
      i++;
      j++;
    } else if (table[i + 1][j] >= table[i][j + 1]) i++;
    else j++;
  }
  return pairs;
}

/** Two step timelines aligned by kind: matched steps share a row, the others face an empty cell. */
export function aligned(
  a: ConversationStep[],
  b: ConversationStep[],
): [ConversationStep | null, ConversationStep | null][] {
  const rows: [ConversationStep | null, ConversationStep | null][] = [];
  let i = 0;
  let j = 0;
  for (const [x, y] of [...lcs(a, b, (s, t) => s.kind === t.kind), [a.length, b.length]]) {
    while (i < x) rows.push([a[i++], null]);
    while (j < y) rows.push([null, b[j++]]);
    if (x < a.length) rows.push([a[i++], b[j++]]);
  }
  return rows;
}

export interface DiffWord {
  word: string;
  change: "same" | "removed" | "added";
}

// ponytail: the word LCS is O(n·m); past this many words each answer shows as wholly changed.
const DIFF_WORDS = 2000;

/** A word diff of two answers: the words of `a` that `b` dropped, the ones it added, the rest kept. */
export function wordDiff(a: string, b: string): DiffWord[] {
  const left = a.split(/\s+/u).filter(Boolean);
  const right = b.split(/\s+/u).filter(Boolean);
  const pairs = left.length > DIFF_WORDS || right.length > DIFF_WORDS ? [] : lcs(left, right, (x, y) => x === y);
  const out: DiffWord[] = [];
  let i = 0;
  let j = 0;
  for (const [x, y] of [...pairs, [left.length, right.length]]) {
    while (i < x) out.push({ word: left[i++], change: "removed" });
    while (j < y) out.push({ word: right[j++], change: "added" });
    if (x < left.length) {
      out.push({ word: left[i++], change: "same" });
      j++;
    }
  }
  return out;
}

/** What a visual step showed: table, chart or map, the chart type, its units and its row count. */
export interface VisualSummary {
  kind: string;
  mark: string | null;
  units: string | null;
  rows: number | null;
}

export function visualSummary(step: ConversationStep): VisualSummary {
  let spec: Record<string, unknown> = {};
  try {
    const parsed: unknown = JSON.parse(step.input ?? "{}");
    if (parsed !== null && typeof parsed === "object" && !Array.isArray(parsed)) spec = parsed as Record<string, unknown>;
  } catch {
    // A stored spec cut at 64 KB no longer parses; the kind and row count still compare.
  }
  const mark = spec.mark;
  const markType =
    typeof mark === "string" ? mark
    : mark !== null && typeof mark === "object" && typeof (mark as { type?: unknown }).type === "string"
      ? (mark as { type: string }).type
      : null;
  const units = spec.units === undefined || spec.units === null ? null : JSON.stringify(spec.units);
  return { kind: step.name, mark: markType, units, rows: step.rows ?? null };
}

/** The spec changes between the visuals of two turns, in order: "chart → map", "units: … → none". */
export function visualChanges(a: ConversationStep[], b: ConversationStep[]): string[] {
  const left = a.filter((s) => s.kind === "visual").map(visualSummary);
  const right = b.filter((s) => s.kind === "visual").map(visualSummary);
  const changes: string[] = [];
  for (let i = 0; i < Math.max(left.length, right.length); i++) {
    const x = left[i];
    const y = right[i];
    if (!x || !y) {
      changes.push(`${x?.kind ?? "–"} → ${y?.kind ?? "–"}`);
      continue;
    }
    for (const key of ["kind", "mark", "units", "rows"] as const) {
      if (x[key] !== y[key]) changes.push(`${key}: ${x[key] ?? "–"} → ${y[key] ?? "–"}`);
    }
  }
  return changes;
}
