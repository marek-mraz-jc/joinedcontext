// The knowledge assistant's administration routes (API/01 §34, T-3057): what a project's
// sources hold once crawled. The answers are jc-assistant's; the shapes below are API/05 §3.
import { api, ApiError, readCsrfToken, unwrap } from "../../api/client";
import type { ProblemDetails } from "../../api/client";
import type { Manifest } from "../../api/manifest";

export interface SourceJob {
  state: "queued" | "running" | "done" | "failed";
  attempts: number;
  error: string | null;
  /** When a worker took the job up, and when it ended (T-4197). */
  startedAt?: string | null;
  finishedAt?: string | null;
}

export interface KnowledgeSourceRow {
  source: string;
  type: "website" | "ckan" | "catalogue" | "guide" | "upload";
  state: "crawled" | "not-crawled";
  startUrls: string[];
  ckanInstanceRef: string | null;
  /** The spaces a `catalogue` source reads; empty is every space of the project (AG-116). */
  contextSpaces?: string[];
  schedule: string | null;
  visibility: "public" | "internal";
  lastCrawl?: string | null;
  pages?: number;
  pagesIncluded?: number;
  documents?: number;
  passages?: number;
  embedded?: number;
  /** Passages not searchable yet: stored, still waiting for their embedding (T-4197). */
  pending?: number;
  /** Pages and documents whose last read failed. */
  failed?: number;
  /** Its passages per language; passages of no known language are not counted (UI-99). */
  languages?: Record<string, number>;
  job?: SourceJob | null;
}

/** What a page or document states of itself (AG-120); `null` where it states nothing. */
export interface Stated {
  title?: string | null;
  description?: string | null;
  published?: string | null;
  modified?: string | null;
}

export interface PageRow extends Stated {
  id: number;
  url: string;
  depth: number;
  parentId: number | null;
  status: "pending" | "fetched" | "failed" | "skipped";
  included: boolean;
  excludedBy: "pattern" | "administrator" | null;
  language: string | null;
  fetchedAt: string | null;
  children: number;
  documents: number;
  passages: number;
}

export interface DocumentRow extends Stated {
  id: number;
  url: string;
  pageId: number | null;
  mime: string | null;
  bytes: number | null;
  pages: number | null;
  offDomain: boolean;
  status: PageRow["status"];
  included: boolean;
  excludedBy: PageRow["excludedBy"];
  passages: number;
}

/** One hit of the search `search_documents` runs (API/05 §4, UI-99). */
export interface SearchHit extends Stated {
  url: string;
  publisher: string;
  language: string | null;
  text: string;
  score: number;
}

/** The shortest page search, in characters (API/05 §3). */
export const PAGE_QUERY_MIN = 2;

export interface Passage {
  ordinal: number;
  text: string;
  lang: string | null;
  url: string;
}

export interface LinkRow {
  url: string;
  kind: "page" | "document" | "external";
}

/** A page or document whose last read failed, and why (API/05 §3, T-4197). */
export interface CrawlError {
  url: string;
  kind: "page" | "document";
  reason: string;
}

export interface InclusionChange {
  pages: number[];
  documents: number[];
  subtree: boolean;
  included: boolean;
}

export interface InclusionCounts {
  pages: number;
  documents: number;
  passagesRemoved: number;
}

interface Items<T> {
  items: T[];
}

/** The `items` of an answer whose schema the Portal declares as an open object. */
function items<T>(answer: unknown): T[] {
  const list = (answer as Partial<Items<T>> | null)?.items;
  return Array.isArray(list) ? list : [];
}

export const knowledgeKeys = {
  sources: (project: string) => ["projects", project, "knowledge", "sources"] as const,
  pages: (project: string, source: string, parent: number | null) =>
    ["projects", project, "knowledge", "sources", source, "pages", parent] as const,
  found: (project: string, source: string, q: string) =>
    ["projects", project, "knowledge", "sources", source, "pages", "q", q] as const,
  documents: (project: string, source: string) => ["projects", project, "knowledge", "sources", source, "documents"] as const,
  search: (project: string, source: string, q: string) =>
    ["projects", project, "knowledge", "sources", source, "search", q] as const,
  passages: (project: string, source: string, owner: { page?: number; document?: number }) =>
    ["projects", project, "knowledge", "sources", source, "passages", owner.page ?? null, owner.document ?? null] as const,
  errors: (project: string, source: string) => ["projects", project, "knowledge", "sources", source, "errors"] as const,
  files: (project: string, source: string) => ["projects", project, "knowledge", "sources", source, "files"] as const,
  links: (project: string, source: string, page: number) =>
    ["projects", project, "knowledge", "sources", source, "links", page] as const,
};

