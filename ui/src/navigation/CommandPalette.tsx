import { useEffect, useId, useMemo, useRef, useState } from "react";
import type { JSX, KeyboardEvent as ReactKeyboardEvent } from "react";
import { useQueries } from "@tanstack/react-query";
import { useRouter } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { clsx } from "clsx";
import { api, queryKeys, unwrap } from "../api/client";
import { asManifests, localized } from "../api/manifest";
import { useAdministers } from "../api/permissions";
import { useProjects } from "../api/projects";
import { useHiddenSections } from "../branding";
import { Dialog } from "../components/ui/Dialog";
import { Input } from "../components/ui/Input";
import { isHiddenSection, NAV_SECTIONS } from "../components/layout/navigation";
import { projectOfPlace, usePlaces } from "./places";
import { act, ASSISTANT_EVENT, available, isTyping, SHORTCUTS } from "./shortcuts";

/** The project's kinds the palette finds by name, each with the page its items open on. */
const KINDS = ["spaces", "endpoints", "pipelines", "apps", "datamodels"] as const;
const PER_GROUP = 8;

interface Row {
  id: string;
  group: string;
  label: string;
  hint?: string;
  /** Where it goes, a path inside the Portal; or what it does. */
  go: string | (() => void);
}

/** Lowercase without accents, so "zelezn" finds "Železnica". */
function fold(text: string): string {
  return text.normalize("NFD").replace(/\p{M}/gu, "").toLowerCase();
}

/** The rows matching `query`, a name's start before a match inside it, at most `PER_GROUP` per group. */
export function narrow(rows: Row[], query: string): Row[] {
  const q = fold(query.trim());
  const groups = new Map<string, { row: Row; rank: number }[]>();
  for (const row of rows) {
    const text = fold(`${row.label} ${row.hint ?? ""}`);
    const rank = q === "" ? 0 : fold(row.label).startsWith(q) ? 0 : text.includes(q) ? 1 : -1;
    if (rank < 0) continue;
    const list = groups.get(row.group) ?? [];
    list.push({ row, rank });
    groups.set(row.group, list);
  }
  return [...groups.values()].flatMap((list) =>
    list
      .map((entry, index) => ({ ...entry, index }))
      .sort((a, b) => a.rank - b.rank || a.index - b.index)
      .slice(0, PER_GROUP)
      .map((entry) => entry.row),
  );
}

/**
 * The command palette (UI-88, T-3238): Ctrl/Cmd+K on any page of a project opens one box over
 * the person's recent and starred pages, actions, the project's pages, the projects they may
 * read, and the project's spaces, endpoints, pipelines, apps, models and entity types, found by
 * name; a URN opens that entity. Everything listed comes from the same lists the pages read, so
 * nobody is offered what they may not read. Ctrl/Cmd+K again hands what was typed to the
 * assistant. "?" lists the shortcuts of the page (UI-92).
 */
