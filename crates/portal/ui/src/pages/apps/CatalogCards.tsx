import { useState } from "react";
import type { JSX } from "react";
import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { clsx } from "clsx";
import { Button, Card, ExternalLink } from "../../components/ui";
import { Icon } from "../../components/ui/icons";
import type { IconName } from "../../components/ui/icons";
import { ago } from "../../components/ago";

/**
 * What the assistant found, as a compact list (UI-46, AG-58): one row per item of a
 * `search_catalog` tool step, with the kind, the title, the name, whether the door is open to the
 * person (a dot, the reason on hover) and how fresh the feed is when it is known. Four rows show;
 * the rest wait behind "Show more". An endpoint row can be put into the conversation's data bar
 * (AG-75). Every string comes from the run's stream and is rendered as text, never as markup
 * (AG-46).
 */

export interface CatalogItem {
  kind: string;
  name: string;
  space: string;
  owner: string;
  title?: string;
  endpointSlug?: string;
  matchReason: string[];
  access: { verdict: string; reason: string };
  freshness: { pipeline: string; scrapedAt: string; received?: number; errors?: number } | null;
}

/** A dataset of the open-data catalogue a search found (T-6104), as `jc_catalog_search` writes it. */
export interface CatalogDataset {
  title: string;
  url: string;
  publisher?: string | null;
  formats?: string[];
}

/** The open datasets of a search step's output; none when the payload carries none. */
export function catalogDatasetsOf(output: unknown): CatalogDataset[] {
  const datasets = typeof output === "object" && output !== null ? (output as { datasets?: unknown }).datasets : undefined;
  return (Array.isArray(datasets) ? datasets : []).filter(
    (one): one is CatalogDataset =>
      typeof one === "object" &&
      one !== null &&
      typeof (one as { title?: unknown }).title === "string" &&
      typeof (one as { url?: unknown }).url === "string",
  );
}

/** What a row is called: its title, or its name when the title is missing or blank (T-6104). */
function labelOf(item: CatalogItem): string {
  const title = item.title?.trim();
  return title !== undefined && title !== "" ? title : item.name;
}

const KIND_ICON: Record<string, IconName> = {
  Endpoint: "endpoints",
  ContextSpace: "spaces",
  DataModel: "models",
};
/** The icon of a kind, by own property only: a kind named `constructor` is not an icon. */
function iconOf(kind: string, fallback: IconName): IconName {
  return Object.hasOwn(KIND_ICON, kind) ? KIND_ICON[kind] : fallback;
}


/** Rows shown before "Show more". */
const FIRST_ROWS = 4;

const KIND_PLURAL: Record<string, string> = {
  Endpoint: "endpoints",
  ContextSpace: "spaces",
  DataModel: "datamodels",
};

/** The items of a `search_catalog` step, or none when the payload is not what the Portal wrote. */
export function catalogItemsOf(output: unknown): CatalogItem[] {
  if (typeof output !== "object" || output === null || !Array.isArray((output as { items?: unknown }).items)) {
    return [];
  }
  return ((output as { items: unknown[] }).items as unknown[]).filter(isItem);
}

function isItem(value: unknown): value is CatalogItem {
  if (typeof value !== "object" || value === null) {
    return false;
  }
  const item = value as Record<string, unknown>;
  const access = item.access as Record<string, unknown> | undefined;
  return (
    typeof item.kind === "string" &&
    typeof item.name === "string" &&
    typeof item.space === "string" &&
    typeof access === "object" &&
    access !== null &&
    typeof access.verdict === "string"
  );
}

