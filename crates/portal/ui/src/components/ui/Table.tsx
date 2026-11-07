import { useLayoutEffect, useRef, useState } from "react";
import type { HTMLAttributes, MouseEvent, ReactNode, TdHTMLAttributes, ThHTMLAttributes } from "react";
import { clsx } from "clsx";
import { Icon } from "./icons";
import { Skeleton } from "./Skeleton";

export interface TableProps extends HTMLAttributes<HTMLTableElement> {
  /** Read by a screen reader as the table's name; visually hidden. */
  caption: string;
  /** Every second row tinted; on by default. */
  zebra?: boolean;
  /** Announced to a screen reader while the rows are a skeleton: "Loading endpoints". */
  status?: string;
  /**
   * A Tailwind max-height class (`max-h-64`) for a table whose length is the caller's data: the
   * frame then scrolls vertically as well, so a long list scrolls in place instead of growing
   * the page. The frame is one focusable, named stop either way (WCAG 2.1.1).
   */
  maxHeight?: string;
  /**
   * Below 640 px each row becomes a card of label and value lines (`table-stack`), for a table
   * whose columns all matter on a phone. Give every `TableCell` after the first a `data-label`
   * naming its column.
   */
  stack?: boolean;
}

/**
 * A data table inside a scrolling frame: the frame scrolls on a phone while the page stays
 * put, the header stays legible, and the caption names it for a screen reader (UI-01).
 */
export function Table({
  caption,
  zebra = true,
  status,
  maxHeight,
  stack,
  className,
  children,
  ...rest
}: TableProps): React.JSX.Element {
  const frame = useRef<HTMLDivElement>(null);
  const more = useColumnsBeyond(frame);
  return (
    // `min-w-0`: as a flex or grid item this wrapper has visible overflow, so its automatic
    // minimum width is the table's and it pushed the page sideways (model diagram at 375 px).
    <div className="relative min-w-0">
      {/* The frame scrolls sideways, so it is focusable and a keyboard scrolls it with the arrow
          keys (WCAG 2.1.1): `overflow-x-auto` alone left the columns past the right edge reachable
          with a pointer and by nothing else. It is named after the caption, or it would be an
          unlabelled stop in the tab order.

          `group`, not `region`: a region is a landmark, and most tables here sit inside a
          `<section aria-labelledby>` whose name is the caption — two nested landmarks with one
          name, which is the `landmark-unique` violation and an ambiguous query for every test that
          asks for that section by name. A group names the tab stop and adds no landmark.

          `relative`: the frame is the containing block of the `sr-only` labels in the cells. They are
          positioned absolutely, and an absolute box is clipped only by a scroller that contains its
          containing block, so without it a header's hidden "Actions" stood at the table's far right
          and the whole page scrolled sideways at 375 px (T-3088). */}
      <div
        ref={frame}
        role="group"
        aria-label={caption}
        tabIndex={0}
        className={clsx(
          // `scroll-hint-x` is the frame's surface with a shadow at its right edge while columns
          // wait beyond it (T-4265, photo audit T-4160).
          "focus-ring scroll-hint-x relative overflow-x-auto rounded-lg border border-border shadow-1",
          maxHeight && ["overflow-y-auto", maxHeight],
        )}
      >
        {status ? (
          <p role="status" className="sr-only">
            {status}
          </p>
        ) : null}
        <table
          aria-busy={status ? true : undefined}
          className={clsx(
            "w-full border-collapse text-left text-body",
            // `:not(:hover)` keeps the stripe off the row under the pointer. Without it the zebra
            // rule — two selectors deep, so more specific than the row's own `hover:` — won an
            // even row's background outright and hovering half the table did nothing.
            zebra && "[&>tbody>tr:nth-child(even):not(:hover)]:bg-surface-subtle/70",
            // Below lg the first column, the row's name, stays at the left edge while the frame
            // scrolls sideways: a row scrolled to its actions still says which record it is
            // (T-4165, photo audit T-4159), at a tablet's width too (T-4265, photo audit T-4160
            // #44). Only that cell is opaque, in its row's zebra and hover, so the frame's edge
            // shadow shows through the rest of the row.
            "max-lg:[&_tr>:first-child]:sticky max-lg:[&_tr>:first-child]:left-0 max-lg:[&_tr>:first-child]:z-10",
            "max-lg:[&_tbody>tr>:first-child]:bg-surface max-lg:[&_thead>tr>:first-child]:bg-surface-subtle",
            zebra && "max-lg:[&_tbody>tr:nth-child(even)>:first-child]:bg-surface-subtle",
            "max-lg:[&_tbody>tr:hover>:first-child]:bg-primary-50",
            stack && "table-stack",
            className,
          )}
          {...rest}
        >
          <caption className="sr-only">{caption}</caption>
          {children}
        </table>
      </div>
      {/* The cue that columns wait past the right edge (T-4992): the frame's own edge shadow sat
          under the rows' zebra and hover fills and showed on no phone. This one lies over the
          rows and leaves once the frame is scrolled to its end. A pointer passes through it, and
          a screen reader reaches every column without it. */}
      {more ? (
        <span
          aria-hidden="true"
          data-testid="table-more-columns"
          className="pointer-events-none absolute inset-y-px right-px flex w-10 items-start justify-end rounded-r-lg scroll-cue-x pr-1 pt-2.5"
        >
          <Icon name="chevronRight" className="size-4 text-fg-muted" />
        </span>
      ) : null}
    </div>
  );
}

