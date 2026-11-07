import { useId, useState } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { useQueries } from "@tanstack/react-query";
import { clsx } from "clsx";
import { api, queryKeys, unwrap } from "../../api/client";
import { asManifests, refName } from "../../api/manifest";
import type { Manifest } from "../../api/manifest";
import { Button, Icon, safeHref } from "../ui";

/**
 * Where each enabled representation answers under the endpoint's URL (EP-08, EP-44): the
 * same paths the gateway's DCAT index at `/api/endpoint/{slug}/` lists, so a steward can hand
 * out one link per shape without opening the index.
 */
export const REPRESENTATION_PATHS: Record<string, string> = {
  // A read names a type: a query with no selector is 400 BadRequestData (GW33, CIM 009
  // 5.7.2.4), and the endpoint's own type list is the conformant way in — every type it
  // serves, each one a query away. The grants decide which of them answer.
  "ngsi-ld": "/ngsi-ld/v1/types",
  mcp: "/mcp",
  csv: "/file.csv",
  "ogc-features": "/ogc/features",
  sta: "/sta/v1.1",
  ical: "/ical/v1/events.ics",
  websocket: "/ws",
};

/**
 * Every representation an Endpoint may serve, in jc-core's `Representation::ALL` order. CSV is
 * the one file (T-4600): the retired ones below are never offered, and a manifest that still
 * names one is read without it.
 */
export const ALL_REPRESENTATIONS = ["ngsi-ld", "mcp", "csv", "ogc-features", "sta", "odata", "websocket", "ical"] as const;

/** The file representations an Endpoint no longer serves (T-4600). */
export const RETIRED_REPRESENTATIONS: readonly string[] = ["xlsx", "parquet", "json", "zip", "geojson"];

/** Served by every Endpoint whatever it lists: `mcp` only while `spec.mcp` is not `false`. */
export function alwaysServed(spec: { mcp?: unknown }): string[] {
  return spec.mcp === false ? ["ngsi-ld"] : ["ngsi-ld", "mcp"];
}

function names(list: unknown): string[] | undefined {
  return Array.isArray(list) ? list.filter((rep): rep is string => typeof rep === "string") : undefined;
}

/**
 * What an Endpoint serves: the representations its manifest lists or, when it lists none,
 * every one but `sta` less `disabledRepresentations` (EP-104), never a retired file (T-4600);
 * then `ngsi-ld`, which every Endpoint serves (EP-10), and `mcp`, which every Endpoint serves,
 * public or internal, unless `spec.mcp` is `false` (EP-24). The gateway reads the same rule
 * (jc-core's `served_representations`), so a link shown here answers there; its Policy still
 * decides every call.
 */
export function servedRepresentations(spec: {
  enabledRepresentations?: unknown;
  disabledRepresentations?: unknown;
  mcp?: unknown;
}): string[] {
  const always = alwaysServed(spec);
  const disabled = names(spec.disabledRepresentations) ?? [];
  const listed =
    names(spec.enabledRepresentations)?.filter((rep) => !RETIRED_REPRESENTATIONS.includes(rep)) ??
    ALL_REPRESENTATIONS.filter(
      (rep) => rep !== "sta" && rep !== "ical" && !disabled.includes(rep) && (rep !== "mcp" || always.includes("mcp")),
    );
  return [...listed, ...always.filter((rep) => !listed.includes(rep))];
}

/** The links every endpoint has whatever it enables: its index and the model it publishes. */
export const ENDPOINT_LINKS: Array<{ key: string; path: string }> = [
  { key: "index", path: "/" },
  { key: "linkml", path: "/schema/v1/linkml" },
  { key: "jsonSchema", path: "/schema/v1/json-schema" },
  { key: "access", path: "/access" },
];

/**
 * Where an endpoint answers. The slug comes out of a manifest, so it is encoded the same way
 * the catalogue encodes a name: a slug carrying `../`, `?` or `#` would otherwise build a link
 * to somewhere else on this origin, and `safeHref` would pass it because it is a real URL.
 */
export function endpointUrl(slug: string, path: string): string {
  return `${window.location.origin}/api/endpoint/${encodeURIComponent(slug)}${path}`;
}

/**
 * The origin an MCP client connects to: the data host `api.{domain}`, which the gateway names as
 * the `resource` of every MCP URL (`JC_GATEWAY_PUBLIC_URL`, ADR-N-052 §2.1, T-4632; `domain` is
 * what the branding serves). A client refuses resource metadata that names another URL than the
 * one it called (RFC 9728 §3.3), so the Portal's own host, right for a browser whose edge session
 * is the bearer, is wrong for a connector (T-3019). The page's origin when the installation names
 * no domain, or one that is not a bare host.
 */