export function CatalogCards({
  project,
  items,
  datasets = [],
  now,
  onUseEndpoint,
  usedEndpoints = [],
  words = "",
}: {
  project: string;
  items: CatalogItem[];
  /** The open-data catalogue's datasets the same search found, drawn as their own group. */
  datasets?: CatalogDataset[];
  /** The instant freshness is counted from; the mount time unless a test fixes it. */
  now?: number;
  /** Puts an endpoint into the conversation's data bar; without it rows offer no "Use". */
  onUseEndpoint?: (name: string) => void;
  /** The endpoints the conversation already reads, whose "Use" is spent. */
  usedEndpoints?: string[];
  /** The words the search ran on, named when nothing matched them. */
  words?: string;
}): JSX.Element {
  const { t } = useTranslation();
  // The one icon in the Portal that carries an accessible name, and it was announcing a raw
  // server-side type name in English — from a tool answer, in a product that ships four locales.
  const kindLabel = (kind: string): string =>
    Object.hasOwn(KIND_ICON, kind) ? t(`spaces.complete.kind.${kind}`) : kind;
  const [mounted] = useState(() => Date.now());
  const [expanded, setExpanded] = useState(false);
  const at = now ?? mounted;
  if (items.length === 0 && datasets.length === 0) {
    return (
      <p className="text-sm text-fg-muted">
        {words.trim() === "" ? t("agentRun.catalog.none") : t("agentRun.catalog.noneFor", { words: words.trim() })}
      </p>
    );
  }
  const shown = expanded ? items : items.slice(0, FIRST_ROWS);
  const hidden = items.length - FIRST_ROWS;
  return (
    <div className="flex min-w-0 max-w-full flex-col gap-2 text-xs">
      {items.length > 0 ? (
        <div className="flex min-w-0 flex-col">
          <Card as="ul" flush className="min-w-0 divide-y divide-border" aria-label={t("agentRun.catalog.title")}>
            {shown.map((item) => {
              const open = item.access.verdict === "allowed";
              const access = `${open ? t("agentRun.catalog.allowed") : t("agentRun.catalog.restricted")} · ${item.access.reason}`;
              const inUse = usedEndpoints.includes(item.name);
              const label = labelOf(item);
              // Another project's Endpoint shared with this one: its pages are that project's, so the
              // row names it and offers "Use" instead of a link the person may not open (T-6104).
              const shared = item.owner !== "" && item.owner !== project;
              const linkClass =
                "min-w-0 truncate font-medium text-fg hover:underline focus:outline-none focus:ring-2 focus:ring-border-focus";
              return (
                <li key={`${item.kind}/${item.name}`} className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 px-2 py-1.5">
                  <Icon
                    name={iconOf(item.kind, "endpoints")}
                    title={kindLabel(item.kind)}
                    className="size-3.5 text-fg-muted"
                  />
                  <span className="flex min-w-0 flex-1 basis-40 items-baseline gap-1.5">
                    {shared ? (
                      <span title={label} className="min-w-0 truncate font-medium text-fg">
                        {label}
                      </span>
                    ) : open && item.kind !== "DataModel" ? (
                      <Link
                        to="/projects/$project/explore"
                        params={{ project }}
                        search={{
                          space: item.space,
                          endpoint: item.kind === "Endpoint" ? item.name : undefined,
                        }}
                        title={label}
                        className={linkClass}
                      >
                        {label}
                      </Link>
                    ) : item.kind === "DataModel" ? (
                      // A model opens its own page, not the list of every model (T-2766).
                      <Link
                        to="/projects/$project/models/$name"
                        params={{ project, name: item.name }}
                        title={label}
                        className={linkClass}
                      >
                        {label}
                      </Link>
                    ) : (
                      <Link
                        to="/projects/$project/$plural"
                        params={{ project, plural: KIND_PLURAL[item.kind] ?? "endpoints" }}
                        title={label}
                        className={linkClass}
                      >
                        {label}
                      </Link>
                    )}
                    {label !== item.name && !shared ? (
                      <span title={item.name} className="min-w-0 truncate font-mono text-caption text-fg-muted">
                        {item.name}
                      </span>
                    ) : null}
                  </span>
                  {shared ? (
                    // Its own line: the title keeps the row's width (T-6104).
                    <span className="order-last flex min-w-0 basis-full flex-wrap gap-x-1.5 pl-5 text-fg-muted">
                      {label !== item.name ? <span className="min-w-0 break-all font-mono">{item.name}</span> : null}
                      <span>{t("agentRun.catalog.sharedBy", { owner: item.owner })}</span>
                    </span>
                  ) : null}
                  {item.freshness ? (
                    <span
                      className="shrink-0 text-fg-muted"
                      title={t("agentRun.catalog.fresh", {
                        when: ago(item.freshness.scrapedAt, at, t),
                        pipeline: item.freshness.pipeline,
                      })}
                    >
                      {t("agentRun.catalog.fed", { when: ago(item.freshness.scrapedAt, at, t) })}
                    </span>
                  ) : null}
                  <span
                    role="img"
                    aria-label={access}
                    title={access}
                    className={clsx("size-2 shrink-0 rounded-full", open ? "bg-success" : "bg-fg-muted opacity-50")}
                  />
                  {onUseEndpoint && item.kind === "Endpoint" && open ? (
                    <Button
                      size="sm"
                      disabled={inUse}
                      // Already in the run's endpoints: refused, and it says so rather than being
                      // a grey pill somebody hovers to find out (UI-44).
                      disabledReason={
                        inUse ? t("agentRun.catalog.inUseLabel", { name: item.name }) : undefined
                      }
                      aria-label={
                        inUse
                          ? t("agentRun.catalog.inUseLabel", { name: item.name })
                          : t("agentRun.catalog.useLabel", { name: item.name })
                      }
                      onClick={() => {
                        onUseEndpoint(item.name);
                      }}
                      className="h-control-xs rounded-full px-2 text-caption"
                    >
                      {inUse ? t("agentRun.catalog.inUse") : t("agentRun.catalog.use")}
                    </Button>
                  ) : null}
                </li>
              );
            })}
          </Card>
          {hidden > 0 ? (
            <Button
              variant="ghost"
              size="sm"
              aria-expanded={expanded}
              onClick={() => {
                setExpanded((was) => !was);
              }}
              className="self-start text-fg-muted"
            >
              {expanded ? t("agentRun.catalog.showLess") : t("agentRun.catalog.showMore", { count: hidden })}
            </Button>
          ) : null}
        </div>
      ) : null}
      {datasets.length > 0 ? (
        <section className="flex min-w-0 flex-col gap-1" aria-label={t("agentRun.catalog.datasets")}>
          <h4 className="text-caption font-medium text-fg-muted">{t("agentRun.catalog.datasets")}</h4>
          <Card as="ul" flush className="min-w-0 divide-y divide-border">
            {datasets.map((dataset) => (
              <li key={dataset.url} className="flex min-w-0 flex-wrap items-baseline gap-x-2 gap-y-0.5 px-2 py-1.5">
                <ExternalLink href={dataset.url} className="min-w-0 truncate font-medium text-fg hover:underline">
                  {dataset.title.trim() !== "" ? dataset.title : dataset.url}
                </ExternalLink>
                {typeof dataset.publisher === "string" && dataset.publisher !== "" ? (
                  <span className="min-w-0 truncate text-caption text-fg-muted">{dataset.publisher}</span>
                ) : null}
                {(dataset.formats ?? []).length > 0 ? (
                  <span className="shrink-0 text-caption text-fg-muted">{(dataset.formats ?? []).join(", ")}</span>
                ) : null}
              </li>
            ))}
          </Card>
        </section>
      ) : null}
    </div>
  );
}
