import { activity, MAX_FAILED_REQUESTS } from "./transport";

/**
 * What the preview shows, read by the SDK inside the frame and posted to the Portal once per
 * document when the Portal asks for it (SDK-27, Architecture/20 §4.1): the run checks each
 * generated version against the entities it sampled without asking anyone to look. Once the application's requests have
 * settled, the observer walks the pages its navigation offers (the links and tabs inside `nav`),
 * reads each one's visible text, table row counts and level-1 headings (a count, never their
 * text: T-3060), and returns to the page the person was on.
 * The bounds are the route's own (API/04 §5), so a post is never refused for its size.
 */

export const MAX_PAGES = 20;
export const MAX_LABEL = 120;
export const MAX_TEXT = 20_000;
export const MAX_ROWS = 50;
export const MAX_H1 = 50;

export interface ObservedPage {
  label: string;
  text: string;
  rows: number[];
  /** How many level-1 headings the page shows; the run asks for exactly one (SDK-28). */
  h1: number;
}

export interface Observation {
  kind: "jc-observation";
  /** The `v` of the preview URL the frame loaded; absent when the document does not know it. */
  version?: number;
  pages: ObservedPage[];
  failedRequests: { path: string; status: number }[];
}

export interface ObserveOptions {
  doc?: Document;
  win?: Window;
  /** How long no request may be pending before the first page counts as rendered. */
  quietMs?: number;
  /** The longest the first wait lasts; the page is read as it stands after that. */
  maxWaitMs?: number;
  /** The same two for each page the walk opens. */
  pageQuietMs?: number;
  pageMaxWaitMs?: number;
  pollMs?: number;
}

const sleep = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));

/**
 * Resolves once no bridge request has been pending for `quietMs`, counted from the call at the
 * earliest, or after `maxMs` in any case; says which of the two it was.
 */
export async function waitForIdle(quietMs: number, maxMs: number, pollMs = 100): Promise<boolean> {
  const started = Date.now();
  for (;;) {
    const now = Date.now();
    if (activity.pending === 0 && now - Math.max(activity.lastChange, started) >= quietMs) {
      return true;
    }
    if (now - started >= maxMs) {
      return false;
    }
    await sleep(Math.min(pollMs, quietMs));
  }
}

function clean(text: string, max: number): string {
  return text
    .replace(/[ \t\u00a0]+/g, " ")
    .replace(/ ?\n ?/g, "\n")
    .replace(/\n{3,}/g, "\n\n")
    .trim()
    .slice(0, max);
}

interface NavItem {
  label: string;
  element: HTMLElement;
}

/** The pages the navigation offers, once each by label, never a link that leaves the document. */
export function navItems(doc: Document): NavItem[] {
  const items: NavItem[] = [];
  const seen = new Set<string>();
  for (const element of Array.from(doc.querySelectorAll<HTMLElement>("nav a, nav button, [role=tab]"))) {
    if (items.length >= MAX_PAGES) break;
    if (element.hasAttribute("disabled") || element.getAttribute("aria-disabled") === "true") continue;
    if (element.tagName === "A") {
      const href = element.getAttribute("href");
      // A click on any other link would navigate the frame away from the application.
      if ((href !== null && href !== "" && !href.startsWith("#")) || element.hasAttribute("download")) continue;
      if (element.getAttribute("target") === "_blank") continue;
    }
    const label = clean(element.getAttribute("aria-label") ?? element.textContent ?? "", MAX_LABEL);
    if (label === "" || seen.has(label)) continue;
    seen.add(label);
    items.push({ label, element });
  }
  return items;
}

function isActive(element: HTMLElement): boolean {
  const current = element.getAttribute("aria-current");
  return (
    (current !== null && current !== "false") ||
    element.getAttribute("aria-selected") === "true" ||
    element.classList.contains("active")
  );
}

