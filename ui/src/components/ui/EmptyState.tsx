import type { ReactNode } from "react";
import { clsx } from "clsx";
import { Icon } from "./icons";
import type { IconName } from "./icons";

export interface EmptyStateProps {
  title: ReactNode;
  /** One sentence on what would fill the page and how. */
  description?: ReactNode;
  icon?: IconName;
  /** The action that creates the first item, when the caller has one. */
  action?: ReactNode;
  /** Inside a table cell or a card the frame is already drawn; skip the dashed one. */
  bare?: boolean;
  /** The title as the page's own heading, when the empty state is all the page is (T-3150). */
  heading?: 1 | 2;
  className?: string;
}

/** What a list shows instead of nothing: why it is empty and what to do about it (UI-01). */
export function EmptyState({
  title,
  description,
  icon = "inbox",
  action,
  bare,
  heading,
  className,
}: EmptyStateProps): React.JSX.Element {
  const Title = heading === 1 ? "h1" : heading === 2 ? "h2" : "p";
  // A status, so a screen reader hears that the list is empty when it loads (T-1393).
  return (
    <div
      role="status"
      // What the page walkers look for (tests/pageChecks.ts, T-2730): an empty state says what to do next.
      data-empty-state=""
      className={clsx(
        "flex flex-col items-center justify-center gap-2 px-6 py-12 text-center",
        !bare && "rounded-lg border border-dashed border-border-strong bg-surface",
        className,
      )}
    >
      <span className="mb-1 inline-flex size-11 items-center justify-center rounded-full bg-primary-soft text-primary-soft-fg">
        <Icon name={icon} className="size-5" />
      </span>
      <Title className="text-body font-semibold text-fg">{title}</Title>
      {description ? <p className="max-w-md text-body text-fg-muted">{description}</p> : null}
      {action ? <div className="mt-3">{action}</div> : null}
    </div>
  );
}