/** Whether the frame holds columns past its right edge, kept current on scroll and resize. */
function useColumnsBeyond(frame: React.RefObject<HTMLDivElement | null>): boolean {
  const [more, setMore] = useState(false);
  useLayoutEffect(() => {
    const element = frame.current;
    if (!element) return;
    const measure = () => setMore(element.scrollLeft + element.clientWidth < element.scrollWidth - 1);
    measure();
    element.addEventListener("scroll", measure, { passive: true });
    const observer = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(measure);
    observer?.observe(element);
    // The rows change the width too: a table that loads its rows grows after the first paint.
    const table = element.querySelector("table");
    if (table) observer?.observe(table);
    return () => {
      element.removeEventListener("scroll", measure);
      observer?.disconnect();
    };
  }, [frame]);
  return more;
}

export function TableHead({
  className,
  children,
  ...rest
}: HTMLAttributes<HTMLTableSectionElement>): React.JSX.Element {
  return (
    <thead className={clsx("bg-surface-subtle", className)} {...rest}>
      <tr className="border-b border-border">{children}</tr>
    </thead>
  );
}

export function TableBody({
  className,
  children,
  ...rest
}: HTMLAttributes<HTMLTableSectionElement>): React.JSX.Element {
  return (
    <tbody className={clsx("divide-y divide-border", className)} {...rest}>
      {children}
    </tbody>
  );
}

/** What handles its own click inside a row: the row never takes that click over. */
const OWN_CLICK =
  "a, button, input, select, textarea, label, summary, [role=button], [role=menuitem], [role=checkbox], [role=switch], [role=tab], [contenteditable=true]";

/**
 * A click anywhere on a row whose record has a link (`data-row-link`, see `RecordLink`) opens
 * that link, as the owner's rule asks: every record opens on click (T-2875). The link itself
 * stays the one control of the row — the keyboard reaches it with Tab and opens it with Enter,
 * and a focusable row holding buttons would be a nested interactive control (axe). What handles
 * its own click is left alone: a button, a menu, a checkbox, another link, the dialogs and menus
 * a row renders through portals (their events bubble through React but not through the DOM),
 * and a click that ends a text selection. Ctrl, Cmd, Shift and the middle button reach the link
 * as they would on the link itself: a new tab. A `Card` holding a record's link follows the same
 * rule.
 */
export function openRowLink(event: MouseEvent<HTMLElement>): void {
  const row = event.currentTarget;
  const target = event.target;
  if (!(target instanceof Element) || !row.contains(target)) {
    return;
  }
  const own = target.closest(OWN_CLICK);
  if (own && row.contains(own)) {
    return;
  }
  if (window.getSelection()?.toString()) {
    return;
  }
  const link = row.querySelector<HTMLAnchorElement>("a[data-row-link]");
  if (!link) {
    return;
  }
  // The click is the link's own, with the keys held: the router takes a plain one in place and
  // leaves a modified one to the browser, which opens the tab (and the link's `rel` holds). The
  // middle button is a new tab, so it is sent as the key that means one.
  const middle = event.button === 1;
  link.dispatchEvent(
    new window.MouseEvent("click", {
      bubbles: true,
      cancelable: true,
      ctrlKey: event.ctrlKey || middle,
      metaKey: event.metaKey || middle,
      shiftKey: event.shiftKey,
      altKey: event.altKey,
    }),
  );  // Handled: a card around the row does not open its own link as well.
  event.preventDefault();
}

/**
 * A record's card that is not a `Card` (a list item, an article) takes a click the way a row does:
 * spread these on it and give its name a record link (T-2875).
 */
export const recordCard = {
  className: "has-[a[data-row-link]]:cursor-pointer",
  onClick: (event: MouseEvent<HTMLElement>): void => {
    if (!event.defaultPrevented) {
      openRowLink(event);
    }
  },
  onAuxClick: (event: MouseEvent<HTMLElement>): void => {
    if (!event.defaultPrevented && event.button === 1) {
      openRowLink(event);
    }
  },
};

