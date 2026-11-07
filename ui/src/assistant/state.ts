/**
 * What the assistant dock remembers between pages (UI-45).
 *
 * The run a person is talking to follows them from page to page, and a `navigate` event's
 * prefill has to survive the route change that delivers it. Both live in `sessionStorage`, so a
 * reload keeps them and a new tab starts clean; every read and write is guarded, because a
 * private window or a cleared site may refuse the storage and the dock must still render.
 */

const RUN_KEY = "jc.assistant.run";
const PREFILL_KEY = "jc.assistant.prefill";
const NOTICE_KEY = "jc.assistant.notice";
const NAVIGATED_KEY = "jc.assistant.navigated";
const ACTED_KEY = "jc.assistant.acted";
const NOTICE_SEEN_KEY = "jc.assistant.noticeSeen";
const TRAIL_KEY = "jc.assistant.trail";

/** How many of the assistant's pages the panel offers back (UI-59). */
export const TRAIL_LENGTH = 3;
const CHANGED = "jc:assistant";
const OPEN_REQUEST = "jc:assistant-open";
const ASK_REQUEST = "jc:assistant-ask";

export interface ActiveRun {
  project: string;
  runId: string;
}

/** The route rule of API/04 §4, mirrored here so a frame the server did not write is still refused. */
export function isPortalRoute(route: unknown): route is string {
  return (
    typeof route === "string" &&
    route.startsWith("/") &&
    !route.startsWith("//") &&
    route.length <= 512 &&
    !route.includes("#") &&
    !route.includes(":") &&
    // eslint-disable-next-line no-control-regex
    !/[\u0000-\u001f\u007f]/.test(route)
  );
}

function read(key: string): unknown {
  try {
    const raw = window.sessionStorage.getItem(key);
    return raw === null ? null : (JSON.parse(raw) as unknown);
  } catch {
    return null;
  }
}

function write(key: string, value: unknown): void {
  try {
    if (value === null) {
      window.sessionStorage.removeItem(key);
    } else {
      window.sessionStorage.setItem(key, JSON.stringify(value));
    }
  } catch {
    // Storage refused: the dock then lives for this page only.
  }
}

/** The stored run as the raw string: a stable snapshot for `useSyncExternalStore`. */
export function runSnapshot(): string | null {
  try {
    return window.sessionStorage.getItem(RUN_KEY);
  } catch {
    return null;
  }
}

export function parseRun(raw: string | null): ActiveRun | null {
  if (raw === null) {
    return null;
  }
  try {
    const value = JSON.parse(raw) as Partial<ActiveRun> | null;
    return value &&
      typeof value.project === "string" &&
      typeof value.runId === "string"
      ? { project: value.project, runId: value.runId }
      : null;
  } catch {
    return null;
  }
}

/** The run the dock follows from now on; `null` closes the dock. */
export function rememberRun(run: ActiveRun | null): void {
  write(RUN_KEY, run);
  window.dispatchEvent(new Event(CHANGED));
}

/** Called back whenever the run or the notice changes, in this tab. */
export function onAssistantChange(listener: () => void): () => void {
  window.addEventListener(CHANGED, listener);
  return () => {
    window.removeEventListener(CHANGED, listener);
  };
}

/**
 * The form values a `navigate` event carries for `route`, kept until the page takes them, and
 * the notice that the assistant navigated, kept until the person dismisses it or leaves the page
 * it names: the dock is remounted by the route change that delivers the event, so neither can
 * live in its state.
 */
export function rememberPrefill(
  route: string,
  prefill: Record<string, unknown>,
): void {
  handPrefill(route, prefill);
  write(NOTICE_KEY, route);
  write(NOTICE_SEEN_KEY, null);
  write(TRAIL_KEY, pushTrail(trail(), route));
  window.dispatchEvent(new Event(CHANGED));
}

/** The pages the assistant opened, newest first, at most [`TRAIL_LENGTH`] (UI-59). */
export function trail(): string[] {
  const value = read(TRAIL_KEY);
  return Array.isArray(value) ? value.filter(isPortalRoute) : [];
}

/** The trail with `route` on top, each page once: walking back is a short list, not a history. */
export function pushTrail(known: string[], route: string): string[] {
  return [route, ...known.filter((seen) => seen !== route)].slice(
    0,
    TRAIL_LENGTH,
  );
}

/**
 * The prefill kept in this tab's memory as well: storage refuses a hand-off past its quota (a
 * few megabytes of drafts, T-0891), and the page the route change mounts must still get it.
 */
let handed: { route: string; prefill: Record<string, unknown> } | null = null;

/**
 * What the page already took, kept until the person leaves that route. A page mounts more than
 * once for one hand-off — `HandOff` remounts it when the address settles behind the route
 * change — and a take that spent the prefill on the first, discarded mount left the second one
 * empty: the assistant's drafts never appeared on Complete this space (T-0907).
 */