export function CommandPalette({ project }: { project: string }): JSX.Element {
  const { t } = useTranslation();
  const router = useRouter();
  const [open, setOpen] = useState(false);
  const [help, setHelp] = useState(false);
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const listId = useId();
  const state = useRef({ open, query });
  useEffect(() => {
    state.current = { open, query };
  }, [open, query]);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      const ctrl = event.ctrlKey || event.metaKey;
      if (ctrl && !event.altKey && !event.shiftKey && event.key.toLowerCase() === "k") {
        event.preventDefault();
        if (state.current.open) {
          // The second press: what was typed becomes a question to the assistant.
          setOpen(false);
          window.dispatchEvent(new CustomEvent(ASSISTANT_EVENT, { detail: { text: state.current.query.trim() } }));
        } else {
          setQuery("");
          setActive(0);
          setOpen(true);
        }
        return;
      }
      if (state.current.open) return;
      if (event.key === "?" && !ctrl && !event.altKey && !isTyping(event.target)) {
        event.preventDefault();
        setHelp(true);
        return;
      }
      if (act(event)) event.preventDefault();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, []);

  const rows = usePaletteRows(project, open, query);
  // Closed, nothing reads the matches: a project's 10 000 Endpoints stay unfolded (T-5181).
  const shown = useMemo(() => (open ? narrow(rows, query) : []), [open, rows, query]);
  const current = shown[Math.min(active, shown.length - 1)];

  const choose = (row: Row | undefined) => {
    if (!row) return;
    setOpen(false);
    if (typeof row.go === "string") {
      void router.history.push(row.go);
    } else {
      row.go();
    }
  };

  const onKeyDown = (event: ReactKeyboardEvent<HTMLInputElement>) => {
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      const step = event.key === "ArrowDown" ? 1 : -1;
      setActive((at) => Math.max(0, Math.min(shown.length - 1, Math.min(at, shown.length - 1) + step)));
    } else if (event.key === "Home" || event.key === "End") {
      event.preventDefault();
      setActive(event.key === "Home" ? 0 : shown.length - 1);
    } else if (event.key === "Enter") {
      event.preventDefault();
      choose(current);
    }
  };

  const groups = [...new Set(shown.map((row) => row.group))];
  return (
    <>
      <Dialog
        open={open}
        onOpenChange={setOpen}
        title={t("palette.title")}
        description={t("palette.hint")}
        closeLabel={t("app.close")}
        size="md"
      >
        <Input
          role="combobox"
          aria-expanded="true"
          aria-controls={listId}
          aria-activedescendant={current ? `${listId}-${current.id}` : undefined}
          aria-label={t("palette.search")}
          placeholder={t("palette.placeholder")}
          autoComplete="off"
          spellCheck={false}
          value={query}
          onChange={(event) => {
            setQuery(event.target.value);
            setActive(0);
          }}
          onKeyDown={onKeyDown}
        />
        <div id={listId} role="listbox" aria-label={t("palette.results")} className="mt-3 flex flex-col gap-3">
          {shown.length === 0 ? (
            <p className="text-caption text-fg-muted">{t("palette.nothing")}</p>
          ) : (
            groups.map((group) => (
              <div key={group} role="group" aria-label={group}>
                <p aria-hidden="true" className="px-2 pb-1 text-caption font-semibold text-fg-subtle">
                  {group}
                </p>
                {shown
                  .filter((row) => row.group === group)
                  .map((row) => (
                    <div
                      key={row.id}
                      id={`${listId}-${row.id}`}
                      role="option"
                      aria-selected={row === current}
                      onMouseMove={() => setActive(shown.indexOf(row))}
                      onClick={() => choose(row)}
                      className={clsx(
                        "flex cursor-pointer items-baseline justify-between gap-3 rounded-md px-2 py-1.5 text-body",
                        row === current ? "bg-primary-soft text-primary-soft-fg" : "text-fg",
                      )}
                    >
                      <span className="min-w-0 truncate">{row.label}</span>
                      {/* On a phone the hint (often the typed text again) gives its room to the label,
                          which it used to cut to "Find entities m…" (T-4165, audit T-4157). */}
                      {row.hint ? <span className="shrink-0 truncate text-caption text-fg-muted max-sm:hidden">{row.hint}</span> : null}
                    </div>
                  ))}
              </div>
            ))
          )}
        </div>
      </Dialog>
      <ShortcutHelp open={help} onOpenChange={setHelp} />
    </>
  );
}

/** "?": the shortcuts that work on this page (UI-92). */
function ShortcutHelp({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }): JSX.Element {
  const { t } = useTranslation();
  // Read as it opens: what the page offers at that moment.
  const here = open ? available() : [];
  return (
    <Dialog open={open} onOpenChange={onOpenChange} title={t("shortcuts.title")} description={t("shortcuts.hint")} closeLabel={t("app.close")} size="sm">
      <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-2 text-body">
        {SHORTCUTS.filter((s) => here.includes(s.id)).map((s) => (
          <div key={s.id} className="contents">
            <dt className="whitespace-nowrap">
              {s.keys.map((key) => (
                <kbd key={key} className="mr-1 rounded border border-border bg-surface-muted px-1.5 py-0.5 font-mono text-caption">
                  {key}
                </kbd>
              ))}
            </dt>
            <dd className="text-fg-muted">{t(`shortcuts.${s.id}`)}</dd>
          </div>
        ))}
      </dl>
    </Dialog>
  );
}

