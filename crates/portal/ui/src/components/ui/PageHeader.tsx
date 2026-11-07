import { useEffect, useId, useRef, useState } from "react";
import type { ReactNode } from "react";
import { clsx } from "clsx";
import { useTranslation } from "react-i18next";
import { setPageTitle } from "../../documentTitle";
import { Icon } from "./icons";

/** The address the last header mounted on; a header mounting on another one is a route change. */
let lastPath: string | null = null;

/** The project of the address, when the segment is a name and nothing else (UI-16). */
function projectOf(pathname: string): string | undefined {
  return /^\/projects\/([a-z0-9][a-z0-9-]*)(?:\/|$)/.exec(pathname)?.[1];
}

export interface PageHeaderProps {
  title: ReactNode;
  /** One line under the title on what the page manages. */
  description?: ReactNode;
  /** The breadcrumb trail, rendered above the title; the shell provides it. */
  breadcrumb?: ReactNode;
  /** The page's actions; the first is the only primary on the page. */
  actions?: ReactNode;
  /** A small control right beside the title, such as a form's User Guide link (T-6105). */
  titleAction?: ReactNode;
  /** Anything that sits beside the actions: a quota bar, a filter. */
  aside?: ReactNode;
  /** A line under the description, such as why the actions are greyed (T-4263). */
  note?: ReactNode;
  className?: string;
  /**
   * A list page (T-6095): the title, its description on hover or focus, the search and the actions
   * share one line, so the grid starts right below. Without it the description is a line of its own.
   */
  compact?: boolean;
  /** The list's search box; a compact header puts it right of the title, before the actions. */
  search?: ReactNode;
}

/**
 * The description of a compact header: an info mark that shows the text on hover or keyboard focus.
 * The text is always in the document (`aria-describedby`), so a screen reader hears it with the mark.
 * Below `sm` the open text is a line of its own under the title, so it never covers the search or the actions.
 */
function DescriptionHint({ children }: { children: ReactNode }): React.JSX.Element {
  const { t } = useTranslation();
  const id = useId();
  const [open, setOpen] = useState(false);
  return (
    <span className="contents sm:relative sm:inline-flex">
      <button
        type="button"
        aria-label={t("pageHeader.about")}
        title={t("pageHeader.about")}
        aria-describedby={id}
        aria-expanded={open}
        className="focus-ring inline-flex size-control-xs items-center justify-center rounded-full text-fg-muted hover:text-fg"
        onMouseEnter={() => setOpen(true)}
        onMouseLeave={() => setOpen(false)}
        onFocus={() => setOpen(true)}
        onBlur={() => setOpen(false)}
        // A touch screen has no hover: a tap shows and hides the text.
        onClick={() => setOpen(!open)}
        onKeyDown={(event) => {
          if (event.key === "Escape" && open) {
            event.stopPropagation();
            setOpen(false);
          }
        }}
      >
        <Icon name="info" className="size-4" />
      </button>
      <span
        id={id}
        role="tooltip"
        className={clsx(
          open
            ? "basis-full rounded-md border border-border bg-surface p-2 text-caption font-normal text-fg sm:absolute sm:left-0 sm:top-full sm:z-50 sm:mt-1 sm:w-72 sm:basis-auto sm:shadow-2"
            : "sr-only",
        )}
      >
        {children}
      </span>
    </span>
  );
}

/** The top of every page: what it is, where it is, and what one can do on it (UI-01). */
export function PageHeader({
  title,
  description,
  breadcrumb,
  actions,
  titleAction,
  aside,
  note,
  className,
  compact,
  search,
}: PageHeaderProps): React.JSX.Element {
  const heading = useRef<HTMLHeadingElement>(null);

  // The tab, the history and the task switcher name the page and the project (UI-15). A title
  // that is not a string is a node this component cannot read, and the installation's name —
  // which `documentTitle` already holds — stays the whole title rather than a wrong page name.
  useEffect(() => {
    if (typeof title === "string") {
      setPageTitle([title, projectOf(window.location.pathname)]);
    }
  }, [title]);

  // A route change moves focus to the heading, so a screen reader says where one has arrived
  // (UI-16). The first page of a visit and a header that mounts again on the same address
  // (loading, then loaded) take nothing from whoever holds the focus.
  useEffect(() => {
    const path = window.location.pathname;
    if (lastPath !== null && lastPath !== path) {
      heading.current?.focus();
    }
    lastPath = path;
  }, []);

  const h1 = (
    <h1 ref={heading} tabIndex={-1} className="focus-ring rounded-sm text-display font-semibold tracking-tight text-fg">{title}</h1>
  );

  // A div, not a <header>: the shell's top bar is the one banner landmark of the page.
  if (compact) {
    return (
      <div className={clsx("flex flex-col gap-2", className)}>
        {breadcrumb}
        <div className="flex flex-wrap items-center gap-x-4 gap-y-2">
          <div className="flex w-full min-w-0 flex-wrap items-center gap-1 sm:w-auto">
            {h1}
            {description ? <DescriptionHint>{description}</DescriptionHint> : null}
            {titleAction}
          </div>
          <div className="ms-auto flex flex-wrap items-center gap-2">
            {/* A control fills its parent: the slot gives the search its width. */}
            {search ? <div className="w-full sm:w-64">{search}</div> : null}
            {aside}
            {actions}
          </div>
        </div>
        {note}
      </div>
    );
  }
  return (
    <div className={clsx("flex flex-col gap-3", className)}>
      {breadcrumb}
      {/* Beside a title control the actions keep to the title's row, so a description opening below never moves them. */}
      <div className={clsx("flex flex-wrap justify-between gap-x-6 gap-y-3", titleAction ? "items-start" : "items-end")}>
        {/* The description <p> stays the h1's next sibling (screen reader order, walker checks); a grid puts titleAction beside the title. */}
        <div className={clsx("min-w-0", titleAction && "grid grid-cols-[auto_1fr] items-center gap-x-1")}>
          {h1}
          {description ? <p className={clsx("mt-1 max-w-prose text-body text-fg-muted", titleAction && "col-span-2")}>{description}</p> : null}
          {titleAction ? (
            <>
              {note ? <div className="col-span-2">{note}</div> : null}
              <span className="col-start-2 row-start-1 flex">{titleAction}</span>
            </>
          ) : (
            note
          )}
        </div>
        {actions || aside ? (
          <div className="flex flex-wrap items-end gap-x-6 gap-y-3">
            {aside}
            {actions ? <div className="flex flex-wrap items-center gap-2">{actions}</div> : null}
          </div>
        ) : null}
      </div>
    </div>
  );
}