export function TableRow({
  className,
  children,
  onClick,
  onAuxClick,
  ...rest
}: HTMLAttributes<HTMLTableRowElement>): React.JSX.Element {
  return (
    <tr
      className={clsx(
        "transition-colors hover:bg-primary-50 has-[a[data-row-link]]:cursor-pointer",
        className,
      )}
      onClick={(event) => {
        onClick?.(event);
        if (!event.defaultPrevented) {
          openRowLink(event);
        }
      }}
      onAuxClick={(event) => {
        onAuxClick?.(event);
        if (!event.defaultPrevented && event.button === 1) {
          openRowLink(event);
        }
      }}
      {...rest}
    >
      {children}
    </tr>
  );
}

export interface TableHeaderCellProps extends ThHTMLAttributes<HTMLTableCellElement> {
  align?: "left" | "right" | "center";
  /** A column a phone does without: hidden below `sm`, there from `sm` up (UI-27). */
  secondary?: boolean;
}

export function TableHeaderCell({
  align = "left",
  secondary,
  className,
  children,
  ...rest
}: TableHeaderCellProps): React.JSX.Element {
  return (
    <th
      scope="col"
      className={clsx(
        // No `whitespace-nowrap`: a translated header is the longest text in its column
        // ("Zuletzt geändert von" against "Changed by") and holding it on one line pushed the
        // last column off the screen instead of letting the header wrap to two lines.
        "px-4 py-2.5 text-caption font-semibold uppercase tracking-wide text-fg-muted",
        align === "right" && "text-right",
        align === "center" && "text-center",
        secondary && "hidden sm:table-cell",
        className,
      )}
      {...rest}
    >
      {children}
    </th>
  );
}

export interface TableCellProps extends TdHTMLAttributes<HTMLTableCellElement> {
  align?: "left" | "right" | "center";
  /** The cell's content is the row's name: bolder, and first to be read. */
  primary?: boolean;
  /**
   * A cell of a column a phone does without (UI-27). Hidden below `sm` — with the same
   * `secondary` on its header cell — so a row at 400 px is the name, the state and what one
   * can do with it, instead of five columns of which two are cut off the screen.
   */
  secondary?: boolean;
}

export function TableCell({
  align = "left",
  primary,
  secondary,
  className,
  children,
  ...rest
}: TableCellProps): React.JSX.Element {
  return (
    <td
      className={clsx(
        "px-4 py-3 align-top text-fg",
        primary && "font-medium",
        align === "right" && "text-right",
        align === "center" && "text-center",
        secondary && "hidden sm:table-cell",
        className,
      )}
      {...rest}
    >
      {children}
    </td>
  );
}

/**
 * The cell that names its own row (`<th scope="row">`), for a table whose first column is the
 * subject and not a value: a permissions matrix, a sample of entities, a field-by-field diff.
 * Without it a page that needed one styled a `<th>` by hand and drifted from every other cell
 * — the padding, the alignment and the `secondary` behaviour are the `TableCell` ones, because
 * it is a cell that happens to be a header, not a column header in the wrong place.
 */
export function TableRowHeaderCell({
  align = "left",
  secondary,
  className,
  children,
  ...rest
}: TableCellProps & ThHTMLAttributes<HTMLTableCellElement>): React.JSX.Element {
  return (
    <th
      scope="row"
      className={clsx(
        "px-4 py-3 text-left align-top font-medium text-fg",
        align === "right" && "text-right",
        align === "center" && "text-center",
        secondary && "hidden sm:table-cell",
        className,
      )}
      {...rest}
    >
      {children}
    </th>
  );
}

/** The one row of an empty table: the message, and whatever action fills it. */
export function TableEmpty({
  columns,
  children,
}: {
  columns: number;
  children: ReactNode;
}): React.JSX.Element {
  return (
    <tr>
      <td colSpan={columns} className="px-4 py-12 text-center text-body text-fg-muted">
        {/* On a phone the head can be wider than the screen and the table scrolls: the message
            stays in view, the width of the screen less the page's gutters, instead of centred
            on a head nobody sees whole (owner 2026-10-30, Subscriptions at 400 px). */}
        <div className="sticky start-4 max-w-[calc(100vw-4.5rem)] sm:static sm:max-w-none">{children}</div>
      </td>
    </tr>
  );
}

/** Rows of grey bars while the list is on its way; the Table's `status` announces the wait. */
export function TableSkeleton({
  columns,
  rows = 4,
}: {
  columns: number;
  rows?: number;
}): React.JSX.Element {
  return (
    <tbody aria-hidden="true" className="divide-y divide-border">
      {Array.from({ length: rows }, (_, row) => (
        <tr key={row}>
          {Array.from({ length: columns }, (_, col) => (
            // A stacked table hides its header below `sm`: the skeleton shows one bar per row there,
            // never a row of bars wider than the phone (T-6169).
            <td key={col} className={col === 0 ? "px-4 py-3.5" : "hidden px-4 py-3.5 sm:table-cell"}>
              <Skeleton className={col === 0 ? "h-4 w-full max-w-40" : "h-4 w-24"} />
            </td>
          ))}
        </tr>
      ))}
    </tbody>
  );
}
