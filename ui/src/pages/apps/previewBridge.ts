import { useEffect, useRef } from "react";
import type { RefObject } from "react";
import { api, readCsrfToken } from "../../api/client";
import { pickElement } from "../../assistant/state";
import { MAX_REASON } from "../../components/entities/filters";

/**
 * The bridge of a sandboxed preview (SDK-18, AP-63, Architecture/20 §5). The frame has no origin
 * and no session: every request of the SDK leaves it as a `jc-request`, this page performs it
 * with the reviewer's own session against the run's sandbox endpoint and posts a `jc-response`
 * back. Only the frame this page created is heard; reads under the endpoint pass always, writes
 * only for an operation the run's confirmed data needs name, `POST /functions/{fn}` to the run's
 * function route, which knows the run's files. Everything else is counted and never
 * forwarded; a refused request that carries an id is answered 403 at once, so the application
 * shows the refusal instead of waiting out its timeout. The frame's `jc-error` reports go to the
 * run for its repair, and its first `jc-observation` of each preview version to the run's
 * verification (SDK-27). The frame walks its pages only when this page asks with `jc-observe`:
 * once its SDK is ready (`jc-ready` or its first request), for a live run, once per run and
 * version in this page session, so closing and opening an application does not walk it again.
 */

export interface PreviewError {
  message: string;
  file?: string;
  line?: number;
}

const SLUG = /^[a-z2-7]{26,32}$/;
/** One path segment; `plainSegment` says what it may decode to (an entity id is a URN). */
const SEGMENT = "[^/]+";
const ID = /^[A-Za-z0-9:._~-]+$/;
const READS = [
  new RegExp(`^ngsi-ld/v1/entities(/${SEGMENT})?$`),
  new RegExp(`^ngsi-ld/v1/temporal/entities(/${SEGMENT})?$`),
  /^schema\/index\.json$/,
  /^schema\/v\d+\/json-schema$/,
  /^access$/,
];
const WRITES: [string, RegExp, string][] = [
  ["POST", /^ngsi-ld\/v1\/entities$/, "createEntity"],
  ["PATCH", new RegExp(`^ngsi-ld/v1/entities/${SEGMENT}/attrs$`), "updateAttrs"],
  ["DELETE", new RegExp(`^ngsi-ld/v1/entities/${SEGMENT}$`), "deleteEntity"],
];
const FUNCTION = /^\/functions\/([a-z][a-z0-9-]{0,39})$/;
const BASE = "http://portal.invalid";
/** The server's own bounds on a reported error (API/04 §5), and how many one frame load relays. */
const MAX_MESSAGE = 2000;
const MAX_FILE = 256;
export const MAX_ERRORS = 20;
/** The bounds of an observation (API/04 §5). */
export const MAX_OBSERVED_PAGES = 20;
export const MAX_OBSERVED_LABEL = 120;
export const MAX_OBSERVED_TEXT = 20_000;
export const MAX_OBSERVED_ROWS = 50;
export const MAX_OBSERVED_H1 = 50;
export const MAX_FAILED_REQUESTS = 50;
export const MAX_FAILED_PATH = 256;

export interface ObservedPage {
  label: string;
  text: string;
  rows: number[];
  /** The page's level-1 headings, counted; absent from an SDK older than T-3060. */
  h1?: number;
}

/** What the frame saw of one preview version, as the run takes it (SDK-27). */
export interface PreviewObservation {
  version: number;
  pages: ObservedPage[];
  failedRequests: { path: string; status: number }[];
}

/**
 * A segment the gateway cannot read as anything but one segment: once decoded, no `/`, `\`, `%`
 * (a double encoding) or dot segment, because the gateway decodes before it normalises.
 */
function plainSegment(segment: string): boolean {
  let decoded: string;
  try {
    decoded = decodeURIComponent(segment);
  } catch {
    return false;
  }
  return ID.test(decoded) && !/^\.+$/.test(decoded);
}

/**
 * What a request is: `"read"`, the NGSI-LD operation of a write, or null when this bridge never
 * forwards it. The path must be exactly what the URL parser keeps, so the path checked is the
 * path fetched.
 */
