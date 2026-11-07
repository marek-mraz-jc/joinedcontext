import { Component, useEffect, useState, type ReactNode } from "react";
import { ProblemError } from "./client";
import { useOptionalClient } from "./hooks";
import { EntityPanel, EntitySelectionProvider, useEntitySelection } from "./panel";
import type { PanelSource } from "./panel";
import { reportError } from "./report";
import { sdkLanguage, sdkWord } from "./words";

/**
 * The one shell of every App (SDK-39): the title, the pages, the language switch, the reader's
 * name, and the loading, empty and error states in the same words and look everywhere. It holds
 * the entity selection, so any map, table, chart or card of a page opens the one entity panel.
 */

export interface ShellPage {
  id: string;
  label: string;
  render: () => ReactNode;
}

export interface ShellLanguage {
  code: string;
  label: string;
}

function initialPage(pages: ShellPage[], initial?: string): string {
  const fromHash = typeof window !== "undefined" ? window.location.hash.replace(/^#\/?/, "").split("?")[0] : "";
  if (pages.some((page) => page.id === fromHash)) return fromHash;
  if (initial && pages.some((page) => page.id === initial)) return initial;
  return pages[0]?.id ?? "";
}

export function AppShell({
  title,
  pages,
  actions,
  initial,
  languages,
  language,
  onLanguage,
  source,
  userName,
}: {
  title: string;
  pages: ShellPage[];
  actions?: ReactNode;
  initial?: string;
  /** The languages the App speaks; the switch shows when there are two or more. */
  languages?: ShellLanguage[];
  language?: string;
  onLanguage?: (code: string) => void;
  /** Where the entity panel reads and writes, for an App that goes through its own backend. */
  source?: PanelSource;
  /** The reader's name, for an App with no served configuration to carry it. */
  userName?: string;
}): React.JSX.Element {
  const client = useOptionalClient();
  const name = userName ?? client?.config.user?.name;
  const spoken = language ?? source?.language ?? client?.config.language;
  const words = sdkLanguage(spoken);
  const [active, setActive] = useState(() => initialPage(pages, initial));

  // The page names the language it shows and its own title (WCAG 3.1.1, 2.4.2): an App's
  // index.html carries one language and one title, the reader may have picked another (T-3577).
  useEffect(() => {
    if (spoken) document.documentElement.lang = spoken;
    document.title = title;
  }, [spoken, title]);

  useEffect(() => {
    // The colour scheme is the App's own stylesheet's (`color-scheme` beside its tokens): forcing
    // "light dark" here gave a light-only App dark native links and lists on a light surface.
    const onHash = () => {
      const id = window.location.hash.replace(/^#\/?/, "").split("?")[0];
      if (pages.some((page) => page.id === id)) setActive(id);
    };
    window.addEventListener("hashchange", onHash);
    return () => window.removeEventListener("hashchange", onHash);
  }, [pages]);

  const open = (id: string) => {
    setActive(id);
    try {
      window.history.replaceState(null, "", `#/${id}`);
    } catch {
      // A sandboxed preview may refuse; the page still changes.
    }
  };

  const page = pages.find((candidate) => candidate.id === active) ?? pages[0];

  return (
    <EntitySelectionProvider source={source} language={language}>
      <div className="jc-shell">
        <header className="jc-header">
          <h1>{title}</h1>
          {pages.length > 1 && (
            <nav aria-label={sdkWord(words, "nav.label")}>
              {pages.map((candidate) => (
                <button key={candidate.id} type="button" aria-current={candidate.id === page?.id ? "page" : undefined} onClick={() => open(candidate.id)}>
                  {candidate.label}
                </button>
              ))}
            </nav>
          )}
          {languages && languages.length > 1 && onLanguage && (
            // `data-jc-sdk`: the language list is the SDK's to test in every language, not each App's (T-4113).
            <label className="jc-language" data-jc-sdk="language">
              <span>{sdkWord(words, "language.label")}</span>
              <select value={language} onChange={(event) => onLanguage(event.target.value)}>
                {languages.map((candidate) => (
                  <option key={candidate.code} value={candidate.code}>
                    {candidate.label}
                  </option>
                ))}
              </select>
            </label>
          )}
          {actions}
          {name && <span className="jc-user">{name}</span>}
        </header>
        <main className="jc-main">
          {page ? <ErrorBoundary key={page.id}>{page.render()}</ErrorBoundary> : <Empty>{sdkWord(words, "nav.none")}</Empty>}
        </main>
        <EntityPanel />
      </div>
    </EntitySelectionProvider>
  );
}

/** The shell's language inside it, else the client's: the states speak what the page speaks. */
function useWords() {
  const client = useOptionalClient();
  const { language } = useEntitySelection();
  return sdkLanguage(language ?? client?.config.language);
}

export function Loading({ label }: { label?: string }): React.JSX.Element {
  const words = useWords();
  return (
    <div className="jc-loading" role="status" aria-live="polite">
      {label ?? sdkWord(words, "state.loading")}
    </div>
  );
}

export function Empty({ children }: { children?: ReactNode }): React.JSX.Element {
  const words = useWords();
  return <p className="jc-empty">{children ?? sdkWord(words, "state.empty")}</p>;
}

/** An error in words a person can act on: the problem's title and detail, and a retry when there is one. */
export function Problem({ error, onRetry }: { error: ProblemError | Error | null | undefined; onRetry?: () => void }): React.JSX.Element | null {
  const words = useWords();
  if (!error) return null;
  // A title that is a code (a broker's `ResourceNotFound`, `HTTP 404`) is no sentence for a
  // visitor: the status picks one, and the code's detail stays out with it (T-4200).
  const coded = error instanceof ProblemError && (!/\s/.test(error.title) || /^HTTP \d+$/.test(error.title));
  // A refusal is the SDK's sentence whatever the title, since a save is refused as well as a read
  // (T-4238): the gateway's Policy says "Access Denied by Policy" in English and no reason (GW6).
  // The broker's AccessDenied detail is its own English; an App's own reason, a sentence, is kept.
  const denied = error instanceof ProblemError && (error.status === 401 || error.status === 403);
  const title = !(error instanceof ProblemError)
    ? error.message
    : coded || denied
      ? sdkWord(words, error.status === 404 ? "problem.notFound" : denied ? "problem.denied" : "problem.failed")
      : error.title;
  const detail =
    error instanceof ProblemError &&
    (denied ? error.type !== "urn:antares:error:AccessDenied" && /\s.*[.!?]$/.test(error.detail ?? "") : !coded) &&
    error.detail &&
    error.detail !== error.title
      ? error.detail
      : undefined;
  return (
    <div className="jc-problem" role="alert">
      <strong>{title}</strong>
      {detail && <p>{detail}</p>}
      {/* Asking again never grants a right: a refusal offers no retry (T-4912). */}
      {onRetry && !denied && (
        <button type="button" onClick={onRetry}>
          {sdkWord(words, "state.retry")}
        </button>
      )}
    </div>
  );
}

/** A page that throws shows the problem and a retry instead of a blank App, and is reported. */
export class ErrorBoundary extends Component<{ children?: ReactNode }, { error: Error | null }> {
  override state: { error: Error | null } = { error: null };

  static getDerivedStateFromError(error: Error): { error: Error } {
    return { error };
  }

  override componentDidCatch(error: Error): void {
    reportError(error);
  }

  override render(): ReactNode {
    return this.state.error ? <Problem error={this.state.error} onRetry={() => this.setState({ error: null })} /> : this.props.children;
  }
}
