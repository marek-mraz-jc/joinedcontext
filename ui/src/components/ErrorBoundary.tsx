import { Component, useEffect } from "react";
import type { ErrorInfo, JSX, ReactNode } from "react";
import i18n from "../i18n";
import { Button, buttonClass } from "./ui/Button";

/**
 * UI-45, UI-46: the page a render error leaves behind.
 *
 * Without one of these React unmounts the whole tree, and a person is left with a white page and
 * nothing to do or to report. This shows words, a reference they can quote, and the two things
 * that ever help: load the page again, or go back to the start.
 *
 * What it does not show is as much of the point. An error message carries whatever threw it — a
 * URL with a token in its query, a response body, a stack naming the build — so none of it
 * reaches the page. The reference is the tie to the console line, where a developer with the
 * browser open already has the real error.
 */

/** i18n may itself be what failed; the English stays as the floor under it. */
function say(key: string, fallback: string): string {
  try {
    return i18n.t(key, { defaultValue: fallback });
  } catch {
    return fallback;
  }
}

/** Short enough to read out over a telephone, long enough not to collide within a day. */
export function errorReference(): string {
  try {
    return crypto.randomUUID().slice(0, 8);
  } catch {
    return Date.now().toString(36);
  }
}

/**
 * T-4201: a code-split chunk that would not load. After a deploy the old chunk names are gone,
 * and a dropped or rate-limited request looks the same to the browser. Loading the page again
 * is the cure, so the panel says that instead of "this page stopped".
 */
export function isChunkLoadError(error: unknown): boolean {
  if (!(error instanceof Error)) return false;
  return (
    error.name === "ChunkLoadError" ||
    /Failed to fetch dynamically imported module|error loading dynamically imported module|Importing a module script failed|Unable to preload CSS/i.test(
      error.message,
    )
  );
}

/** When the page last reloaded itself for a chunk, in this tab (sessionStorage). */
const CHUNK_RELOADED_AT = "jc.chunkReloadedAt";
/** One reload per minute: a chunk still missing after it is a real outage, not a deploy. */
const CHUNK_RELOAD_ONCE_MS = 60_000;

function tabStorage(): Pick<Storage, "getItem" | "setItem"> | undefined {
  try {
    return window.sessionStorage;
  } catch {
    return undefined;
  }
}

/**
 * T-4949: whether a chunk error will reload the page by itself. A tab opened before a rollout asks
 * for chunk names the new release no longer has; one reload fetches the new index and its
 * chunks. A second failure within the minute shows the panel instead, so a real outage never
 * loops. Pure: `reloadForChunk` is the one that writes.
 */
export function mayReloadForChunk(error: unknown, storage = tabStorage(), now = Date.now()): boolean {
  if (!isChunkLoadError(error) || !storage) return false;
  try {
    return now - Number(storage.getItem(CHUNK_RELOADED_AT) ?? 0) >= CHUNK_RELOAD_ONCE_MS;
  } catch {
    return false;
  }
}

/** Reloads the page once for a missing chunk; `false` when it already did within the minute. */
export function reloadForChunk(
  storage = tabStorage(),
  reload = () => window.location.reload(),
  now = Date.now(),
): boolean {
  if (!storage) return false;
  try {
    if (now - Number(storage.getItem(CHUNK_RELOADED_AT) ?? 0) < CHUNK_RELOAD_ONCE_MS) return false;
    storage.setItem(CHUNK_RELOADED_AT, String(now));
  } catch {
    return false;
  }
  reload();
  return true;
}

interface State {
  reference: string | null;
  chunk: boolean;
}

export interface ErrorBoundaryProps {
  children: ReactNode;
  /**
   * What to show instead of the whole-page panel. The Portal's chrome uses it to keep the
   * header, the sidebar and the project switcher up while one page is down (T-2426): a person
   * whose page failed can still go somewhere else, which is the one thing they will want.
   */
  fallback?: (reference: string, retry: () => void) => ReactNode;
  /**
   * A value that means "this is a different page now". React never clears a boundary by itself,
   * so without this the panel would survive every navigation the chrome still allows.
   */
  resetKey?: string;
}

export class ErrorBoundary extends Component<ErrorBoundaryProps, State> {
  override state: State = { reference: null, chunk: false };

  static getDerivedStateFromError(error: unknown): State {
    return { reference: errorReference(), chunk: isChunkLoadError(error) };
  }

  override componentDidCatch(error: Error, info: ErrorInfo): void {
    // A missing chunk after a rollout reloads the page (ChunkFailed): nothing went wrong to log.
    if (mayReloadForChunk(error)) return;
    // The console is the developer's surface, not the page's: React logs the error and the
    // component stack there already, and this ties them to what the person is reading.
    console.error(`Portal error ${this.state.reference ?? ""}`, error, info.componentStack);
  }

  override componentDidUpdate(previous: ErrorBoundaryProps): void {
    if (this.state.reference !== null && previous.resetKey !== this.props.resetKey) {
      this.retry();
    }
  }

  private readonly retry = (): void => {
    this.setState({ reference: null, chunk: false });
  };

  override render(): ReactNode {
    const { reference: id, chunk } = this.state;
    if (id === null) {
      return this.props.children;
    }
    if (chunk) {
      return <ChunkFailed />;
    }
    return this.props.fallback ? this.props.fallback(id, this.retry) : <ErrorPage reference={id} />;
  }
}

/** The whole-page panel: what is left when the chrome itself could not be drawn. */
export function ErrorPage({ reference: id }: { reference: string }): JSX.Element {
  return (
    <main className="mx-auto flex max-w-lg flex-col gap-4 p-8" role="alert" aria-live="assertive">
      <h1 className="text-xl font-semibold">{say("app.error.crashTitle", "This page stopped")}</h1>
      <p className="text-fg-muted">
        {say(
          "app.error.crashBody",
          "Something in the Portal failed while drawing this page. Nothing you were looking at was changed.",
        )}
      </p>
      <p className="text-sm text-fg-muted">
        {say("app.error.crashReference", "Quote this when you report it:")}{" "}
        <code data-testid="error-reference">{id}</code>
      </p>
      <div className="flex gap-2">
        <Button
          variant="primary"
          onClick={() => {
            window.location.reload();
          }}
        >
          {say("app.error.crashReload", "Load the page again")}
        </Button>
        <a className={buttonClass("secondary")} href="/">
          {say("app.error.crashHome", "Go to the start")}
        </a>
      </div>
    </main>
  );
}

/** A part of the Portal did not load: nothing failed in it, the page only needs loading again. */
export function ChunkFailed(): JSX.Element {
  useEffect(() => {
    reloadForChunk();
  }, []);
  return (
    <main className="mx-auto flex max-w-lg flex-col gap-4 p-8" role="alert" aria-live="assertive">
      <h1 className="text-xl font-semibold">{say("app.error.chunkTitle", "Part of the Portal did not load")}</h1>
      <p className="text-fg-muted">
        {say(
          "app.error.chunkBody",
          "The connection dropped or the Portal was just updated. Load the page again; nothing you were looking at was changed.",
        )}
      </p>
      <div>
        <Button
          variant="primary"
          onClick={() => {
            window.location.reload();
          }}
        >
          {say("app.error.chunkReload", "Load the page again")}
        </Button>
      </div>
    </main>
  );
}