let replay: { route: string; prefill: Record<string, unknown> } | null = null;

/** The form values `route`'s page takes as it mounts, left by a page or the assistant. */
export function handPrefill(
  route: string,
  prefill: Record<string, unknown>,
): void {
  handed = { route: route.split("?")[0], prefill };
  replay = null;
  write(PREFILL_KEY, handed);
  handOffs += 1;
  handOffListeners.forEach((listener) => listener());
}

/**
 * How many prefills were handed in this tab: a second one to the page already open at the same
 * address (a connector's newer plan, JC2-11) mounts the page afresh, so its form takes the new
 * values instead of keeping what it took the first time.
 */
let handOffs = 0;
const handOffListeners = new Set<() => void>();

export function handOffCount(): number {
  return handOffs;
}

export function subscribeHandOffs(listener: () => void): () => void {
  handOffListeners.add(listener);
  return () => handOffListeners.delete(listener);
}

/**
 * The notice against the page the person is on: on the page it names it has been seen, and on
 * any other page after that it is gone. Before it has been seen, the page that is still open
 * while the assistant's navigation happens leaves it alone.
 */
export function settleNotice(pathname: string): void {
  const route = noticeSnapshot();
  if (route === null) {
    return;
  }
  if (route.split("?")[0] === pathname) {
    write(NOTICE_SEEN_KEY, route);
  } else if (read(NOTICE_SEEN_KEY) === route) {
    dismissNotice();
  }
}

/**
 * The `seq` of the last `navigate` event of `runId` the dock followed. The route change remounts
 * the dock and its stream replays the run from the start, so the mark lives in the tab: an event
 * already followed never takes the person back from a page they went to since.
 */
export function navigatedSeq(runId: string): number {
  const value = read(NAVIGATED_KEY) as {
    runId?: unknown;
    seq?: unknown;
  } | null;
  return value?.runId === runId && typeof value.seq === "number"
    ? value.seq
    : 0;
}

export function rememberNavigated(runId: string, seq: number): void {
  write(NAVIGATED_KEY, { runId, seq });
}

/** The `seq` of the last `ui` action of `runId` the page ran (T-5959), kept like [`navigatedSeq`]. */
export function actedSeq(runId: string): number {
  const value = read(ACTED_KEY) as { runId?: unknown; seq?: unknown } | null;
  return value?.runId === runId && typeof value.seq === "number" ? value.seq : 0;
}

export function rememberActed(runId: string, seq: number): void {
  write(ACTED_KEY, { runId, seq });
}

/** The route the assistant last opened, until dismissed; a stable snapshot. */
export function noticeSnapshot(): string | null {
  const value = read(NOTICE_KEY);
  return typeof value === "string" ? value : null;
}

export function dismissNotice(): void {
  write(NOTICE_KEY, null);
  write(NOTICE_SEEN_KEY, null);
  window.dispatchEvent(new Event(CHANGED));
}

/**
 * Whether a prefill waits for this path, without taking it: a page that is two pages at one
 * address (the Data models list and its editor, T-2765) asks this to know which one to mount.
 */
export function hasPrefill(pathname: string): boolean {
  const value = (handed ?? read(PREFILL_KEY)) as { route?: unknown } | null;
  return value?.route === pathname || replay?.route === pathname;
}

/** The prefill left for this path, taken once: a second visit starts with an empty form. */
export function takePrefill(pathname: string): Record<string, unknown> | null {
  const value = (handed ?? read(PREFILL_KEY)) as {
    route?: unknown;
    prefill?: unknown;
  } | null;
  if (value && value.route === pathname) {
    handed = null;
    write(PREFILL_KEY, null);
    replay =
      typeof value.prefill === "object" && value.prefill !== null
        ? { route: pathname, prefill: value.prefill as Record<string, unknown> }
        : null;
  }
  return replay && replay.route === pathname ? replay.prefill : null;
}

/**
 * The hand-off is spent when the person is somewhere else: called with the route they are on, so
 * a page that mounts again for the same hand-off still gets it and a page opened later does not.
 */
export function settlePrefill(pathname: string): void {
  if (replay && replay.route !== pathname) {
    replay = null;
  }
}

/**
 * The resource a page was opened to change (`?edit=<name>`, AG-77), with the changed manifest the
 * assistant left for this path when it left one; the prefill is taken once. With `name`, only a
 * request for that resource is taken, so each row of a list can ask for its own.
 */
export function takeEditRequest(
  name?: string,
): { name: string; manifest: Record<string, unknown> | null } | null {
  if (typeof window === "undefined") {
    return null;
  }
  const edit = new URLSearchParams(window.location.search).get("edit");
  if (!edit || (name !== undefined && edit !== name)) {
    return null;
  }
  const prefill = takePrefill(window.location.pathname);
  const named =
    (prefill?.metadata as { name?: unknown } | undefined)?.name === edit;
  return { name: edit, manifest: named ? prefill : null };
}