export async function listSources(project: string): Promise<KnowledgeSourceRow[]> {
  return items<KnowledgeSourceRow>(
    await unwrap(await api.GET("/api/v1/projects/{project}/knowledge/sources", { params: { path: { project } } })),
  );
}

export async function listPages(project: string, source: string, parent: number | null): Promise<PageRow[]> {
  return items<PageRow>(
    await unwrap(
      await api.GET("/api/v1/projects/{project}/knowledge/sources/{source}/pages", {
        params: { path: { project, source }, query: parent === null ? {} : { parent } },
      }),
    ),
  );
}

/** At most 100 pages of the whole source whose title or address holds `q` (UI-99). */
export async function findPages(project: string, source: string, q: string): Promise<PageRow[]> {
  return items<PageRow>(
    await unwrap(
      await api.GET("/api/v1/projects/{project}/knowledge/sources/{source}/pages", {
        params: { path: { project, source }, query: { q } },
      }),
    ),
  );
}

/** What `search_documents` would answer from this one source, best first (UI-99). */
export async function trySearch(project: string, source: string, q: string): Promise<SearchHit[]> {
  const answer = await unwrap(
    await api.GET("/api/v1/projects/{project}/knowledge/sources/{source}/search", {
      params: { path: { project, source }, query: { q, limit: 5 } },
    }),
  );
  const hits = (answer as { hits?: unknown } | null)?.hits;
  return Array.isArray(hits) ? (hits as SearchHit[]) : [];
}

/** A source's languages, the largest share first: `sk 92 %`, `en 8 %`. */
function languageShares(languages: Record<string, number> | undefined, language: string): string[] {
  const entries = Object.entries(languages ?? {}).filter(([, n]) => n > 0);
  const total = entries.reduce((sum, [, n]) => sum + n, 0);
  if (total === 0) return [];
  const percent = new Intl.NumberFormat(language, { style: "percent", maximumFractionDigits: 0 });
  return entries
    .sort(([a, x], [b, y]) => y - x || a.localeCompare(b))
    .map(([code, n]) => `${code} ${percent.format(n / total)}`);
}

/** The three largest languages and how many more there are, for a table cell (T-4994). */
export function topLanguages(
  languages: Record<string, number> | undefined,
  language: string,
): { top: string; more: number; all: string } {
  const shares = languageShares(languages, language);
  return { top: shares.slice(0, 3).join(", ") || "—", more: Math.max(0, shares.length - 3), all: shares.join(", ") };
}

export async function listDocuments(project: string, source: string): Promise<DocumentRow[]> {
  return items<DocumentRow>(
    await unwrap(
      await api.GET("/api/v1/projects/{project}/knowledge/sources/{source}/documents", {
        params: { path: { project, source } },
      }),
    ),
  );
}

export async function listPassages(
  project: string,
  source: string,
  owner: { page?: number; document?: number },
): Promise<Passage[]> {
  return items<Passage>(
    await unwrap(
      await api.GET("/api/v1/projects/{project}/knowledge/sources/{source}/passages", {
        params: { path: { project, source }, query: owner },
      }),
    ),
  );
}

export async function listLinks(project: string, source: string, page: number): Promise<LinkRow[]> {
  return items<LinkRow>(
    await unwrap(
      await api.GET("/api/v1/projects/{project}/knowledge/sources/{source}/pages/{page}/links", {
        params: { path: { project, source, page } },
      }),
    ),
  );
}

export async function setInclusion(project: string, source: string, change: InclusionChange): Promise<InclusionCounts> {
  const answer = await unwrap(
    await api.POST("/api/v1/projects/{project}/knowledge/sources/{source}/inclusion", {
      params: { path: { project, source } },
      body: change,
    }),
  );
  const counts = answer as Partial<InclusionCounts> | null;
  return { pages: counts?.pages ?? 0, documents: counts?.documents ?? 0, passagesRemoved: counts?.passagesRemoved ?? 0 };
}

export async function listErrors(project: string, source: string): Promise<CrawlError[]> {
  return items<CrawlError>(
    await unwrap(
      await api.GET("/api/v1/projects/{project}/knowledge/sources/{source}/errors", {
        params: { path: { project, source } },
      }),
    ),
  );
}

