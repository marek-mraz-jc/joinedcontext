/**
 * The tab a person connected to one of their connectors (T-5967): which connector, the tab's
 * id for its event stream, and the actions the connector's client sent here. The pairing is the
 * tab's own (sessionStorage), so another tab of the same browser is not steered; the server
 * holds it to the person's session too.
 *
 * Every action arrives already checked by the server, the same checks the assistant's actions
 * pass (T-5959): a page from the Portal's table, a form's values against the kind's schema. Here
 * an action only opens a page, fills a form or points at a field. Nothing submits. A `refresh`
 * says only that the connector's client made a call: the open page reads its data again (T-6501).
 */
import { api, unwrap } from "../api/client";
import { handPrefill, isPortalRoute } from "../assistant/state";
import { runUiAction, uiActionOf } from "../assistant/uiActions";

const KEY = "jc.pairedTab";
const CHANGED = "jc:paired-tab";
/** How many actions the bar lists. */
export const LOG_LENGTH = 20;
/** At most one re-read of the page per this many ms, however fast the client calls. */
const REFRESH_MS = 3_000;
/** How long a closed stream waits before it counts as ended: a page unloading never gets there. */
const ENDED_AFTER_MS = 1_000;

export interface Pairing {
  connector: string;
  name: string;
  tab: string;
}

/** One action the connector's client sent this tab, as the bar lists it. */
export interface TabAction {
  tool: string;
  /** The page opened or the action run (`highlight`, `filter`). */
  what: string;
  connector: string;
  client: string | null;
  at: string;
  ran: boolean;
}

let log: TabAction[] = [];
let ended = false;
/** The tab's one event stream: it outlives a page (the shell remounts between sections). */
let source: EventSource | null = null;
let sourceKey = "";
/** The router's navigation of the mounted bar; actions wait for it while a page remounts. */
let go: ((href: string) => void) | null = null;
const waiting: unknown[] = [];
/** The mounted bar's re-read of the open page. */
let reread: (() => void) | null = null;
let rereading: ReturnType<typeof setTimeout> | null = null;

function announce(): void {
  window.dispatchEvent(new Event(CHANGED));
}

function scheduleReread(): void {
  rereading ??= setTimeout(() => {
    rereading = null;
    reread?.();
  }, REFRESH_MS);
}

function deliver(data: unknown): void {
  if ((data as { frame?: unknown } | null)?.frame === "refresh") {
    scheduleReread();
  } else if (go) {
    void runTabAction(data, go);
  } else {
    waiting.push(data);
  }
}

/** Opens the stream of the stored pairing, or closes it when there is none. */
function follow(): void {
  const current = pairing();
  const key = current ? `${current.connector}/${current.tab}` : "";
  if (key === sourceKey) {
    return;
  }
  source?.close();
  source = null;
  sourceKey = key;
  const Source = (globalThis as { EventSource?: typeof EventSource }).EventSource;
  if (!current || !Source) {
    return;
  }
  const opened = new Source(
    `/api/v1/connectors/${encodeURIComponent(current.connector)}/tab/${encodeURIComponent(current.tab)}/events`,
    { withCredentials: true },
  );
  opened.addEventListener("action", ((message: MessageEvent<string>) => {
    try {
      deliver(JSON.parse(message.data));
    } catch {
      // Not an action.
    }
  }) as EventListener);
  // A stream the server refuses (404 after a disconnect elsewhere, a newer tab, the session's
  // end) is closed by the browser for good; a dropped connection is retried by it. Leaving or
  // reloading the page closes it too, and that pairing lives on: the next page listens again.
  opened.onerror = () => {
    if (opened.readyState === Source.CLOSED) {
      setTimeout(() => {
        if (source === opened) {
          markEnded();
        }
      }, ENDED_AFTER_MS);
    }
  };
  source = opened;
}

/**
 * The mounted bar hands its navigation and its re-read here and the stream starts; actions that
 * arrived while no bar was mounted run now, in order. Returns the detach.
 */
export function attach(navigate: (href: string) => void, refresh: () => void): () => void {
  go = navigate;
  reread = refresh;
  follow();
  for (const data of waiting.splice(0)) {
    void runTabAction(data, navigate);
  }
  return () => {
    if (go === navigate) {
      go = null;
      reread = null;
    }
  };
}

