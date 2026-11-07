import type { ComponentType, ReactNode } from "react";
import { Component, StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { reportError } from "./report";
import { createClient, setClient } from "./client";
import type { Client } from "./client";
import { ConfigError, readConfig } from "./config";
import { JcProvider } from "./hooks";
import { applyTokens } from "./tokens";
import type { DesignTokens } from "./tokens";
import { transportFor } from "./transport";
import { startObserver } from "./observe";
import { startSelect } from "./select";

let errorListenersRegistered = false;

/**
 * The last line under the application: a render error no component of the application caught is
 * reported and shown as its message. The template's own `ErrorBoundary` catches before this one.
 */
class RootBoundary extends Component<{ children: ReactNode }, { error: Error | null }> {
  override state: { error: Error | null } = { error: null };

  static getDerivedStateFromError(error: Error): { error: Error } {
    return { error };
  }

  override componentDidCatch(error: Error): void {
    reportError(error);
  }

  override render(): ReactNode {
    return this.state.error ? <p role="alert">{this.state.error.message}</p> : this.props.children;
  }
}

/**
 * A published App framed by the Portal's Open page says it is up (AP-122, T-2941): the page cannot
 * see inside a frame of another origin, and a frame the browser refused (the realm's sign-in form,
 * which may not be framed) looks to it like one still loading. The message carries nothing but
 * its kind, so it goes to whichever page frames the App; the Portal checks that it came from its
 * own frame and the App's origin. Top-level, it is not sent.
 */
export function announceReady(win: Window | undefined = typeof window !== "undefined" ? window : undefined): void {
  if (!win || !win.parent || win.parent === win) {
    return;
  }
  try {
    win.parent.postMessage({ kind: "jc-ready" }, "*");
  } catch {
    // A parent that cannot be told keeps offering its own way back.
  }
}

export function startApp(
  App: ComponentType,
  options?: { tokens?: unknown; root?: HTMLElement; doc?: Document },
): { client: Client; tokens: DesignTokens } {
  const doc = options?.doc ?? (typeof document !== "undefined" ? document : undefined);

  let config;
  try {
    config = readConfig(doc);
  } catch (err) {
    if (err instanceof ConfigError) {
      const targetRoot = options?.root ?? doc?.getElementById("root");
      if (targetRoot) {
        createRoot(targetRoot).render(<p role="alert">{err.message}</p>);
      }
    }
    throw err;
  }

  const client = createClient(config, transportFor(config));
  setClient(client);

  const tokens = applyTokens(options?.tokens ?? {});

  const targetRoot = options?.root ?? doc?.getElementById("root");
  if (!targetRoot) {
    throw new Error("No #root element");
  }

  createRoot(targetRoot).render(
    <StrictMode>
      <JcProvider client={client}>
        <RootBoundary>
          <App />
        </RootBoundary>
      </JcProvider>
    </StrictMode>,
  );

  // A preview reads itself page by page when the host page asks, so the run can check what it shows (SDK-27).
  if (config.transport === "bridge") {
    startObserver({ doc });
    startSelect({ doc });
  } else {
    announceReady();
  }

  if (!errorListenersRegistered && typeof window !== "undefined") {
    errorListenersRegistered = true;
    window.addEventListener("error", (e: ErrorEvent) => {
      reportError(e.error ?? e.message);
    });
    window.addEventListener("unhandledrejection", (e: PromiseRejectionEvent) => {
      reportError(e.reason);
    });
  }

  return { client, tokens };
}