/** What the palette offers in `project`; the lists are read only while it is open. */
function usePaletteRows(project: string, open: boolean, query: string): Row[] {
  const { t, i18n } = useTranslation();
  const projects = useProjects();
  const hidden = useHiddenSections();
  const { administers } = useAdministers();
  const { recent, favourites } = usePlaces();
  const lists = useQueries({
    queries: KINDS.map((plural) => ({
      queryKey: queryKeys.list(project, plural),
      queryFn: async () => unwrap(await api.GET("/api/v1/projects/{project}/{plural}", { params: { path: { project, plural } } })),
      enabled: open && !isHiddenSection(plural, hidden),
      retry: false,
    })),
  });

  return useMemo(() => {
    const readable = new Set(projects.data ?? []);
    // A page of a project the person may no longer read is not offered again.
    const mayRead = (path: string) => {
      const owner = projectOfPlace(path);
      return owner === undefined || readable.has(owner);
    };
    const rows: Row[] = [];
    const g = {
      recent: t("palette.groups.recent"),
      favourites: t("palette.groups.favourites"),
      actions: t("palette.groups.actions"),
      pages: t("palette.groups.pages"),
      projects: t("palette.groups.projects"),
      types: t("palette.groups.types"),
    };
    recent.filter((p) => mayRead(p.path)).forEach((p) => rows.push({ id: `r:${p.path}`, group: g.recent, label: p.title, go: p.path }));
    favourites.filter((p) => mayRead(p.path)).forEach((p) => rows.push({ id: `f:${p.path}`, group: g.favourites, label: p.title, go: p.path }));

    // Nothing typed: the actions come first. A name typed: what has that name comes first, the
    // actions after it, so Enter opens the item.
    const actionRows: Row[] = [];
    const typed = query.trim();
    if (/^urn:/i.test(typed)) {
      actionRows.push({ id: "a:entity", group: g.actions, label: t("palette.actions.openEntity", { id: typed }), hint: typed, go: `/projects/${project}/explore?entityId=${encodeURIComponent(typed)}` });
    } else if (typed !== "") {
      actionRows.push({ id: "a:find", group: g.actions, label: t("palette.actions.findEntities", { q: typed }), hint: typed, go: `/projects/${project}/explore?q=${encodeURIComponent(typed)}` });
    }
    actionRows.push({
      id: "a:assistant",
      group: g.actions,
      label: t("palette.actions.ask"),
      hint: typed,
      go: () => window.dispatchEvent(new CustomEvent(ASSISTANT_EVENT, { detail: { text: typed } })),
    });
    if (!isHiddenSection("pipelines", hidden)) {
      actionRows.push({ id: "a:pipeline", group: g.actions, label: t("palette.actions.newPipeline"), go: `/projects/${project}/pipelines/new` });
    }
    if (administers) {
      actionRows.push({ id: "a:invite", group: g.actions, label: t("palette.actions.invite"), go: "/organization/people" });
    }
    actionRows.push({ id: "a:settings", group: g.actions, label: t("palette.actions.settings"), go: `/projects/${project}/settings/general` });

    if (typed === "") rows.push(...actionRows);
    for (const section of NAV_SECTIONS) {
      if (isHiddenSection(section.plural, hidden)) continue;
      rows.push({ id: `p:${section.plural}`, group: g.pages, label: t(section.labelKey), go: `/projects/${project}/${section.plural}` });
    }
    rows.push({ id: "p:explore", group: g.pages, label: t("nav.explore"), go: `/projects/${project}/explore` });
    rows.push({ id: "p:models", group: g.pages, label: t("nav.models"), go: `/projects/${project}/models` });
    rows.push({ id: "p:catalogue", group: g.pages, label: t("nav.catalogue"), go: "/catalogue" });
    rows.push({ id: "p:glossary", group: g.pages, label: t("palette.glossary"), go: "/glossary" });

    for (const name of projects.data ?? []) {
      rows.push({ id: `j:${name}`, group: g.projects, label: name, go: `/projects/${name}/spaces` });
    }

    KINDS.forEach((plural, index) => {
      const items = asManifests(lists[index]?.data?.items ?? []);
      const group = t(`nav.${plural === "datamodels" ? "models" : plural}`);
      for (const item of items) {
        const name = item.metadata.name;
        const shown = localized(item.metadata.title, i18n.language, name);
        const label = shown === name ? undefined : shown;
        const go = plural === "datamodels" ? `/projects/${project}/models/${name}` : `/projects/${project}/${plural}/${name}`;
        rows.push({ id: `${plural}:${name}`, group, label: label ?? name, hint: label ? name : undefined, go });
        if (plural === "datamodels") {
          const classes = (item.spec as { classes?: unknown }).classes;
          for (const type of Array.isArray(classes) ? classes : []) {
            if (typeof type !== "string") continue;
            rows.push({ id: `type:${name}:${type}`, group: g.types, label: type, hint: name, go: `/projects/${project}/explore?type=${encodeURIComponent(type)}` });
          }
        }
      }
    });
    if (typed !== "") rows.push(...actionRows);
    return rows;
  }, [t, i18n.language, project, projects.data, hidden, administers, recent, favourites, lists, query]);
}