export function pairing(): Pairing | null {
  try {
    const value = JSON.parse(sessionStorage.getItem(KEY) ?? "null") as Partial<Pairing> | null;
    return value && typeof value.connector === "string" && typeof value.name === "string" && typeof value.tab === "string"
      ? { connector: value.connector, name: value.name, tab: value.tab }
      : null;
  } catch {
    return null;
  }
}

function store(value: Pairing | null): void {
  try {
    if (value) {
      sessionStorage.setItem(KEY, JSON.stringify(value));
    } else {
      sessionStorage.removeItem(KEY);
    }
  } catch {
    // Without sessionStorage the pairing lasts as long as the page.
  }
  follow();
  announce();
}

export function tabLog(): TabAction[] {
  return log;
}

/** Whether the server ended the pairing (disconnected elsewhere, a newer tab, the session). */
export function pairingEnded(): boolean {
  return ended;
}

export function onPairingChange(listener: () => void): () => void {
  window.addEventListener(CHANGED, listener);
  return () => window.removeEventListener(CHANGED, listener);
}

/** Pairs this tab with the person's connector; a tab paired before stops hearing it. */
export async function connectTab(connector: string): Promise<Pairing> {
  const paired = await unwrap(await api.POST("/api/v1/connectors/{id}/tab", { params: { path: { id: connector } } }));
  const value = { connector: paired.connector, name: paired.name, tab: paired.tab };
  log = [];
  ended = false;
  store(value);
  return value;
}

/** Ends this tab's pairing, on the server first: the connector's UI tools refuse from then on. */
export async function disconnectTab(): Promise<void> {
  const current = pairing();
  if (current) {
    await unwrap(await api.DELETE("/api/v1/connectors/{id}/tab", { params: { path: { id: current.connector } } }));
  }
  log = [];
  ended = false;
  store(null);
}

/** The stream closed for good: the bar says so and offers to connect again. */
export function markEnded(): void {
  ended = true;
  log = [];
  store(null);
}

/** The person read that the pairing ended. */
export function dismissEnded(): void {
  ended = false;
  announce();
}

/** Reads one `action` event: `null` for anything the bar would not run. */
export function actionOf(data: unknown): { frame: "navigate" | "ui"; payload: Record<string, unknown>; entry: Omit<TabAction, "ran"> } | null {
  if (typeof data !== "object" || data === null) {
    return null;
  }
  const event = data as Record<string, unknown>;
  const payload = event.payload;
  const from = (event.from ?? {}) as Record<string, unknown>;
  if ((event.frame !== "navigate" && event.frame !== "ui") || typeof payload !== "object" || payload === null || typeof event.tool !== "string") {
    return null;
  }
  const body = payload as Record<string, unknown>;
  // What the bar names: the page opened, the field pointed at, the text typed into the filter.
  const what = event.frame === "navigate" ? body.route : body.action === "highlight" ? body.field : body.query;
  if (typeof what !== "string" || (event.frame === "navigate" && !isPortalRoute(what))) {
    return null;
  }
  return {
    frame: event.frame,
    payload: body,
    entry: {
      tool: event.tool,
      what,
      connector: typeof from.connector === "string" ? from.connector : "",
      client: typeof from.client === "string" ? from.client : null,
      at: typeof event.at === "string" ? event.at : new Date().toISOString(),
    },
  };
}

/**
 * Runs one action on this tab: a navigation hands its form values to the page it opens (taken
 * once by that page's form), a `ui` action points at a field or types into the table filter.
 */
export async function runTabAction(data: unknown, go: (href: string) => void): Promise<TabAction | null> {
  const action = actionOf(data);
  if (!action) {
    return null;
  }
  let ran = true;
  if (action.frame === "navigate") {
    const prefill = action.payload.prefill;
    if (typeof prefill === "object" && prefill !== null && !Array.isArray(prefill)) {
      handPrefill(action.entry.what, prefill as Record<string, unknown>);
    }
    go(action.entry.what);
  } else {
    const ui = uiActionOf(action.payload);
    ran = ui ? await runUiAction(ui) : false;
  }
  const entry = { ...action.entry, ran };
  log = [entry, ...log].slice(0, LOG_LENGTH);
  announce();
  return entry;
}
