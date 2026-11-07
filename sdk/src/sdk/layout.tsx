// Layout primitives an application is built from (T-2777, SDK-12, UI-84): each one adapts to the
// width it is given, not to the window, so a Grid inside a Split or a framed app lays out for the
// room it has. The rules live in layout.css (container queries); no primitive measures in script.
import { useEffect, useId, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import "./layout.css";

type Level = 1 | 2 | 3;

function Heading({ level, children }: { level: Level; children: ReactNode }): React.JSX.Element {
  const Tag = `h${level}` as const;
  return <Tag>{children}</Tag>;
}

/** The content column of a screen: a gutter that grows with the width and a readable maximum. */
export function Page({
  children,
  width = "wide",
  label,
}: {
  children: ReactNode;
  /** `narrow` for forms and reading, `wide` for dashboards, `full` for a map or a desk. */
  width?: "narrow" | "wide" | "full";
  /** Names the region for a screen reader when the page has no heading of its own. */
  label?: string;
}): React.JSX.Element {
  return (
    <section className="jc-page" data-width={width} aria-label={label}>
      {children}
    </section>
  );
}

/** A screen's or a section's title, with its actions beside it; the actions wrap under it on a phone. */
export function Header({
  title,
  subtitle,
  actions,
  level = 2,
}: {
  title: ReactNode;
  subtitle?: ReactNode;
  actions?: ReactNode;
  level?: Level;
}): React.JSX.Element {
  return (
    <header className="jc-page-header">
      <div className="jc-page-heading">
        <Heading level={level}>{title}</Heading>
        {subtitle && <p>{subtitle}</p>}
      </div>
      {actions && <div className="jc-page-actions">{actions}</div>}
    </header>
  );
}

/**
 * A side panel beside the content: side by side when there is room, a drawer on a phone, opened by
 * a button that names it. Escape or the close button shuts the drawer and gives focus back.
 */
export function Sidebar({
  label,
  side,
  children,
  position = "start",
}: {
  /** What the panel holds ("Filters", "Layers"); the drawer's button and landmark carry it. */
  label: string;
  side: ReactNode;
  children: ReactNode;
  position?: "start" | "end";
}): React.JSX.Element {
  const [open, setOpen] = useState(false);
  const id = useId();
  const toggle = useRef<HTMLButtonElement>(null);
  const panel = useRef<HTMLElement>(null);
  const wasOpen = useRef(false);

  useEffect(() => {
    if (open) {
      panel.current?.focus();
    } else if (wasOpen.current) {
      toggle.current?.focus();
    }
    wasOpen.current = open;
  }, [open]);

  const onKeyDown = (event: KeyboardEvent) => {
    if (event.key === "Escape" && open) {
      event.stopPropagation();
      setOpen(false);
    }
  };

  return (
    <div className="jc-with-sidebar" data-position={position} data-open={open || undefined}>
      <button
        ref={toggle}
        type="button"
        className="jc-sidebar-toggle"
        aria-expanded={open}
        aria-controls={id}
        onClick={() => setOpen(true)}
      >
        {label}
      </button>
      <aside ref={panel} id={id} className="jc-sidebar" aria-label={label} tabIndex={-1} onKeyDown={onKeyDown}>
        <button type="button" className="jc-sidebar-close" onClick={() => setOpen(false)}>
          Close {label}
        </button>
        {side}
      </aside>
      {open && <div className="jc-sidebar-backdrop" aria-hidden="true" onClick={() => setOpen(false)} />}
      <div className="jc-sidebar-content">{children}</div>
    </div>
  );
}

/**
 * Tiles or cards in columns: one on a phone, then two, three and at most `columns` as the room it
 * is given grows (under 40rem, 40rem, 64rem, 90rem).
 */
export function Grid({ children, columns = 3 }: { children: ReactNode; columns?: 1 | 2 | 3 | 4 }): React.JSX.Element {
  return (
    <div className="jc-grid-box">
      <div className="jc-grid-cols" data-columns={columns}>
        {children}
      </div>
    </div>
  );
}

/** A bordered block with an optional title and actions: a chart, a list, a form section. */
export function Card({
  title,
  actions,
  children,
  level = 2,
  label,
}: {
  title?: ReactNode;
  actions?: ReactNode;
  children: ReactNode;
  level?: Level;
  /** Names the card for a screen reader when it has no title. */
  label?: string;
}): React.JSX.Element {
  return (
    <article className="jc-card" aria-label={label}>
      {(title || actions) && (
        <header className="jc-card-header">
          {title && <Heading level={level}>{title}</Heading>}
          {actions && <div className="jc-card-actions">{actions}</div>}
        </header>
      )}
      {children}
    </article>
  );
}

/** Two panes side by side when there is room (48rem), stacked on a phone, the first one on top. */
export function Split({
  children,
  ratio = "1:1",
}: {
  children: [ReactNode, ReactNode];
  /** The share of the first pane against the second. */
  ratio?: "1:1" | "2:1" | "1:2";
}): React.JSX.Element {
  return (
    <div className="jc-split-box">
      <div className="jc-split" data-ratio={ratio}>
        <div className="jc-split-pane">{children[0]}</div>
        <div className="jc-split-pane">{children[1]}</div>
      </div>
    </div>
  );
}

export interface Tab {
  id: string;
  label: string;
  render: () => ReactNode;
}

/**
 * Tabs by the WAI-ARIA pattern: arrow keys, Home and End move between them, only the chosen one is
 * in the tab order, and on a phone the row scrolls rather than wraps.
 */
export function Tabs({
  tabs,
  label,
  initial,
  onChange,
}: {
  tabs: Tab[];
  /** Names the tab list ("Views of the stations"). */
  label: string;
  initial?: string;
  onChange?: (id: string) => void;
}): React.JSX.Element | null {
  const [chosen, setChosen] = useState(() => (tabs.some((tab) => tab.id === initial) ? initial : tabs[0]?.id));
  const base = useId();
  const buttons = useRef(new Map<string, HTMLButtonElement>());
  const active = tabs.find((tab) => tab.id === chosen) ?? tabs[0];
  if (!active) return null;

  const choose = (id: string, focus: boolean) => {
    setChosen(id);
    onChange?.(id);
    if (focus) buttons.current.get(id)?.focus();
  };

  const onKeyDown = (event: KeyboardEvent) => {
    const at = tabs.findIndex((tab) => tab.id === active.id);
    const to =
      event.key === "ArrowRight"
        ? (at + 1) % tabs.length
        : event.key === "ArrowLeft"
          ? (at - 1 + tabs.length) % tabs.length
          : event.key === "Home"
            ? 0
            : event.key === "End"
              ? tabs.length - 1
              : -1;
    if (to < 0) return;
    event.preventDefault();
    choose(tabs[to].id, true);
  };

  return (
    <div className="jc-tabs">
      <div className="jc-tablist" role="tablist" aria-label={label} onKeyDown={onKeyDown}>
        {tabs.map((tab) => (
          <button
            key={tab.id}
            ref={(node) => {
              if (node) buttons.current.set(tab.id, node);
              else buttons.current.delete(tab.id);
            }}
            type="button"
            role="tab"
            id={`${base}-${tab.id}-tab`}
            aria-controls={`${base}-${tab.id}-panel`}
            aria-selected={tab.id === active.id}
            tabIndex={tab.id === active.id ? 0 : -1}
            onClick={() => choose(tab.id, false)}
          >
            {tab.label}
          </button>
        ))}
      </div>
      <div
        className="jc-tabpanel"
        role="tabpanel"
        id={`${base}-${active.id}-panel`}
        aria-labelledby={`${base}-${active.id}-tab`}
        tabIndex={0}
      >
        {active.render()}
      </div>
    </div>
  );
}
