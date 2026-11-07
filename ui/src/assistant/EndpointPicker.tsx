import { useEffect, useId, useMemo, useRef, useState } from "react";
import type { JSX, KeyboardEvent } from "react";
import { clsx } from "clsx";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, queryKeys, unwrap } from "../api/client";
import { asManifests, localized } from "../api/manifest";
import { Badge, Button, Input } from "../components/ui";
import { Icon } from "../components/ui/icons";

/**
 * The data a conversation may query (AG-75): a bar directly above the text box, the way an MCP
 * client is pointed at its connectors. The chosen endpoints are pills with their titles; "Add
 * endpoint" opens a searchable list of the project's endpoints, at most five chosen. The same bar
 * sits in a new conversation and in a running one, where a change travels with the next message.
 */

/** How many endpoints one conversation may query (AG-75). */
export const MAX_ENDPOINTS = 5;

const STORAGE_PREFIX = "jc.assistant.endpoints.";

/** The endpoints last chosen in this project, for the next conversation; none without storage. */
export function storedEndpoints(project: string): string[] {
  try {
    const raw = sessionStorage.getItem(STORAGE_PREFIX + project);
    const parsed: unknown = raw === null ? [] : JSON.parse(raw);
    return Array.isArray(parsed)
      ? parsed.filter((name): name is string => typeof name === "string").slice(0, MAX_ENDPOINTS)
      : [];
  } catch {
    return [];
  }
}

export function rememberEndpoints(project: string, names: string[]): void {
  try {
    sessionStorage.setItem(STORAGE_PREFIX + project, JSON.stringify(names));
  } catch {
    // No storage (a private window): the choice lasts until the next page.
  }
}

/** The endpoint names of a run's `endpoints` field (`[{name, slug, space}]`), in order. */
export function runEndpointNames(endpoints: unknown): string[] {
  if (!Array.isArray(endpoints)) {
    return [];
  }
  return endpoints
    .map((endpoint: unknown) =>
      typeof endpoint === "object" && endpoint !== null && typeof (endpoint as { name?: unknown }).name === "string"
        ? (endpoint as { name: string }).name
        : null,
    )
    .filter((name): name is string => name !== null && name !== "");
}

/** Whether two endpoint lists name the same endpoints in the same order. */
export function sameEndpoints(a: string[], b: string[]): boolean {
  return a.length === b.length && a.every((name, i) => name === b[i]);
}

interface Choice {
  name: string;
  title: string;
  audience: string;
}

/** The project's endpoints, one query for every bar and for the dock's prefetch (T-2719). */
export function endpointsQuery(project: string) {
  return {
    queryKey: queryKeys.list(project, "endpoints"),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "endpoints" } },
        }),
      ),
  };
}

/** The project's endpoints as choices, read once per project and shared by every bar. */
function useChoices(project: string): { choices: Choice[]; loading: boolean } {
  const { i18n } = useTranslation();
  const query = useQuery(endpointsQuery(project));
  const choices = useMemo(
    () =>
      asManifests(query.data?.items ?? []).map((endpoint) => {
        const name = endpoint.metadata.name;
        const audience = (endpoint.spec as { audience?: unknown } | undefined)?.audience;
        return {
          name,
          title: localized(endpoint.metadata.title, i18n.language, name) || name,
          audience: typeof audience === "string" ? audience : "",
        };
      }),
    [query.data, i18n.language],
  );
  return { choices, loading: query.isLoading };
}

/** The short word for an endpoint's audience on its badge. */
function audienceLabel(audience: string, t: (key: string) => string): string {
  if (audience === "public") {
    return t("assistant.data.audience.public");
  }
  if (audience === "organization") {
    return t("assistant.data.audience.organization");
  }
  return audience === "" ? "" : t("assistant.data.audience.project");
}

