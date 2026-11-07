import { Component, type ReactNode } from "react";
import { ProblemError, reportError } from "@joinedcontext/sdk";
import { t } from "../i18n";

export function Loading({ label }: { label?: string }): React.JSX.Element {
  return (
    <div className="jc-loading" role="status" aria-live="polite">
      {label ?? t("state.loading")}
    </div>
  );
}

export function Empty({ children }: { children?: ReactNode }): React.JSX.Element {
  return <p className="jc-empty">{children ?? t("state.empty")}</p>;
}

export function Problem({
  error,
  onRetry,
}: {
  error: ProblemError | Error | null | undefined;
  onRetry?: () => void;
}): React.JSX.Element | null {
  if (!error) return null;

  let title: string;
  let detail: string | undefined;

  if (error instanceof ProblemError) {
    title = error.title;
    detail = error.detail && error.detail !== error.title ? error.detail : undefined;
  } else {
    title = error.message;
    detail = undefined;
  }

  return (
    <div className="jc-problem" role="alert">
      <strong>{title}</strong>
      {detail && <p>{detail}</p>}
      {onRetry && (
        <button type="button" onClick={onRetry}>
          {t("state.retry")}
        </button>
      )}
    </div>
  );
}

export class ErrorBoundary extends Component<
  { children?: ReactNode; fallback?: (error: Error, reset: () => void) => ReactNode },
  { error: Error | null }
> {
  override state: { error: Error | null } = { error: null };

  static getDerivedStateFromError(error: Error): { error: Error } {
    return { error };
  }

  override componentDidCatch(error: Error): void {
    reportError(error);
  }

  reset = (): void => {
    this.setState({ error: null });
  };

  override render(): ReactNode {
    const { error } = this.state;
    if (error) {
      if (this.props.fallback) {
        return this.props.fallback(error, this.reset);
      }
      return <Problem error={error} onRetry={this.reset} />;
    }
    return this.props.children;
  }
}