/** One page as it stands: its label, visible text, the row count of each table body and its h1s. */
export function snapshot(doc: Document, label: string): ObservedPage {
  const body = doc.body as (HTMLElement & { innerText?: string }) | null;
  const raw = body ? (typeof body.innerText === "string" ? body.innerText : (body.textContent ?? "")) : "";
  const rows = Array.from(doc.querySelectorAll("table tbody"))
    .slice(0, MAX_ROWS)
    .map((tbody) => tbody.querySelectorAll("tr").length);
  const h1 = Math.min(doc.querySelectorAll('h1, [role="heading"][aria-level="1"]').length, MAX_H1);
  return { label: clean(label, MAX_LABEL) || "Page", text: clean(raw, MAX_TEXT), rows, h1 };
}

/** The `v` of the preview URL, without URLSearchParams (the SDK avoids Web APIs it can do without). */
export function versionOf(search: string): number | undefined {
  const match = /[?&]v=(\d{1,9})(?:&|$)/.exec(search);
  const version = match ? Number(match[1]) : NaN;
  return Number.isInteger(version) && version >= 1 ? version : undefined;
}

/**
 * Reads the application page by page. Null outside a frame: a published application is nobody's
 * preview. Never throws.
 */
export async function observe(options: ObserveOptions = {}): Promise<Observation | null> {
  const win = options.win ?? (typeof window !== "undefined" ? window : undefined);
  const doc = options.doc ?? (typeof document !== "undefined" ? document : undefined);
  if (!win || !doc || !win.parent || win.parent === win) {
    return null;
  }
  const poll = options.pollMs ?? 100;
  await waitForIdle(options.quietMs ?? 1500, options.maxWaitMs ?? 15_000, poll);

  const pages: ObservedPage[] = [];
  const items = navItems(doc);
  if (items.length === 0) {
    pages.push(snapshot(doc, doc.title));
  } else {
    const hash = win.location.hash;
    const active = items.find((item) => isActive(item.element))?.label;
    for (const { label } of items) {
      // The navigation may render again after each click, so each item is found afresh.
      const item = navItems(doc).find((candidate) => candidate.label === label);
      if (!item) continue;
      item.element.click();
      await waitForIdle(options.pageQuietMs ?? 1000, options.pageMaxWaitMs ?? 5000, poll);
      pages.push(snapshot(doc, label));
    }
    const back = active === undefined ? undefined : navItems(doc).find((item) => item.label === active);
    if (back) {
      back.element.click();
    } else if (win.location.hash !== hash) {
      win.location.hash = hash;
    }
    if (pages.length === 0) {
      pages.push(snapshot(doc, doc.title));
    }
  }
  return {
    kind: "jc-observation",
    version: versionOf(win.location.search),
    pages,
    failedRequests: activity.failed.slice(0, MAX_FAILED_REQUESTS),
  };
}

let started = false;
let listening = false;

/**
 * Waits for the host page to ask (`{kind: "jc-observe", version}` from the window that framed
 * this document) and then observes once and posts the result; errors stay in the console. The
 * page asks only for a live run's version it has not had an observation of, so reopening a
 * finished application never walks its pages (SDK-27). Tells the host it is listening with
 * `{kind: "jc-ready"}`.
 */
export function startObserver(options: ObserveOptions = {}): void {
  if (listening) return;
  const win = options.win ?? (typeof window !== "undefined" ? window : undefined);
  if (!win || !win.parent || win.parent === win || typeof win.addEventListener !== "function") {
    return;
  }
  listening = true;
  win.addEventListener("message", (event: MessageEvent) => {
    const data = (typeof event.data === "object" && event.data !== null ? event.data : {}) as Record<string, unknown>;
    if (event.source !== win.parent || data.kind !== "jc-observe" || started) {
      return;
    }
    started = true;
    const asked = typeof data.version === "number" && Number.isInteger(data.version) && data.version >= 1 ? data.version : undefined;
    void observe(options)
      .then((observation) => {
        if (observation && win.parent && win.parent !== win) {
          win.parent.postMessage({ ...observation, version: observation.version ?? asked }, "*");
        }
      })
      .catch((error: unknown) => {
        console.error("jc: the preview could not be observed", error);
      });
  });
  try {
    win.parent.postMessage({ kind: "jc-ready" }, "*");
  } catch {
    // A host that cannot be told simply never asks.
  }
}

/** Lets a test observe again in the same document. */
export function resetObserver(): void {
  started = false;
  listening = false;
}