export async function recrawl(project: string, source: string): Promise<void> {
  await unwrap(
    await api.POST("/api/v1/projects/{project}/knowledge/sources/{source}/recrawl", {
      params: { path: { project, source } },
    }),
  );
}

/** A file an administrator uploaded to a `source: upload` (API/05 §3, T-4184). */
export interface UploadedFile {
  id: number;
  name: string;
  mime: string | null;
  bytes: number | null;
  passages: number;
  status: PageRow["status"];
}

/** The file types an upload source reads (API/05 §3): checked again by content on the server. */
export const UPLOAD_ACCEPT = ".pdf,.docx,.xlsx,.csv,.md,.txt";

/** The file and page an uploaded file's passage cites as `upload:{source}/{name}#page={n}`, else null. */
export function uploadedFile(url: string): { name: string; page: number | null } | null {
  const match = /^upload:[^/]*\/(.+?)(?:#page=(\d+))?$/.exec(url);
  return match ? { name: match[1], page: match[2] ? Number(match[2]) : null } : null;
}

export async function listFiles(project: string, source: string): Promise<UploadedFile[]> {
  return items<UploadedFile>(
    await unwrap(
      await api.GET("/api/v1/projects/{project}/knowledge/sources/{source}/files", {
        params: { path: { project, source } },
      }),
    ),
  );
}

/** Sends the file's bytes as they are; the Portal's sentence comes back as an `ApiError`. */
export async function uploadFile(project: string, source: string, file: Blob, name: string): Promise<UploadedFile> {
  const headers: Record<string, string> = { "Content-Type": "application/octet-stream" };
  const csrf = readCsrfToken();
  if (csrf) headers["x-csrf-token"] = csrf;
  const response = await globalThis.fetch(
    new Request(
      `${window.location.origin}/api/v1/projects/${encodeURIComponent(project)}/knowledge/sources/${encodeURIComponent(source)}/files?name=${encodeURIComponent(name)}`,
      { method: "POST", credentials: "same-origin", headers, body: await file.arrayBuffer() },
    ),
  );
  const answer: unknown = await response.json().catch(() => null);
  if (!response.ok) {
    const problem = answer as ProblemDetails | null;
    const detail = typeof problem?.detail === "string" ? problem.detail : `HTTP ${response.status}`;
    throw new ApiError(response.status, detail, problem ?? undefined, response.headers.get("x-request-id") ?? undefined);
  }
  if (!answer) throw new ApiError(response.status, "the upload answered nothing");
  return answer as UploadedFile;
}

export async function removeFile(project: string, source: string, id: number): Promise<void> {
  await unwrap(
    await api.DELETE("/api/v1/projects/{project}/knowledge/sources/{source}/files/{id}", {
      params: { path: { project, source, id } },
    }),
  );
}

export interface UsageDay {
  day: string;
  requests: number;
  tokensIn: number;
  tokensOut: number;
}

export async function listUsage(project: string, deployment: string): Promise<UsageDay[]> {
  return items<UsageDay>(
    await unwrap(
      await api.GET("/api/v1/projects/{project}/knowledge/deployments/{deployment}/usage", {
        params: { path: { project, deployment } },
      }),
    ),
  );
}

/** A byte count in the person's units: 1.2 MB, 340 kB. */
export function bytesText(bytes: number | null | undefined, language: string): string {
  if (bytes === null || bytes === undefined) return "—";
  const units = ["byte", "kilobyte", "megabyte", "gigabyte"] as const;
  let value = bytes;
  let unit = 0;
  while (value >= 1000 && unit < units.length - 1) {
    value /= 1000;
    unit += 1;
  }
  return new Intl.NumberFormat(language, {
    style: "unit",
    unit: units[unit],
    unitDisplay: "short",
    maximumFractionDigits: value < 10 && unit > 0 ? 1 : 0,
  }).format(value);
}

/** A time the assistant reported, in the person's locale; a dash when there is none. */
export function timeText(iso: string | null | undefined, language: string): string {
  if (!iso) return "—";
  const at = new Date(iso);
  return Number.isNaN(at.getTime())
    ? "—"
    : new Intl.DateTimeFormat(language, { dateStyle: "medium", timeStyle: "short" }).format(at);
}

/** One turn of a chat, as API/05 §1.1 sends it back. */
export interface ChatTurn {
  role: "user" | "assistant";
  text: string;
}

export interface Citation {
  n: number;
  url?: string;
  tool?: string;
  endpoint?: string;
  /** The cited page's title, which the person reads instead of its address (AG-117). */
  title?: string;
}

/** The events of API/05 §1.3, as the chat panel reads them. */
export type ChatEvent =
  | { name: "conversation"; id: string }
  | { name: "tool"; tool: string; endpoint?: string; status: "started" | "done" | "failed" }
  | { name: "script"; code: string; output?: string; error?: string }
  | { name: "answer"; text: string }
  | { name: "citations"; citations: Citation[] }
  | { name: "data"; data: DataTable }
  | { name: "error"; detail: string; title?: string }
  | { name: "done"; tokens: number };

function text(value: unknown): string {
  return typeof value === "string" ? value : "";
}

/** A `data` event's rows (API/05 §1.3, T-4140): at most 50, its links only when http(s). */
export interface DataTable {
  caption: string;
  columns: string[];
  rows: unknown[][];
  dataset?: string;
  download?: string;
}

function webUrl(value: unknown): string | undefined {
  return typeof value === "string" && /^https?:\/\//.test(value) ? value : undefined;
}

function dataTable(d: Record<string, unknown>): DataTable | null {
  if (!Array.isArray(d.columns) || d.columns.length === 0) return null;
  const columns = d.columns.map((c) => (typeof c === "string" ? c : JSON.stringify(c)));
  const rows = (Array.isArray(d.rows) ? d.rows : []).filter(Array.isArray).slice(0, 50) as unknown[][];
  return { caption: text(d.type), columns, rows, dataset: webUrl(d.dataset), download: webUrl(d.download) };
}

/** One cell as text: nothing for null, JSON for an object. */
export function cellText(value: unknown): string {
  if (value === null || value === undefined) return "";
  return typeof value === "object" ? JSON.stringify(value) : String(value);
}

export interface DataChart {
  kind: "line" | "bar";
  value: string;
  by: string;
  points: { label: string; value: number }[];
}

const TIME = /^\d{4}-\d{2}-\d{2}(?:[T ][\d:.]+(?:Z|[+-]\d{2}:?\d{2})?)?$/;

/**
 * The widget's chart rule (T-4198, crates/assistant/widget/render.js `chart`): exactly one column
 * holds numbers and another names them; a line over time when every name is a date, else bars; at
 * most 50 points, a value that is no finite number skipped; null otherwise and under two points.
 */
export function dataChart(table: DataTable): DataChart | null {
  const finite = (v: unknown): v is number => typeof v === "number" && Number.isFinite(v);
  const numeric = table.columns.map((_, i) => {
    const numbers = table.rows.filter((row) => finite(row[i])).length;
    const words = table.rows.filter((row) => typeof row[i] === "string" && (row[i] as string).trim() !== "").length;
    return numbers > 0 && numbers > words;
  });
  const values = numeric.flatMap((yes, i) => (yes ? [i] : []));
  if (values.length !== 1) return null;
  const named = table.columns.map((_, i) => i).filter((i) => !numeric[i]);
  const time = named.find((i) =>
    table.rows.every((row) => typeof row[i] === "string" && TIME.test(row[i] as string) && Number.isFinite(Date.parse(row[i] as string))),
  );
  const by = time ?? named[0];
  if (by === undefined) return null;
  const v = values[0];
  const points = table.rows
    .filter((row) => finite(row[v]))
    .slice(0, 50)
    .map((row) => ({ label: cellText(row[by]).slice(0, 40), value: row[v] as number }));
  if (time !== undefined) points.sort((a, b) => Date.parse(a.label) - Date.parse(b.label));
  if (points.length < 2) return null;
  return { kind: time !== undefined ? "line" : "bar", value: table.columns[v], by: table.columns[by], points };
}

/** One event block's name and data as a `ChatEvent`; `null` for one this panel does not show. */
function chatEvent(name: string, data: unknown): ChatEvent | null {
  const d = (data ?? {}) as Record<string, unknown>;
  switch (name) {
    case "conversation":
      return { name, id: text(d.id) };
    case "tool": {
      const status = d.status === "done" || d.status === "failed" ? d.status : "started";
      return { name, tool: text(d.name), endpoint: typeof d.endpoint === "string" ? d.endpoint : undefined, status };
    }
    case "script":
      return {
        name,
        code: text(d.code),
        output: typeof d.output === "string" ? d.output : undefined,
        error: typeof d.error === "string" ? d.error : undefined,
      };
    case "answer":
      return { name, text: text(d.text) };
    case "citations":
      return {
        name,
        citations: (Array.isArray(data) ? data : [])
          .filter((c): c is Record<string, unknown> => typeof c === "object" && c !== null && typeof c.n === "number")
          .map((c) => ({
            n: c.n as number,
            // An uploaded file is cited as `upload:{source}/{file}`, shown by name, never as a link (T-4869).
            url: typeof c.url === "string" && /^(https?:\/\/|upload:)/.test(c.url) ? c.url : undefined,
            tool: typeof c.tool === "string" ? c.tool : undefined,
            endpoint: typeof c.endpoint === "string" ? c.endpoint : undefined,
            title: typeof c.title === "string" ? c.title : undefined,
          })),
      };
    case "data": {
      const table = dataTable(d);
      return table ? { name, data: table } : null;
    }
    case "error":
      return { name, detail: text(d.detail), title: text(d.title) || undefined };
    case "done":
      return { name, tokens: typeof d.tokens === "number" ? d.tokens : 0 };
    default:
      return null;
  }
}

/**
 * The complete Server-Sent Events in `buffer`, and what is left of an event still arriving. A
 * block whose data is not JSON is skipped: the stream goes on.
 */
export function readEvents(buffer: string): { events: ChatEvent[]; rest: string } {
  const events: ChatEvent[] = [];
  const blocks = buffer.replace(/\r\n/g, "\n").split("\n\n");
  const rest = blocks.pop() ?? "";
  for (const block of blocks) {
    let name = "";
    let data = "";
    for (const line of block.split("\n")) {
      if (line.startsWith("event:")) name = line.slice(6).trim();
      else if (line.startsWith("data:")) data += line.slice(5).trim();
    }
    if (!name) continue;
    try {
      const event = chatEvent(name, JSON.parse(data));
      if (event) events.push(event);
    } catch {
      // A malformed block is skipped; the next one may be whole.
    }
  }
  return { events, rest };
}

export interface ChatQuestion {
  conversation?: string;
  message: string;
  history: ChatTurn[];
  connectors?: string[];
}

/**
 * Asks `deployment` as the signed-in person (API/01 §34, AG-115) and hands each event to
 * `onEvent` as it arrives. A refusal before the stream starts throws an `ApiError` with the
 * Portal's sentence.
 */
export async function askAssistant(
  project: string,
  deployment: string,
  question: ChatQuestion,
  onEvent: (event: ChatEvent) => void,
  signal?: AbortSignal,
): Promise<void> {
  const headers: Record<string, string> = { "Content-Type": "application/json", Accept: "text/event-stream" };
  const csrf = readCsrfToken();
  if (csrf) headers["x-csrf-token"] = csrf;
  const response = await globalThis.fetch(
    new Request(
      `${window.location.origin}/api/v1/projects/${encodeURIComponent(project)}/knowledge/deployments/${encodeURIComponent(deployment)}/chat`,
      { method: "POST", credentials: "same-origin", headers, body: JSON.stringify(question), signal },
    ),
  );
  if (!response.ok || !response.body) {
    const problem = (await response.json().catch(() => null)) as ProblemDetails | null;
    const detail = typeof problem?.detail === "string" ? problem.detail : `HTTP ${response.status}`;
    throw new ApiError(response.status, detail, problem ?? undefined, response.headers.get("x-request-id") ?? undefined);
  }
  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let buffer = "";
  for (;;) {
    const chunk = await reader.read();
    if (chunk.done) break;
    buffer += decoder.decode(chunk.value, { stream: true });
    const { events, rest } = readEvents(buffer);
    buffer = rest;
    events.forEach(onEvent);
  }
  readEvents(`${buffer}${decoder.decode()}\n\n`).events.forEach(onEvent);
}

/**
 * The one line a city site includes to get the assistant's corner button (T-4138, T-4196): the
 * launcher of this deployment on the assistant's host, in the deployment's first language. It
 * names only the public deployment; the chat answers on its allowed origins alone.
 */
export function launcherSnippet(domain: string, publicId: string, language?: string): string {
  const lang = language && /^[a-z]{2,3}(-[A-Za-z0-9]{2,8})*$/.test(language) ? ` data-lang="${language}"` : "";
  return `<script src="https://assistant.${domain}/d/${publicId}/embed.js"${lang} defer></script>`;
}

/** The iframe a site pastes to place a public assistant (API/05 §1.6). */
export function embedSnippet(domain: string, publicId: string, title: string): string {
  const escapedTitle = title.replace(/&/g, "&amp;").replace(/"/g, "&quot;").replace(/</g, "&lt;");
  return `<iframe src="https://assistant.${domain}/d/${publicId}/widget" title="${escapedTitle}" width="400" height="600" loading="lazy"></iframe>`;
}

/** `{base}-copy`, then `{base}-copy-2`, … : the first that `taken` does not hold, cut to a DNS label. */
function freeName(base: string, taken: Set<string>): string {
  for (let n = 1; ; n++) {
    const suffix = n === 1 ? "-copy" : `-copy-${n}`;
    const name = `${base.slice(0, 63 - suffix.length).replace(/-+$/, "")}${suffix}`;
    if (!taken.has(name)) return name;
  }
}

/**
 * A deployment's copy, to open as a create form (T-4281 item 9): every capability kept, a new name
 * and a new `publicId`, because two deployments that declare one are both refused (MF-52, T-3283).
 * Labels and annotations stay with the original.
 */
export function duplicateDeployment(
  manifest: { metadata: { name: string; namespace?: string }; spec?: Record<string, unknown> },
  existing: { metadata: { name: string }; spec?: { publicId?: unknown } }[],
): { apiVersion: string; kind: string; metadata: { name: string; namespace?: string }; spec: Record<string, unknown> } {
  const names = new Set(existing.map((item) => item.metadata.name));
  const ids = new Set(existing.map((item) => item.spec?.publicId).filter((id): id is string => typeof id === "string"));
  const publicId = typeof manifest.spec?.publicId === "string" ? manifest.spec.publicId : manifest.metadata.name;
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "AssistantDeployment",
    metadata: { name: freeName(manifest.metadata.name, names), namespace: manifest.metadata.namespace },
    spec: { ...structuredClone(manifest.spec ?? {}), publicId: freeName(publicId, ids) },
  };
}

/** One EU data-theme category with its label in the asked language (AG-124). */
export interface Category {
  code: string;
  label: string;
}

/** What an assistant reads, as the resolver resolves it (API/05 §1.9, AG-121). */
export interface AssistantContext {
  publicId: string;
  channel: string;
  title: string | null;
  languages: string[];
  categories: Category[];
  limits: {
    perClientPerHour: number | null;
    perClientPerDay: number | null;
    messagesPerConversation: number | null;
    questionChars: number | null;
  };
  endpoints: {
    slug: string;
    project: string;
    name: string;
    title: string | null;
    audience: string | null;
    themes: Category[];
    types: string[];
    /** The connector's tools; empty for an Endpoint that joined by the selector or the server. */
    tools: string[];
    timeoutSeconds: number;
  }[];
  /** Slugs past the 50 one deployment serves (AG-122). */
  omitted: string[];
  sources: {
    project: string;
    name: string;
    kind: string;
    visibility: string;
    sites: string[];
    categories: Category[];
  }[];
  /** The manifest's own rule findings; the preview resolves what it can regardless. */
  problems: string[];
}

/** The context an unsaved AssistantDeployment manifest would have; a dry run that changes nothing. */
export async function previewContext(
  project: string,
  manifest: Manifest,
  lang: string,
): Promise<AssistantContext> {
  const answer = (await unwrap(
    await api.POST("/api/v1/projects/{project}/knowledge/deployments/context", {
      params: { path: { project }, query: { dryRun: "All", lang } },
      body: manifest,
    }),
  )) as Partial<AssistantContext> | null;
  // The lists are what the preview walks; an answer without them reads as an empty context.
  const listed = <T,>(items: T[] | undefined): T[] => (Array.isArray(items) ? items : []);
  return {
    publicId: answer?.publicId ?? "",
    channel: answer?.channel ?? "",
    title: answer?.title ?? null,
    languages: listed(answer?.languages),
    categories: listed(answer?.categories),
    limits: answer?.limits ?? { perClientPerHour: null, perClientPerDay: null, messagesPerConversation: null, questionChars: null },
    endpoints: listed(answer?.endpoints),
    omitted: listed(answer?.omitted),
    sources: listed(answer?.sources),
    problems: listed(answer?.problems),
  };
}
