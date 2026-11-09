import { useMemo, useState } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { Link } from "@tanstack/react-router";
import type { components } from "../../api/schema";
import { Badge, Button, Field, Icon, Input, Select } from "../../components/ui";

type Summary = components["schemas"]["DashboardSummary"];

/** Grafana's browse sort choices. */
export type Sort = "alpha-asc" | "alpha-desc";

/** What the list shows: Grafana's search over titles and folders, one tag, one order. */
export function browse(items: readonly Summary[], query: string, tag: string, sort: Sort): Summary[] {
  const words = query.trim().toLowerCase();
  const kept = items.filter(
    (item) => (!words || item.title.toLowerCase().includes(words)) && (!tag || item.tags.includes(tag)),
  );
  const order = sort === "alpha-desc" ? -1 : 1;
  return kept.sort((a, b) => order * a.title.localeCompare(b.title, undefined, { sensitivity: "base" }));
}

/** Folders by title in the asked order, each with its dashboards; General's dashboards follow, as in Grafana. */
export function folders(items: readonly Summary[], sort: Sort): { folder: string | null; items: Summary[] }[] {
  const byFolder = new Map<string, Summary[]>();
  for (const item of items) {
    if (item.folder) byFolder.set(item.folder, [...(byFolder.get(item.folder) ?? []), item]);
  }
  const order = sort === "alpha-desc" ? -1 : 1;
  const named = [...byFolder.keys()]
    .sort((a, b) => order * a.localeCompare(b, undefined, { sensitivity: "base" }))
    .map((folder) => ({ folder, items: byFolder.get(folder) ?? [] }));
  const general = items.filter((item) => !item.folder);
  return general.length ? [...named, { folder: null, items: general }] : named;
}

function Row({ project, item, location }: { project: string; item: Summary; location: boolean }): JSX.Element {
  const { t } = useTranslation();
  return (
    <li className="flex flex-wrap items-center gap-x-3 gap-y-1 border-b border-border py-2 pl-2 last:border-b-0">
      <Icon name="dashboards" className="size-4 shrink-0 text-fg-muted" />
      <span className="flex min-w-0 flex-1 flex-col">
        <Link
          data-row-link=""
          to="/projects/$project/grafana/dashboards/$uid"
          params={{ project, uid: item.uid }}
          className="focus-ring break-words rounded-sm text-body font-medium text-primary-soft-fg hover:underline"
        >
          {item.title}
        </Link>
        {location ? (
          <span className="flex items-center gap-1 text-caption text-fg-muted">
            <Icon name="folder" className="size-3" />
            {item.folder ?? t("grafana.dashboards.general")}
          </span>
        ) : null}
      </span>
      {item.tags.length ? (
        <span className="flex flex-wrap gap-1">
          {item.tags.map((tag) => (
            <Badge key={tag}>{tag}</Badge>
          ))}
        </span>
      ) : null}
    </li>
  );
}

/** One folder of the tree: closed at first, as Grafana's browse view opens it. */
function FolderRow({ project, folder, items }: { project: string; folder: string; items: Summary[] }): JSX.Element {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  return (
    <li className="border-b border-border last:border-b-0">
      <Button variant="ghost" size="sm" aria-expanded={open} onClick={() => setOpen(!open)} className="w-full justify-start">
        <Icon name={open ? "chevronDown" : "chevronRight"} className="size-4" />
        <Icon name="folder" className="size-4 text-fg-muted" />
        <span className="min-w-0 flex-1 truncate text-left">{folder}</span>
        <span className="text-caption text-fg-muted">{t("grafana.dashboards.inFolder", { count: items.length })}</span>
      </Button>
      {open ? (
        <ul className="ml-6">
          {items.map((item) => (
            <Row key={item.uid} project={project} item={item} location={false} />
          ))}
        </ul>
      ) : null}
    </li>
  );
}