export function mcpOrigin(domain: string | undefined): string {
  const host = domain?.trim().toLowerCase() ?? "";
  if (host) {
    try {
      const url = new URL(`https://${host}`);
      if (url.host === host && url.pathname === "/" && !url.username && !url.password) {
        return `https://api.${url.host}`;
      }
    } catch {
      // Not a host: the page's own origin below.
    }
  }
  return window.location.origin;
}

/**
 * Where one representation of an endpoint answers: `mcp` and `websocket` on the platform host
 * their clients connect to (a WebSocket client copies the one address API/07 and the catalogue
 * name, T-4132), every other one on this origin, where the edge turns the session into the bearer.
 */
/** An https feed address as `webcal://`, which a calendar client opens as a subscription (RFC 5545 feeds). */
export function webcalUrl(url: string): string {
  return url.replace(/^https?:\/\//, "webcal://");
}

export function representationUrl(slug: string, representation: string, domain: string | undefined): string {
  const path = REPRESENTATION_PATHS[representation] ?? "/";
  return representation === "mcp" || representation === "websocket"
    ? `${mcpOrigin(domain)}/api/endpoint/${encodeURIComponent(slug)}${path}`
    : endpointUrl(slug, path);
}

/**
 * The MCP hub: one connector over every Endpoint the caller may read, the Endpoint named on each
 * call (EP-87, ADR-N-025), on the platform host an MCP client connects to.
 */
export function hubUrl(domain: string | undefined): string {
  return `${mcpOrigin(domain)}/api/mcp`;
}

/**
 * A named MCP server over chosen Endpoints (EP-92, ADR-N-043): `/api/mcp/{project}/{name}`, `org`
 * for the organization's, on the platform host an MCP client connects to.
 */
export function namedServerUrl(domain: string | undefined, project: string, name: string): string {
  return `${mcpOrigin(domain)}/api/mcp/${encodeURIComponent(project)}/${encodeURIComponent(name)}`;
}

/**
 * A dataset's page on the site of a `CkanInstance`: `{spec.url}/dataset/{name}`, the address the
 * catalogue API builds too (EP-82). `undefined` for a base that is not an https URL: jc-core
 * refuses any other `spec.url`, and a link is never built from what it would refuse (T-3018).
 */
export function catalogueUrl(instanceUrl: string, dataset: string): string | undefined {
  let base: URL;
  try {
    base = new URL(instanceUrl);
  } catch {
    return undefined;
  }
  if (base.protocol !== "https:" || base.search || base.hash) {
    return undefined;
  }
  return `${base.href.replace(/\/+$/, "")}/dataset/${encodeURIComponent(dataset)}`;
}

/**
 * The catalogue page of each Endpoint that publishes one (EP-62, EP-82): the dataset its
 * `spec.publish.ckan` names (its own name when that names none) on the site of the `CkanInstance`
 * its `instanceRef` names in the Endpoint's `project`. `undefined` for an Endpoint that publishes
 * nowhere, while the project's catalogues load, and for a catalogue this person cannot read: the
 * link is the instance's own URL or none, never a host guessed from the Portal's (T-3018).
 */
export function useCatalogueLinks(
  projects: string[],
): (project: string, endpoint: Manifest) => string | undefined {
  const unique = [...new Set(projects.filter(Boolean))].sort();
  const lists = useQueries({
    queries: unique.map((project) => ({
      queryKey: queryKeys.list(project, "ckaninstances"),
      retry: false,
      queryFn: async () =>
        unwrap(
          await api.GET("/api/v1/projects/{project}/{plural}", {
            params: { path: { project, plural: "ckaninstances" } },
          }),
        ),
    })),
  });
  const sites = new Map<string, string>();
  unique.forEach((project, index) => {
    for (const instance of asManifests(lists[index]?.data?.items ?? [])) {
      const url = (instance.spec as { url?: unknown }).url;
      if (typeof url === "string") {
        sites.set(`${project}/${instance.metadata.name}`, url);
      }
    }
  });
  return catalogueLinkOf(sites);
}

/** What `useCatalogueLinks` answers, given the sites it resolved per `{project}/{instance}`. */
export function catalogueLinkOf(
  sites: Map<string, string>,
): (project: string, endpoint: Manifest) => string | undefined {
  return (project, endpoint) => {
    const publish = (endpoint.spec as { publish?: { ckan?: { instanceRef?: unknown; name?: unknown } } })
      .publish?.ckan;
    if (!publish) {
      return undefined;
    }
    const site = sites.get(`${project}/${refName(publish.instanceRef)}`);
    const dataset = typeof publish.name === "string" && publish.name ? publish.name : endpoint.metadata.name;
    return site ? catalogueUrl(site, dataset) : undefined;
  };
}

const LINK_PRIMARY =
  "border-primary-200 bg-primary-soft font-medium text-primary-soft-fg hover:border-primary-400 hover:bg-primary-100";
const LINK_MUTED =
  "border-border text-fg-muted hover:border-border-strong hover:bg-surface-muted hover:text-fg";

/** A pill that opens one URL of an endpoint in a new tab; `muted` for the links every endpoint has. */
export function EndpointLink({
  href,
  children,
  muted = false,
}: {
  href: string;
  children: string;
  muted?: boolean;
}): JSX.Element {
  const safe = safeHref(href);
  if (!safe) {
    // A URL the Portal did not build: the words stay, the link is withheld (PF-50).
    return <span className="font-mono text-caption text-fg-muted">{children}</span>;
  }
  return (
    <a
      href={safe}
      target="_blank"
      rel="noreferrer"
      title={safe}
      className={clsx(
        "focus-ring inline-flex items-center gap-1 rounded-full border px-2 py-0.5 font-mono text-caption",
        muted ? LINK_MUTED : LINK_PRIMARY,
      )}
    >
      {children}
      <Icon name="external" className="size-3" />
    </a>
  );
}

/** Copies a URL; its own module so a page that shows one does not load the Endpoints page (bundle budget). */
export function CopyUrlButton({ url, label, spoken }: { url: string; label?: string; /** The name a screen reader says where several buttons read the same. */ spoken?: string }): JSX.Element {
  const { t } = useTranslation();
  const [copied, setCopied] = useState(false);

  return (
    <Button
      size="sm"
      title={url}
      aria-label={copied ? undefined : spoken}
      icon={<Icon name={copied ? "check" : "copy"} className="size-4" />}
      onClick={() => {
        void navigator.clipboard
          ?.writeText(url)
          .then(() => setCopied(true))
          .catch(() => setCopied(false));
      }}
    >
      {copied ? t("endpoints.copied") : (label ?? t("endpoints.copyUrl"))}
    </Button>
  );
}

/** The surfaces a summary names first: the ones most people connect with (T-4989). */
const SUMMARY_FIRST = ["ngsi-ld", "mcp", "ogc-features"];
const SUMMARY_NAMED = 3;

/**
 * What an endpoint serves in one line for a list row (T-4989): the three main surfaces by name
 * and "+N more" for the rest, which a hover or keyboard focus shows and a screen reader reads.
 * The per-surface links are on the endpoint's own page.
 */
export function RepresentationSummary({ served }: { served: string[] }): JSX.Element {
  const { t } = useTranslation();
  const tipId = useId();
  const [open, setOpen] = useState(false);
  if (served.length === 0) {
    return <span className="text-fg-subtle">{t("endpoints.field.noRepresentations")}</span>;
  }
  const rank = (rep: string): number => {
    const at = SUMMARY_FIRST.indexOf(rep);
    return at < 0 ? SUMMARY_FIRST.length : at;
  };
  const labels = [...served]
    .sort((a, b) => rank(a) - rank(b))
    .map((rep) => t(`spaces.api.surface.${rep}`, { defaultValue: rep }));
  const named = labels.slice(0, SUMMARY_NAMED);
  const rest = labels.slice(SUMMARY_NAMED);
  return (
    <span className="relative inline-flex max-w-full items-center gap-1.5 whitespace-nowrap text-body text-fg">
      <span className="truncate" title={labels.join(", ")}>
        {named.join(", ")}
      </span>
      {rest.length > 0 ? (
        <>
          <span
            tabIndex={0}
            aria-describedby={tipId}
            className="focus-ring cursor-help rounded-sm border-b border-dotted border-fg-muted text-caption text-fg-muted"
            onMouseEnter={() => setOpen(true)}
            onMouseLeave={() => setOpen(false)}
            onFocus={() => setOpen(true)}
            onBlur={() => setOpen(false)}
            onKeyDown={(event) => {
              if (event.key === "Escape" && open) {
                event.stopPropagation();
                setOpen(false);
              }
            }}
          >
            {t("endpoints.field.more", { count: rest.length })}
          </span>
          <span
            id={tipId}
            role="tooltip"
            className={clsx(
              "absolute left-0 top-full z-20 mt-1 rounded-md border border-border bg-surface px-2 py-1 text-caption text-fg shadow-2",
              open ? "block" : "sr-only",
            )}
          >
            {rest.join(", ")}
          </span>
        </>
      ) : null}
    </span>
  );
}