/** What the panel opens on: the chat, or the app builder (`build`). */
export type OpenIntent = "chat" | "build";

/** Nobody listens yet: a request made while the page mounts, before the dock has (T-2750). */
let openListeners = 0;
let unheardOpen: OpenIntent | null = null;

export function requestOpen(intent: OpenIntent = "chat"): void {
  if (openListeners === 0) {
    unheardOpen = intent;
  }
  window.dispatchEvent(
    new CustomEvent<OpenIntent>(OPEN_REQUEST, { detail: intent }),
  );
}

export function onOpenRequest(
  listener: (intent: OpenIntent) => void,
): () => void {
  const handler = (event: Event) => {
    listener(
      (event as CustomEvent<OpenIntent>).detail === "build" ? "build" : "chat",
    );
  };
  window.addEventListener(OPEN_REQUEST, handler);
  openListeners += 1;
  // A page effect runs before the dock's (the dock is drawn after the page), so an address that
  // opens the dock — `/apps/new` — asks before anyone listens; the first listener gets it.
  if (unheardOpen !== null) {
    const intent = unheardOpen;
    unheardOpen = null;
    listener(intent);
  }
  return () => {
    window.removeEventListener(OPEN_REQUEST, handler);
    openListeners -= 1;
  };
}

/**
 * The form a person is standing in, for the question they are about to ask (UI-61, T-1611).
 *
 * Module state, not `sessionStorage`: it lives exactly as long as the open dialog does, and a
 * reload with no form open must not tell the assistant there is one. Values never travel — only the
 * kind, the draft the form edits and the field the person was last in; what the form holds is the
 * draft itself, which the assistant reads under the person's own grants.
 */
export interface FormContext {
  kind?: string;
  name?: string;
  field?: string;
}

let openForm: FormContext | null = null;

/** The open form names itself, or takes it back when it closes (`null`). */
export function standingIn(form: FormContext | null): void {
  openForm = form;
}

/** What to send with the next question, or `undefined` when no form is open. */
export function formContext(): FormContext | undefined {
  if (!openForm || (!openForm.kind && !openForm.name && !openForm.field)) {
    return undefined;
  }
  return { ...openForm };
}

/** A route segment the Portal takes as a page, a name or a tab: a DNS-1123 label. */
const LABEL = /^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$/;

/**
 * The page the person is on, to send with a question (T-2763, API/04): the route, and only a
 * route the Portal reads as a page of this project, so a question asked anywhere else starts
 * without one rather than being refused. The Portal reads it again; names only, never values.
 */
export function pageContext(project: string, location: Pick<Location, "pathname" | "search"> = window.location): { route: string } | undefined {
  const segments = location.pathname.replace(/\/+$/, "").split("/").slice(1);
  const [root, on, ...rest] = segments;
  if (root !== "projects" || on !== project || rest.length < 1 || rest.length > 3 || !rest.every((one) => LABEL.test(one))) {
    return undefined;
  }
  const tab = new URLSearchParams(location.search).get("tab");
  const route = `/projects/${project}/${rest.join("/")}${tab !== null && LABEL.test(tab) ? `?tab=${tab}` : ""}`;
  return route.length <= 300 ? { route } : undefined;
}

/**
 * Opens the assistant with a question already written, which the person sends (or edits) themselves
 * — the dock never asks on its own behalf (AG-73).
 */
export function askAbout(question: string): void {
  window.dispatchEvent(
    new CustomEvent<string>(ASK_REQUEST, { detail: question }),
  );
  requestOpen();
}

export function onAskRequest(listener: (question: string) => void): () => void {
  const handler = (event: Event) => {
    const asked = (event as CustomEvent<string>).detail;
    if (typeof asked === "string" && asked.trim() !== "") {
      listener(asked);
    }
  };
  window.addEventListener(ASK_REQUEST, handler);
  return () => {
    window.removeEventListener(ASK_REQUEST, handler);
  };
}

const PICKED = "jc:assistant-picked";
let picked: { runId: string; src: string } | null = null;

/**
 * The element a person pointed at in a run's preview, `{file}:{line}` (SDK-46): the next message
 * to that run carries it as its scope; `null` takes it back. Module state: a reload forgets it.
 */
export function pickElement(runId: string, src: string | null): void {
  if (src !== null) {
    picked = { runId, src };
  } else if (picked?.runId === runId) {
    picked = null;
  }
  window.dispatchEvent(new Event(PICKED));
}

/** What the person pointed at in `runId`'s preview, or null. */
export function pickedElement(runId: string | undefined): string | null {
  return picked !== null && picked.runId === runId ? picked.src : null;
}

export function onPickChange(listener: () => void): () => void {
  window.addEventListener(PICKED, listener);
  return () => {
    window.removeEventListener(PICKED, listener);
  };
}