export function DataBar({
  project,
  selected,
  onChange,
  opens = "up",
}: {
  project: string;
  selected: string[];
  onChange: (names: string[]) => void;
  /** Where the list opens: up over a conversation's messages, down in a scrolling empty state. */
  opens?: "up" | "down";
}): JSX.Element {
  const { t } = useTranslation();
  const { choices, loading } = useChoices(project);
  const [open, setOpen] = useState(false);
  const [search, setSearch] = useState("");
  const [active, setActive] = useState(0);
  const listId = useId();
  const root = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const box = useRef<HTMLInputElement>(null);
  const full = selected.length >= MAX_ENDPOINTS;

  const titleOf = (name: string) => choices.find((choice) => choice.name === name)?.title ?? name;
  const words = search.trim().toLowerCase();
  const shown = choices.filter(
    (choice) => words === "" || choice.name.toLowerCase().includes(words) || choice.title.toLowerCase().includes(words),
  );

  // The list opens for typing, so focus goes where the next key lands — but only when it opens,
  // which is a click of the trigger. React's prop of that name did it on mount instead, so a bar
  // rendered already open took the focus on arrival, which is what UI-15 refuses.
  useEffect(() => {
    if (open) {
      box.current?.focus();
    }
  }, [open]);

  // A click anywhere else closes the list, as every menu does.
  useEffect(() => {
    if (!open) {
      return;
    }
    const onDown = (event: MouseEvent) => {
      if (root.current && !root.current.contains(event.target as Node)) {
        setOpen(false);
      }
    };
    document.addEventListener("mousedown", onDown);
    return () => {
      document.removeEventListener("mousedown", onDown);
    };
  }, [open]);

  const toggle = (name: string) => {
    if (selected.includes(name)) {
      onChange(selected.filter((chosen) => chosen !== name));
    } else if (!full) {
      onChange([...selected, name]);
    }
  };

  const close = () => {
    setOpen(false);
    setSearch("");
    trigger.current?.focus();
  };

  const onKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key === "ArrowDown") {
      event.preventDefault();
      setActive((at) => Math.min(at + 1, Math.max(shown.length - 1, 0)));
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      setActive((at) => Math.max(at - 1, 0));
    } else if (event.key === "Enter") {
      event.preventDefault();
      const choice = shown[active];
      if (choice) {
        toggle(choice.name);
      }
    } else if (event.key === "Escape") {
      event.preventDefault();
      close();
    }
  };

  const pill = "inline-flex items-center gap-1 rounded-full border px-2 py-0.5";

  return (
    <div
      ref={root}
      role="group"
      aria-label={t("assistant.data.label")}
      className="relative flex flex-wrap items-center gap-1 text-xs"
    >
      <span className="mr-0.5 font-medium text-fg-muted">{t("assistant.data.title")}</span>
      {selected.length > 0 ? (
        <ul aria-label={t("assistant.data.chosen")} className="flex flex-wrap items-center gap-1">
          {selected.map((name) => (
            <li key={name} title={name} className={clsx(pill, "border-primary-200 bg-primary-soft pr-1 text-primary-soft-fg")}>
              <Icon name="endpoints" className="size-3" />
              <span className="max-w-40 truncate">{titleOf(name)}</span>
              <Button
                variant="ghost"
                size="xs"
                aria-label={t("assistant.data.remove", { name })}
                onClick={() => {
                  onChange(selected.filter((chosen) => chosen !== name));
                }}
                className="rounded-full px-1"
              >
                ×
              </Button>
            </li>
          ))}
        </ul>
      ) : null}
      <Button
        ref={trigger}
        variant="ghost"
        size="xs"
        aria-haspopup="listbox"
        aria-expanded={open}
        onClick={() => {
          setOpen((was) => !was);
          setActive(0);
        }}
        className={clsx(pill, "border-dashed border-border text-fg-muted hover:text-fg")}
      >
        <Icon name="plus" className="size-3" />
        {t("assistant.data.add")}
      </Button>
      {selected.length === 0 ? <span className="text-fg-muted">{t("assistant.data.empty")}</span> : null}

      {open ? (
        <div
          className={clsx(
            "absolute left-0 z-50 w-80 min-w-56 max-w-full rounded-lg border border-border bg-surface p-1 shadow-2",
            opens === "up" ? "bottom-full mb-1" : "top-full mt-1",
          )}
        >
          <div className="relative">
            <Icon
              name="search"
              aria-hidden="true"
              className="pointer-events-none absolute left-2 top-1/2 size-3.5 -translate-y-1/2 text-fg-muted"
            />
            <Input
              ref={box}
              role="combobox"
              aria-expanded
              aria-controls={listId}
              aria-activedescendant={shown[active] ? `${listId}-${shown[active].name}` : undefined}
              aria-label={t("assistant.data.search")}
              placeholder={t("assistant.data.search")}
              value={search}
              onChange={(event) => {
                setSearch(event.target.value);
                setActive(0);
              }}
              onKeyDown={onKeyDown}
              className="h-control pl-7 text-caption"
            />
          </div>
          {full ? <p className="px-2 pt-1 text-fg-muted">{t("assistant.data.full", { max: MAX_ENDPOINTS })}</p> : null}
          {loading ? <p className="px-2 py-1 text-fg-muted">{t("assistant.data.loading")}</p> : null}
          {!loading && shown.length === 0 ? <p className="px-2 py-1 text-fg-muted">{t("assistant.data.none")}</p> : null}
          <ul
            id={listId}
            role="listbox"
            aria-multiselectable
            aria-label={t("assistant.data.label")}
            className="mt-1 max-h-52 overflow-y-auto"
          >
            {shown.map((choice, index) => {
              const checked = selected.includes(choice.name);
              const disabled = !checked && full;
              const audience = audienceLabel(choice.audience, t);
              return (
                <li
                  key={choice.name}
                  id={`${listId}-${choice.name}`}
                  role="option"
                  aria-selected={checked}
                  aria-disabled={disabled}
                  onMouseEnter={() => {
                    setActive(index);
                  }}
                  onClick={() => {
                    if (!disabled) {
                      toggle(choice.name);
                    }
                  }}
                  className={clsx(
                    "flex cursor-pointer items-center gap-2 rounded-md px-2 py-1.5",
                    index === active && "bg-surface-subtle",
                    disabled && "cursor-not-allowed opacity-50",
                  )}
                >
                  <span className="flex size-4 shrink-0 items-center justify-center text-primary-soft-fg">
                    {checked ? <Icon name="check" className="size-3.5" /> : null}
                  </span>
                  <span className="min-w-0 flex-1">
                    <span className="block truncate font-medium text-fg">{choice.title}</span>
                    <span className="block truncate font-mono text-caption text-fg-muted">{choice.name}</span>
                  </span>
                  {audience ? (
                    <Badge tone="neutral" className="shrink-0">
                      {audience}
                    </Badge>
                  ) : null}
                </li>
              );
            })}
          </ul>
        </div>
      ) : null}
    </div>
  );
}