export function operationOf(method: unknown, path: unknown, slug: string): string | null {
  if (typeof path !== "string" || !SLUG.test(slug)) {
    return null;
  }
  let url: URL;
  try {
    url = new URL(path, BASE);
  } catch {
    return null;
  }
  const prefix = `/api/endpoint/${slug}/`;
  if (url.origin !== BASE || `${url.pathname}${url.search}` !== path || !url.pathname.startsWith(prefix)) {
    return null;
  }
  const rest = url.pathname.slice(prefix.length);
  if (!rest.split("/").every(plainSegment)) {
    return null;
  }
  if (method === "GET") {
    return READS.some((pattern) => pattern.test(rest)) ? "read" : null;
  }
  if (url.search !== "") {
    return null;
  }
  return WRITES.find(([verb, pattern]) => verb === method && pattern.test(rest))?.[2] ?? null;
}

/**
 * Where a `POST /functions/{fn}` of the frame goes: the run's function route under `route`, with
 * the request's query string; null for anything else.
 */
export function functionPathOf(method: unknown, path: unknown, route: string): string | null {
  if (method !== "POST" || typeof path !== "string") {
    return null;
  }
  let url: URL;
  try {
    url = new URL(path, BASE);
  } catch {
    return null;
  }
  const name = FUNCTION.exec(url.pathname)?.[1];
  if (url.origin !== BASE || `${url.pathname}${url.search}` !== path || !name) {
    return null;
  }
  return `${route}${name}${url.search}`;
}

/**
 * The `src` a preview frame may be given, or undefined when the value cannot be framed (UI-45).
 *
 * A preview URL arrives from the run record and from a manifest's status, so it is data. A
 * framed `javascript:` URL runs with the framing page's origin in browsers that still allow it,
 * and an absolute URL on another host turns the Portal into a frame for somebody else's page.
 *
 * Two shapes are served: a run's own pass, under `/api/v1/projects/…/preview`, and a published
 * app under `/apps/` (AP-14). Both are paths on this origin, and nothing else is accepted. The
 * caller shows the words instead of framing.
 */
const PREVIEW = /^\/(?:apps\/|api\/v1\/projects\/[^/]+\/agent-runs\/[^/]+\/preview(?:\?|$))/;

export function previewSrc(src: unknown): string | undefined {
  if (typeof src !== "string" || !PREVIEW.test(src)) {
    return undefined;
  }
  let url: URL;
  try {
    url = new URL(src, BASE);
  } catch {
    return undefined;
  }
  // `//evil.example/apps/` parses with an authority, and a backslash or an escape can put one
  // back after the prefix check: the round trip is what proves the string is only a path.
  return url.origin === BASE && `${url.pathname}${url.search}` === src ? src : undefined;
}

/** The operations the run's confirmed data needs name (AP-22): the grant, not the prompt. */
export function grantedOperations(dataNeeds: unknown): Set<string> {
  const needs: unknown[] = Array.isArray(dataNeeds) ? dataNeeds : [];
  return new Set(
    needs.flatMap((need) => {
      const operations = (need as { operations?: unknown } | null)?.operations;
      return Array.isArray(operations) ? operations.filter((op): op is string => typeof op === "string") : [];
    }),
  );
}

/** A `jc-error` message as the run stores it, or null when it is not one. */
export function previewErrorOf(data: unknown): PreviewError | null {
  const m = (typeof data === "object" && data !== null ? data : {}) as Record<string, unknown>;
  if (m.kind !== "jc-error" || typeof m.message !== "string" || m.message.trim() === "") {
    return null;
  }
  const error: PreviewError = { message: m.message.trim().slice(0, MAX_MESSAGE) };
  if (typeof m.file === "string" && m.file !== "") {
    error.file = m.file.slice(0, MAX_FILE);
  }
  if (typeof m.line === "number" && Number.isInteger(m.line) && m.line > 0) {
    error.line = m.line;
  }
  return error;
}

/** A position the transpiler stamps: an interface file under `src/` and a line (SDK-46). */
const SELECTED = /^src\/(?:[A-Za-z0-9_-][A-Za-z0-9._-]*\/)*[A-Za-z0-9_-][A-Za-z0-9._-]*\.tsx:[1-9]\d{0,8}$/;

/**
 * The `src` of a `jc-select` message, or null when it is not one: a position, never more, so a
 * page's text or rows cannot ride along as the scope of the next message.
 */
export function selectedSourceOf(data: unknown): string | null {
  const m = (typeof data === "object" && data !== null ? data : {}) as Record<string, unknown>;
  return m.kind === "jc-select" && typeof m.src === "string" && m.src.length <= 300 && SELECTED.test(m.src) ? m.src : null;
}

