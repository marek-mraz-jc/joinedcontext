import { useRef } from "react";
import type { KeyboardEvent, ReactNode } from "react";
import { clsx } from "clsx";

export interface TabItem<T extends string> {
  value: T;
  label: ReactNode;
}

export interface TabsProps<T extends string> {
  /** Prefix of the tab and panel ids, so `tabPanelProps(id, value)` names the same pair. */
  id: string;
  /** What the tabs switch between; the tab list's accessible name. */
  label: string;
  tabs: TabItem<T>[];
  value: T;
  onChange: (value: T) => void;
  /** `line` for the views of a page, `pill` for tabs nested inside one of them. */
  variant?: "line" | "pill";
  className?: string;
}

/** The attributes of the panel a tab shows, wired to the tab that labels it. */
export function tabPanelProps(id: string, value: string) {
  return {
    role: "tabpanel" as const,
    id: `${id}-panel-${value}`,
    "aria-labelledby": `${id}-tab-${value}`,
  };
}

/**
 * The one tab list of the Portal (UI-16): one tab stop, the arrows, Home and End move between
 * tabs and select as they go, and each tab names the panel it controls. The caller renders the
 * panel and spreads `tabPanelProps` on it. No Radix Tabs in this bundle, and the pattern is a
 * row of buttons with a roving tabindex.
 */
export function Tabs<T extends string>({
  id,
  label,
  tabs,
  value,
  onChange,
  variant = "line",
  className,
}: TabsProps<T>): React.JSX.Element {
  const list = useRef<HTMLDivElement>(null);

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const at = tabs.findIndex((tab) => tab.value === value);
    const last = tabs.length - 1;
    const to =
      event.key === "ArrowRight"
        ? (at + 1) % tabs.length
        : event.key === "ArrowLeft"
          ? (at + last) % tabs.length
          : event.key === "Home"
            ? 0
            : event.key === "End"
              ? last
              : -1;
    if (to < 0 || tabs.length === 0) {
      return;
    }
    event.preventDefault();
    onChange(tabs[to].value);
    list.current?.querySelectorAll<HTMLButtonElement>('[role="tab"]')[to]?.focus();
  };

  return (
    <div
      ref={list}
      role="tablist"
      aria-label={label}
      onKeyDown={onKeyDown}
      className={clsx("flex flex-wrap gap-1", variant === "line" && "border-b border-border", className)}
    >
      {tabs.map((tab) => {
        const selected = tab.value === value;
        return (
          <button
            key={tab.value}
            type="button"
            role="tab"
            id={`${id}-tab-${tab.value}`}
            aria-selected={selected}
            aria-controls={`${id}-panel-${tab.value}`}
            tabIndex={selected ? 0 : -1}
            onClick={() => onChange(tab.value)}
            className={clsx(
              "focus-ring text-body transition-colors",
              variant === "line"
                ? "-mb-px rounded-t-md border-b-2 px-3 py-2"
                : "rounded-md border px-3 py-1",
              variant === "line"
                ? selected
                  ? "border-primary font-medium text-fg"
                  : "border-transparent text-fg-muted hover:border-border-strong hover:text-fg"
                : selected
                  ? "border-border bg-surface-subtle font-medium text-fg"
                  : "border-transparent text-fg-muted hover:bg-surface-subtle hover:text-fg",
            )}
          >
            {tab.label}
          </button>
        );
      })}
    </div>
  );
}