/** Grafana's dashboard browser: search, tag filter, sort, and a folder tree while nothing is searched. */
export function DashboardList({ project, items }: { project: string; items: readonly Summary[] }): JSX.Element {
  const { t } = useTranslation();
  const [query, setQuery] = useState("");
  const [tag, setTag] = useState("");
  const [sort, setSort] = useState<Sort>("alpha-asc");
  // Grafana's two browse views: the folder tree, or every dashboard in one list with its folder.
  const [flat, setFlat] = useState(false);
  const tags = useMemo(() => [...new Set(items.flatMap((item) => item.tags))].sort(), [items]);
  const shown = browse(items, query, tag, sort);
  const searching = Boolean(query.trim() || tag);
  return (
    <div className="flex flex-col gap-3">
      <div className="grid gap-3 sm:grid-cols-[minmax(0,1fr)_12rem_auto_12rem]">
        <Field id="dashboard-search" label={t("grafana.dashboards.search")} help={t("grafana.dashboards.searchHint")}>
          <Input id="dashboard-search" type="search" value={query} onChange={(event) => setQuery(event.target.value)} />
        </Field>
        <Field id="dashboard-tag" label={t("grafana.dashboards.tag")} help={t("grafana.dashboards.tagHint")}>
          <Select id="dashboard-tag" value={tag} onChange={(event) => setTag(event.target.value)}>
            <option value="">{t("grafana.dashboards.anyTag")}</option>
            {tags.map((name) => (
              <option key={name} value={name}>
                {name}
              </option>
            ))}
          </Select>
        </Field>
        <div role="group" aria-label={t("grafana.dashboards.view")} className="flex items-start gap-1 sm:pt-6">
          <Button variant="secondary" aria-pressed={!flat} aria-label={t("grafana.dashboards.folderView")} title={t("grafana.dashboards.folderView")} onClick={() => setFlat(false)} className={!flat ? "bg-primary-soft text-primary-soft-fg" : undefined}>
            <Icon name="folder" className="size-4" />
          </Button>
          <Button variant="secondary" aria-pressed={flat} aria-label={t("grafana.dashboards.listView")} title={t("grafana.dashboards.listView")} onClick={() => setFlat(true)} className={flat ? "bg-primary-soft text-primary-soft-fg" : undefined}>
            <Icon name="menu" className="size-4" />
          </Button>
        </div>
        <Field id="dashboard-sort" label={t("grafana.dashboards.sort")} help={t("grafana.dashboards.sortHint")}>
          <Select id="dashboard-sort" value={sort} onChange={(event) => setSort(event.target.value as Sort)}>
            <option value="alpha-asc">{t("grafana.dashboards.alphaAsc")}</option>
            <option value="alpha-desc">{t("grafana.dashboards.alphaDesc")}</option>
          </Select>
        </Field>
      </div>
      {shown.length === 0 ? (
        <div role="status" className="flex flex-col items-start gap-2 py-3">
          <p className="text-body text-fg-muted">{t("grafana.dashboards.noMatch")}</p>
          {/* Grafana's empty search offers to clear the search and filters in one step. */}
          <Button
            size="sm"
            variant="secondary"
            onClick={() => {
              setQuery("");
              setTag("");
            }}
          >
            {t("grafana.dashboards.clear")}
          </Button>
        </div>
      ) : null}
      {shown.length > 0 ? (
        // Grafana's column heads over the browse list.
        <div aria-hidden="true" className="flex justify-between border-b border-border bg-surface-subtle px-2 py-1.5 text-caption font-semibold text-fg-muted">
          <span>{t("grafana.dashboards.name")}</span>
          <span>{t("grafana.dashboards.tags")}</span>
        </div>
      ) : null}
      {shown.length === 0 ? null : searching || flat ? (
        <ul aria-label={t("grafana.dashboards.results")}>
          {shown.map((item) => (
            <Row key={item.uid} project={project} item={item} location />
          ))}
        </ul>
      ) : (
        <ul aria-label={t("grafana.dashboards.tree")}>
          {folders(shown, sort).map((group) =>
            group.folder === null ? (
              group.items.map((item) => <Row key={item.uid} project={project} item={item} location={false} />)
            ) : (
              <FolderRow key={group.folder} project={project} folder={group.folder} items={group.items} />
            ),
          )}
        </ul>
      )}
    </div>
  );
}