/** The `v` of a preview URL, or undefined. */
export function previewVersionOf(previewUrl: string | undefined): number | undefined {
  const match = /[?&]v=(\d{1,9})(?:&|#|$)/.exec(previewUrl ?? "");
  const version = match ? Number(match[1]) : NaN;
  return Number.isInteger(version) && version >= 1 ? version : undefined;
}

const isCount = (value: unknown, min: number, max: number): value is number =>
  typeof value === "number" && Number.isInteger(value) && value >= min && value <= max;

/**
 * A `jc-observation` exactly as the route accepts it, or null when any part breaks a bound: an
 * observation is judged whole, never trimmed into something the frame did not see. A frame that
 * does not know its version gets `fallbackVersion`, the `v` of the URL this page framed.
 */
export function previewObservationOf(data: unknown, fallbackVersion?: number): PreviewObservation | null {
  const m = (typeof data === "object" && data !== null ? data : {}) as Record<string, unknown>;
  if (m.kind !== "jc-observation") {
    return null;
  }
  const version = m.version === undefined ? fallbackVersion : m.version;
  if (!isCount(version, 1, Number.MAX_SAFE_INTEGER)) {
    return null;
  }
  if (!Array.isArray(m.pages) || m.pages.length < 1 || m.pages.length > MAX_OBSERVED_PAGES) {
    return null;
  }
  const pages: ObservedPage[] = [];
  for (const raw of m.pages as unknown[]) {
    const page = (typeof raw === "object" && raw !== null ? raw : {}) as Record<string, unknown>;
    const { label, text, rows, h1 } = page;
    if (
      typeof label !== "string" ||
      label.length > MAX_OBSERVED_LABEL ||
      typeof text !== "string" ||
      text.length > MAX_OBSERVED_TEXT ||
      !Array.isArray(rows) ||
      rows.length > MAX_OBSERVED_ROWS ||
      !rows.every((count) => isCount(count, 0, Number.MAX_SAFE_INTEGER)) ||
      (h1 !== undefined && !isCount(h1, 0, MAX_OBSERVED_H1))
    ) {
      return null;
    }
    pages.push(h1 === undefined ? { label, text, rows: rows as number[] } : { label, text, rows: rows as number[], h1 });
  }
  const failed = m.failedRequests === undefined ? [] : m.failedRequests;
  if (!Array.isArray(failed) || failed.length > MAX_FAILED_REQUESTS) {
    return null;
  }
  const failedRequests: { path: string; status: number }[] = [];
  for (const raw of failed as unknown[]) {
    const entry = (typeof raw === "object" && raw !== null ? raw : {}) as Record<string, unknown>;
    if (typeof entry.path !== "string" || entry.path.length > MAX_FAILED_PATH || !isCount(entry.status, 400, 599)) {
      return null;
    }
    failedRequests.push({ path: entry.path, status: entry.status });
  }
  return { version, pages, failedRequests };
}

/**
 * Posts each version's first observation to the run and no other; a refusal (`409` for a
 * version already observed or a run that ended) changes nothing on this page.
 */
export function observationRelay(
  project: string,
  id: string,
  fetchImpl: typeof fetch = (input, init) => globalThis.fetch(input, init),
): (observation: PreviewObservation) => void {
  const relayed = new Set<number>();
  return (observation) => {
    if (relayed.has(observation.version)) {
      return;
    }
    relayed.add(observation.version);
    void fetchImpl(
      `/api/v1/projects/${encodeURIComponent(project)}/agent-runs/${encodeURIComponent(id)}/preview-observations`,
      {
        method: "POST",
        credentials: "same-origin",
        headers: {
          accept: "application/json",
          "content-type": "application/json",
          "x-csrf-token": readCsrfToken() ?? "",
        },
        body: JSON.stringify(observation),
      },
    ).catch(() => undefined);
  };
}

/** The run versions this page session has asked a frame to observe, as `project/id/version`. */
const asked = new Set<string>();

/**
 * Whether to ask for `version` of the run `key`: true the first time only. `memory` is the page
 * session's own set unless a test brings one.
 */
export function firstAsk(key: string, memory: Set<string> = asked): (version: number) => boolean {
  return (version) => {
    const entry = `${key}/${version}`;
    if (memory.has(entry)) {
      return false;
    }
    memory.add(entry);
    return true;
  };
}

export interface Preview {
  slug: string | undefined;
  /** Every other endpoint slug of the run (AP-44, SDK-18): reads and writes there pass the same rules. */
  slugs?: readonly string[];
  operations: ReadonlySet<string>;
  /** The run's function route, ending in `/functions/`. */
  functions?: string;
  /** The frame this page created. */
  source: Window | null | undefined;
  /** Where a valid observation goes; without it an observation is refused. */
  observe?: (observation: PreviewObservation) => void;
  /** The `v` of the URL this page framed, for a frame that does not say its own. */
  version?: number;
  /** Whether the run is still live; an ended run's preview is never asked to observe. */
  live?: boolean;
  /** Whether this version may be asked for now; true once per version (see `firstAsk`). */
  ask?: (version: number) => boolean;
  /** Where the position of a `jc-select` goes (SDK-46); without it a selection is refused. */
  select?: (src: string) => void;
}

export type Outcome = "forwarded" | "refused" | "relayed" | "ignored";

/** What a preview's write says when the application names no reason of its own (DM-113). */
export const PREVIEW_REASON = "written by an application preview";

/** The frame's own reason when the gateway would take it (1 to 500 characters), else the preview's. */
export function previewReason(asked: unknown): string {
  const trimmed = typeof asked === "string" ? asked.trim() : "";
  return trimmed !== "" && [...trimmed].length <= MAX_REASON ? trimmed : PREVIEW_REASON;
}

async function perform(
  method: string,
  path: string,
  body: unknown,
  fetchImpl: typeof fetch,
  reason?: string,
): Promise<{ status: number; body: unknown }> {
  const headers: Record<string, string> = { accept: "application/json" };
  if (method !== "GET") {
    headers["x-csrf-token"] = readCsrfToken() ?? "";
    if (reason !== undefined) {
      headers["JC-Reason"] = encodeURIComponent(reason);
      // As the SDK transport: a write with a reason keeps the value it replaces (T-6333).
      headers["JC-Record"] = "version";
    }
    if (body !== undefined) {
      headers["content-type"] = "application/json";
    }
  }
  try {
    const response = await fetchImpl(path, {
      method,
      credentials: "same-origin",
      headers,
      body: method !== "GET" && body !== undefined ? JSON.stringify(body) : undefined,
    });
    if (response.status === 204) {
      return { status: 204, body: null };
    }
    const json = (response.headers.get("content-type") ?? "").includes("json");
    const text = await response.text().catch(() => "");
    let parsed: unknown = text === "" ? null : text;
    if (json && text !== "") {
      try {
        parsed = JSON.parse(text);
      } catch {
        parsed = text;
      }
    }
    return { status: response.status, body: parsed };
  } catch (err) {
    return { status: 0, body: { title: err instanceof Error ? err.message : String(err) } };
  }
}

/** Asks a ready frame to observe its version, when the run is live and has not asked for it yet. */
function askToObserve(source: Window, preview: Preview): void {
  const { live, version, ask } = preview;
  if (live && version !== undefined && ask?.(version)) {
    source.postMessage({ kind: "jc-observe", version }, "*");
  }
}

/**
 * One message: ignored when it is not from the preview's frame, relayed when it is an error
 * report or the frame saying it is ready, forwarded when it is a request this bridge performs,
 * refused otherwise.
 */
export async function handleBridgeMessage(
  event: { source: unknown; data: unknown },
  preview: Preview,
  report: (error: PreviewError) => void,
  fetchImpl: typeof fetch = (input, init) => globalThis.fetch(input, init),
): Promise<Outcome> {
  const { slug, source } = preview;
  if (!slug || !source || event.source !== source) {
    return "ignored";
  }
  const error = previewErrorOf(event.data);
  if (error) {
    report(error);
    return "relayed";
  }
  const m = (typeof event.data === "object" && event.data !== null ? event.data : {}) as Record<string, unknown>;
  if (m.kind === "jc-ready") {
    askToObserve(source, preview);
    return "relayed";
  }
  if (m.kind === "jc-select") {
    const src = selectedSourceOf(event.data);
    if (src === null || !preview.select) {
      return "refused";
    }
    preview.select(src);
    return "relayed";
  }
  if (m.kind === "jc-observation") {
    const observation = previewObservationOf(event.data, preview.version);
    if (!observation || !preview.observe) {
      return "refused";
    }
    preview.observe(observation);
    return "relayed";
  }
  if (m.kind !== "jc-request" || typeof m.id !== "number") {
    return "refused";
  }
  // A frame that sends requests is ready, whether or not its `jc-ready` arrived.
  askToObserve(source, preview);
  const fn = preview.functions ? functionPathOf(m.method, m.path, preview.functions) : null;
  if (fn !== null) {
    source.postMessage({ kind: "jc-response", id: m.id, ...(await perform("POST", fn, m.body, fetchImpl)) }, "*");
    return "forwarded";
  }
  const operation = [slug, ...(preview.slugs ?? [])].reduce<string | null>(
    (found, candidate) => found ?? operationOf(m.method, m.path, candidate),
    null,
  );
  const writesBody = m.method === "POST" || m.method === "PATCH";
  let refusal: string | null = null;
  if (operation === null) {
    refusal = "The preview may only read and write under its own endpoint and call its own functions.";
  } else if (operation !== "read" && !preview.operations.has(operation)) {
    refusal = `The run's data needs do not name ${operation}.`;
  } else if (writesBody && (typeof m.body !== "object" || m.body === null)) {
    refusal = "A write carries a JSON object.";
  }
  if (refusal !== null) {
    source.postMessage(
      { kind: "jc-response", id: m.id, status: 403, body: { title: "Refused by the preview bridge", status: 403, detail: refusal } },
      "*",
    );
    return "refused";
  }
  const reason = operation === "read" ? undefined : previewReason(m.reason);
  const answer = await perform(m.method as string, m.path as string, m.body, fetchImpl, reason);
  source.postMessage({ kind: "jc-response", id: m.id, ...answer }, "*");
  return "forwarded";
}

/**
 * Serves the frame's requests while the page is mounted, relays its first MAX_ERRORS distinct
 * error reports per preview document to the run, and asks it to observe while `live`.
 */
export function usePreviewBridge(
  frame: RefObject<HTMLIFrameElement | null>,
  run:
    | {
        id: string;
        project: string;
        endpointSlug?: string;
        endpoints?: readonly { slug: string }[];
        dataNeeds?: unknown;
        previewUrl?: string;
      }
    | undefined,
  live: boolean,
): void {
  const id = run?.id;
  const project = run?.project;
  const slug = run?.endpointSlug;
  const others = (run?.endpoints ?? [])
    .map((endpoint) => endpoint.slug)
    .filter((candidate) => candidate !== slug)
    .join(",");
  const previewUrl = run?.previewUrl;
  const needs = JSON.stringify(run?.dataNeeds ?? null);
  // One relay per run, so a version observed once is not posted again when the frame reloads.
  const relay = useRef<{
    key: string;
    observe: (observation: PreviewObservation) => void;
    ask: (version: number) => boolean;
  } | null>(null);
  useEffect(() => {
    if (!id || !project || !slug) {
      return undefined;
    }
    const operations = grantedOperations(JSON.parse(needs));
    const functions = `/api/v1/projects/${encodeURIComponent(project)}/agent-runs/${encodeURIComponent(id)}/functions/`;
    const seen = new Set<string>();
    let refused = 0;
    const report = (error: PreviewError) => {
      const key = `${error.file ?? ""}:${error.line ?? ""}:${error.message}`;
      if (seen.has(key) || seen.size >= MAX_ERRORS) {
        return;
      }
      seen.add(key);
      void api
        .POST("/api/v1/projects/{project}/agent-runs/{id}/preview-errors", {
          params: { path: { project, id } },
          body: error,
        })
        .catch(() => undefined);
    };
    const key = `${project}/${id}`;
    if (relay.current?.key !== key) {
      relay.current = { key, observe: observationRelay(project, id), ask: firstAsk(key) };
    }
    const { observe, ask } = relay.current;
    const version = previewVersionOf(previewUrl);
    const listener = (event: MessageEvent) => {
      const slugs = others === "" ? [] : others.split(",");
      const select = (src: string) => pickElement(id, src);
      const preview = { slug, slugs, operations, functions, source: frame.current?.contentWindow, observe, version, live, ask, select };
      void handleBridgeMessage(event, preview, report).then((outcome) => {
        if (outcome === "refused") {
          refused += 1;
          console.warn(`preview bridge: refused message ${refused} from the preview frame`);
        }
      });
    };
    window.addEventListener("message", listener);
    return () => window.removeEventListener("message", listener);
  }, [frame, id, project, slug, others, needs, previewUrl, live]);
}
