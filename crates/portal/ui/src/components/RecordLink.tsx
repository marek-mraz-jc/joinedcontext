import type { ReactNode } from "react";
import { Link } from "@tanstack/react-router";
import { clsx } from "clsx";
import { useFormRoute } from "./forms/FormRoute";
import { safeHref } from "./ui";

/** How a record link looks; a card whose link goes elsewhere (an App opens itself) wears the same. */
export const RECORD_LINK_STYLE = "focus-ring touch-target text-primary-soft-fg underline-offset-2 hover:underline";

/** Kinds with a page of their own (the router's DETAIL_PAGES); every other kind opens on its edit form. */
const DETAIL = new Set(["spaces", "endpoints", "apps", "csrs", "pipelines", "mqttbrokers"]);

/**
 * The link to one record: its own page where it has one, else its edit form (T-2875). It is also
 * the row's link — `TableRow` opens it on a click anywhere in the row — so each row renders one.
 */
export function RecordLink({
  project,
  plural,
  name,
  className,
  children,
}: {
  project: string;
  plural: string;
  name: string;
  className?: string;
  children?: ReactNode;
}): React.JSX.Element {
  const style = clsx(RECORD_LINK_STYLE, className);
  return DETAIL.has(plural) ? (
    <Link data-row-link="" to="/projects/$project/$plural/$name" params={{ project, plural, name }} className={style}>
      {children ?? name}
    </Link>
  ) : (
    <Link data-row-link="" to="/projects/$project/$plural/$name/edit" params={{ project, plural, name }} className={style}>
      {children ?? name}
    </Link>
  );
}

/**
 * The link to one record's edit form on a page that hosts its forms at an address of its own (a
 * tab of Project settings or of the Organization): `{tab}/{name}/edit`. A plain click goes there
 * through the router; a modified one is the browser's, a new tab. Outside such a page it is the
 * record's name, since there is no address to go to.
 */
export function FormRecordLink({
  name,
  className,
  children,
}: {
  name: string;
  className?: string;
  children?: ReactNode;
}): React.JSX.Element {
  const route = useFormRoute();
  if (!route) {
    return <>{children ?? name}</>;
  }
  return (
    <LocalRecordLink href={route.editHref(name)} onOpen={() => route.openEdit(name)} className={className}>
      {children ?? name}
    </LocalRecordLink>
  );
}

/**
 * A record link on a page that opens the record in place (local state, a form or a detail pane).
 * The address is one the page's route reads back, so a plain click calls `onOpen` and a modified
 * one (Ctrl, Cmd, Shift, Alt, middle) is the browser's: the record's address in a new tab.
 */
export function LocalRecordLink({
  href,
  onOpen,
  className,
  children,
}: {
  href: string;
  onOpen: () => void;
  className?: string;
  children: ReactNode;
}): React.JSX.Element {
  return (
    <a
      data-row-link=""
      href={safeHref(href)}
      className={clsx(RECORD_LINK_STYLE, className)}
      onClick={(event) => {
        if (event.button !== 0 || event.ctrlKey || event.metaKey || event.shiftKey || event.altKey) {
          return;
        }
        event.preventDefault();
        onOpen();
      }}
    >
      {children}
    </a>
  );
}
