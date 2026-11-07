import { AlertDialog } from "../../components/AlertDialog";
import { useCallback, useEffect, useId, useMemo, useRef, useState } from "react";
import { clsx } from "clsx";
import type { JSX, ReactNode } from "react";
import { useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import type { UseQueryResult } from "@tanstack/react-query";
import { andQ, applyChanges, attributesOf, cellText, endpointSource, matchesQ, MAX_ENTITIES, originTransport, parseGridConfig, queryFromGroup, searchQuery, SourceError, sourceFor } from "@joinedcontext/sdk";
import type { FilterColumn, FilterGroup, GridState, RichRow } from "@joinedcontext/sdk";
import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { ApiError } from "../../api/client";
import { bytesOfType, entityBytes, exactBytes, sizeText, spaceUsageQuery, usageRefusal } from "../../api/spaceUsage";
import type { SpaceUsage } from "../../api/spaceUsage";
import { asManifests, localized, refName } from "../../api/manifest";
import type { Change, Manifest } from "../../api/manifest";
import { ActivityFeed } from "../../components/ActivityFeed";
import { LifecycleBadge } from "../../components/status/LifecycleBadge";
import {
  ENDPOINT_LINKS,
  EndpointLink,
  endpointUrl,
  REPRESENTATION_PATHS,
  representationUrl,
  servedRepresentations,
  useCatalogueLinks,
} from "../../components/endpoints/links";
import { useBranding } from "../../branding";
import { SharedWithBadge } from "../../components/endpoints/sharing";
import { useIdentity } from "../../auth/AuthProvider";
import { accessQuery, mayDelete } from "../../components/entities/AccessPanel";
import { attributeAccessQuery, writableOf } from "../../api/access";
import { AttributeHistory } from "../../components/entities/history/AttributeHistory";
import type { AttributeAccess } from "../../api/access";
import type { GrantDocument } from "../../api/access";
import { PortalEntityGrid, rememberedSort } from "../../components/entities/PortalEntityGrid";
import { enumsOfModel, filterSlotsOf, relationsOfModel, rulesOfModel, useModelSource } from "../../components/entities/filters";
import type { FilterSlot } from "../../components/entities/filters";
import { AddFieldDialog } from "../../components/entities/AddFieldDialog";
import { ViewBar } from "../../components/entities/ViewBar";
import { columnLabel, displayOrder } from "../../components/entities/displayOrder";
import { readLayout, writeLayout } from "../../components/entities/viewLayout";
import { dataViewsKey, listViews } from "../../api/dataViews";
import { GroupCounts, groupTerm } from "../../components/entities/ViewOptions";
import { EMPTY_GROUP, FilterChips, readGroup } from "../../components/entities/FilterPanel";
import { useIsPhone } from "../../phone";
import { DataToolbar } from "../../components/entities/DataToolbar";
import { AGGREGATIONS, FooterAggregate, GroupCount, MAX_GROUP_FIELDS, MAX_SORT_KEYS, ROW_SIZES, ToolbarPopover } from "../../components/entities/GridToolbar";
import type { SortKey } from "../../components/entities/GridToolbar";
import type { Aggregation, RowSize } from "../../components/entities/GridToolbar";
import type { ViewExtras } from "../../components/entities/ViewOptions";
import type { DataView as SavedView, ViewConfig } from "../../api/dataViews";
import { ChangeNotice } from "../../components/ChangeNotice";
import { PermissionGuard } from "../../components/ui/PermissionGuard";
import { usePermissions } from "../../api/permissions";
import { assigneeWords, operationKinds } from "./policyWords";
import { classSlots, isAnyClass, parseModel } from "../models/linkml";
import { localId, textOf } from "../apps/QueryResultCard";
import { TypeLink } from "../models/ModelLinks";
import { useSourceOf } from "../models/ModelPage";
import { ProposeLink } from "../models/ModelsList";
import { deleteRow, LiveNotice, TrashPanel, trashKey, useLive } from "./DataViews";
import type { LiveChange } from "./DataViews";
import { SpaceDrift } from "./SpaceDrift";
import { TypeApi } from "./TypeApi";
import { SharePanel } from "./SharePanel";
import { ExportLinks, ImportRowsDialog } from "./ImportRows";
import { useWorkspace } from "../../components/layout/WorkspaceContext";
import { CommentsPanel } from "./Comments";
import { importSlotsOf, MAX_EXPORT_IDS } from "./importRows";
import { useOrgDomain } from "../../api/projects";
import { FormView } from "./FormView";
import { SpaceChecks } from "./SpaceChecks";
import { SpaceQuality } from "./SpaceQuality";
import { SpacePersonalData } from "./SpacePersonalData";
import { SpaceRetention } from "./SpaceRetention";
import { SpaceTemporalAudit } from "./SpaceTemporalAudit";
import { rulesOf } from "./retention";
import type { RetentionRule } from "./retention";
import {
  Alert,
  Badge,
  Button,
  buttonClass,
  Dialog,
  EmptyState,
  Icon,
  PageFailed,
  PageHeader,
  PageLoading,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeaderCell,
  TableRow,
  Tabs,
  tabPanelProps,
  Term,
} from "../../components/ui";
import { Card, ConfirmDialog, Field, Input, Select } from "../../components/ui";
import { ResourcePageFailed } from "../../components/ui/PageState";
import { SourceRemovalsPanel } from "../pipelines/PipelineSourceReport";
import { useUrlParam, useUrlText, useUrlUrn } from "../../navigation/urlState";
import { policyRefsOf } from "../../components/endpoints/policyBinding";

const SAMPLE_LIMIT = 3;

import { projectListQuery, spaceQuery } from "./preload";
import { entityTypesOf, modelsOfSpace, parseResultsCount, pickReadEndpoint, spaceOf } from "./spaceFacts";


type KeyValues = Record<string, unknown> & { id: string };

interface TypeInside {
  count?: number;
  samples: KeyValues[];
}

async function gatewayGet(slug: string, query: URLSearchParams): Promise<Response> {
  // A `Request` rather than a URL string, as `api/client.ts` sends: same origin, default mode.
  const response = await globalThis.fetch(
    new Request(endpointUrl(slug, `/ngsi-ld/v1/entities?${query.toString()}`), {
      headers: { Accept: "application/ld+json" },
    }),
  );
  if (!response.ok) {
    throw new ApiError(response.status, response.statusText || `HTTP ${response.status}`);
  }
  return response;
}

/**
 * The reads of one Endpoint, one after the other (T-3161): a space of fifteen types asked its
 * public Endpoint thirty questions at once, and the Endpoint's rate limit refused some with 429.
 */
const queues = new Map<string, Promise<unknown>>();
export function inTurn<T>(slug: string, read: () => Promise<T>): Promise<T> {
  const turn = (queues.get(slug) ?? Promise.resolve()).then(read, read);
  queues.set(slug, turn.catch(() => undefined));
  return turn;
}

/** What the gateway holds under one type: the live count and a few keyValues samples. */
async function fetchTypeInside(slug: string, type: string): Promise<TypeInside> {
  // One read answers both: the count rides in its header beside the samples (F-R2UI4-014).
  const sampled = await gatewayGet(
    slug,
    new URLSearchParams({ type, limit: String(SAMPLE_LIMIT), count: "true", options: "keyValues" }),
  );
  const count = parseResultsCount(sampled.headers);
  const body: unknown = await sampled.json();
  const samples = (Array.isArray(body) ? body : [])
    .filter(
      (item): item is KeyValues =>
        typeof item === "object" && item !== null && typeof (item as KeyValues).id === "string",
    )
    .slice(0, SAMPLE_LIMIT);
  return { count, samples };
}

/**
 * One entity in keyValues form as a single line of values a person reads (T-2760): a place as
 * its position, a name in the reader's language. It printed `location={"coordinates":…}`.
 */
const URN = /urn:ngsi-ld:[^\s,;]+/g;
const INSTANT = /\b\d{4}-\d{2}-\d{2}T\d{2}:\d{2}(?::\d{2}(?:\.\d+)?)?(?:Z|[+-]\d{2}:?\d{2})?/g;

/**
 * A sample value as a person reads it: a number to two decimals, an instant as a date and time,
 * a URN as its own name, never `urn:ngsi-ld:Pipeline:hel.fi:…` or `8.897155361050329`.
 */
export function readable(value: unknown, language?: string): string {
  const plain = isKeyValue(value) ? value.value : value;
  if (typeof plain === "number") {
    return new Intl.NumberFormat(language, { maximumFractionDigits: 2 }).format(plain);
  }
  const when = new Intl.DateTimeFormat(language, { dateStyle: "medium", timeStyle: "short" });
  return textOf(value, language)
    .replace(URN, (urn) => localId(urn))
    .replace(INSTANT, (instant) => {
      const at = new Date(instant);
      return Number.isNaN(at.getTime()) ? instant : when.format(at);
    });
}

function isKeyValue(value: unknown): value is { value: unknown } {
  return typeof value === "object" && value !== null && !Array.isArray(value) && "value" in value;
}

export function summarize(entity: KeyValues, language?: string): string {
  const attributes = Object.entries(entity)
    .filter(([key]) => key !== "id" && key !== "type" && key !== "@context")
    .slice(0, 4)
    .map(([key, value]) => {
      const text = readable(value, language);
      return `${key}: ${text.length > 40 ? `${text.slice(0, 39)}…` : text}`;
    });
  return attributes.join(" · ");
}

/** The entity's own name, its full id a click away (T-2760): the URN is five segments wide. */
function EntityId({ id }: { id: string }): JSX.Element {
  const { t } = useTranslation();
  const [copied, setCopied] = useState(false);
  return (
    <span className="inline-flex items-center gap-1">
      <span className="text-fg" title={id}>
        {localId(id)}
      </span>
      <Button
        size="sm"
        variant="ghost"
        aria-label={t("spaces.inside.copyId", { id })}
        title={copied ? t("endpoints.copied") : t("spaces.inside.copyId", { id })}
        onClick={() => {
          void navigator.clipboard?.writeText(id).then(() => setCopied(true));
        }}
      >
        <Icon name={copied ? "check" : "copy"} className="size-3.5" />
      </Button>
    </span>
  );
}

function TypeRow({
  project,
  space,
  slug,
  type,
  bytes,
}: {
  project: string;
  space: string;
  slug?: string;
  type: string;
  /** What the type's entities take in the broker's store (T-2890), when it measured them. */
  bytes?: number;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const signedIn = useIdentity() !== null;
  const inside = useQuery({
    queryKey: ["gateway", slug ?? "", "inside", type],
    enabled: slug !== undefined,
    retry: false,
    queryFn: () => inTurn(slug ?? "", () => fetchTypeInside(slug ?? "", type)),
  });
  const status = inside.error instanceof ApiError ? inside.error.status : undefined;
  const refused = status === 401 || status === 403 || status === 404;

  let count: JSX.Element | string;
  if (slug === undefined) {
    count = <span className="text-fg-subtle">—</span>;
  } else if (inside.isPending) {
    // caption sans, as the refusal: a monospace "Loading..." pushed SIZE off a phone (T-6156)
    count = <span className="font-sans text-caption text-fg-subtle">{t("app.loading")}</span>;
  } else if (inside.isError) {
    count =
      status === 429 ? (
        // The Endpoint's own rate limit, not a refusal: said, with the read one press away.
        <span className="inline-flex flex-wrap items-center gap-2 text-fg-muted">
          {t("spaces.inside.tooMany")}
          <Button size="sm" variant="ghost" onClick={() => void inside.refetch()}>
            {t("app.error.retry")}
          </Button>
        </span>
      ) : (
        // in the page's text face and caption size: a monospace sentence in the count column ran
        // three lines and pushed the size off a phone's screen (T-6145)
        <span className="inline-block max-w-44 font-sans text-caption text-fg-muted">
          {t(refused ? refusalKey(signedIn) : "app.error.generic")}
        </span>
      );
  } else {
    count = inside.data.count === undefined ? "—" : inside.data.count.toLocaleString(i18n.language);
  }

  return (
    <TableRow>
      {/* the samples sit under their type at every width, so a phone sees them too (T-6156) */}
      <TableCell className="align-top">
        {/* a long type name ends in "…" with the whole name on hover, never broken mid-word */}
        <span className="block max-w-64 truncate font-mono sm:max-w-none" title={type}>
          <TypeLink project={project} type={type} space={space} />
        </span>
        {inside.isSuccess ? <Samples samples={inside.data.samples} /> : null}
      </TableCell>
      {refused && bytes === undefined ? (
        // one sentence for a refused person, no count and no size to leave blank (T-6156)
        <TableCell colSpan={2} className="align-top font-sans text-caption text-fg-muted">
          {t(refusalKey(signedIn))}
        </TableCell>
      ) : (
        <>
          <TableCell align="right" className="align-top font-mono">{count}</TableCell>
          <TableCell align="right" className="align-top whitespace-nowrap tabular-nums">
            {bytes === undefined ? (
              <span className="text-fg-subtle">—</span>
            ) : (
              <span title={exactBytes(bytes, i18n.language)}>{sizeText(bytes, i18n.language)}</span>
            )}
          </TableCell>
        </>
      )}
    </TableRow>
  );
}

/** A refused read said for who is asking: anonymous, or a person without a read grant (T-6156). */
export function refusalKey(signedIn: boolean): "spaces.inside.noReadGrant" | "spaces.inside.notReadable" {
  return signedIn ? "spaces.inside.noReadGrant" : "spaces.inside.notReadable";
}

/** A type's sample entities, each with its own size, under the type's name (T-2760, T-6145). */
export function Samples({ samples }: { samples: KeyValues[] }): JSX.Element {
  const { t, i18n } = useTranslation();
  if (samples.length === 0) {
    return <p className="mt-1 text-caption text-fg-subtle">{t("spaces.inside.noEntities")}</p>;
  }
  return (
    <ul className="mt-1 space-y-1" aria-label={t("spaces.inside.samples")}>
      {samples.map((entity) => (
        <li key={entity.id} className="text-caption">
          <EntityId id={entity.id} />
          <SampleSize bytes={entityBytes(entity)} />
          {summarize(entity, i18n.language) ? (
            // its own line under the id, so a wrapped summary never starts with a stray "·"
            <span className="block text-fg-subtle [overflow-wrap:anywhere]">{summarize(entity, i18n.language)}</span>
          ) : null}
        </li>
      ))}
    </ul>
  );
}

/** One sample entity's size beside its id, read aloud with its label (T-6145). */
function SampleSize({ bytes }: { bytes: number }): JSX.Element {
  const { t, i18n } = useTranslation();
  return (
    <span
      className="ml-2 tabular-nums text-fg-muted"
      title={t("spaces.inside.sampleSize", { bytes: exactBytes(bytes, i18n.language) })}
    >
      <span className="sr-only">{t("spaces.inside.size")}: </span>
      {sizeText(bytes, i18n.language)}
    </span>
  );
}

function useProjectList(project: string, plural: string) {
  return useQuery(projectListQuery(project, plural));
}

/**
 * The whole space in the grid (T-1434; SP-04, SP-06): the space surface answers by the caller's own
 * grants, so this is what this person may read of the space, not one endpoint's view of it.
 *
 * A read is tried for the chosen type before the grid is built, because the surface answers a
 * caller with no grant with a 404 that says nothing about whether the space exists (SP-06) — and
 * then the page says where this person can see it instead. View only: a space surface publishes no
 * grant document (only `/api/endpoint/{slug}/access` does), and a cell that takes a value the
 * gateway will refuse loses the person's typing (UI-44).
 */
/** The space's endpoints whose grants are read for a writer: a space rarely has more. */
const MAX_ACCESS_READS = 10;
const NO_ENDPOINTS: Manifest[] = [];

/** Each endpoint's grant document, as one value `useQueries` keeps while none of them changes. */
function grantsOf(results: UseQueryResult<GrantDocument>[]): (GrantDocument | undefined)[] {
  return results.map((result) => result.data);
}

/** Each endpoint's decisions per attribute, as one value `useQueries` keeps while none of them changes. */
function tablesOf(results: UseQueryResult<AttributeAccess[]>[]): { rows: (AttributeAccess[] | undefined)[]; asking: boolean } {
  return { rows: results.map((result) => result.data), asking: results.some((result) => result.isFetching) };
}

/**
 * The first endpoint whose decisions let the person correct attributes of the type, with those
 * attributes (T-5528): the gateway's answer, less the formula fields a pipeline writes (DM-80).
 */
function writerOf(
  tables: (AttributeAccess[] | undefined)[],
  endpoints: Manifest[],
  slots: FilterSlot[],
): { slug: string; attrs: string[] } | null {
  for (const [at, rows] of tables.entries()) {
    const slug = slugOf(endpoints[at]);
    const attrs = writableOf(rows).filter((attr) => slots.some((slot) => slot.name === attr && !slot.formula));
    if (slug && attrs.length > 0) return { slug, attrs };
  }
  return null;
}

/** Whether a DataModel ships the generated JSON Schema the gateway serves its attributes from (DM-61). */
function hasJsonSchema(model: Manifest | undefined): boolean {
  const artifacts = model?.spec.artifacts;
  return typeof artifacts === "object" && artifacts !== null && typeof (artifacts as { jsonSchema?: unknown }).jsonSchema === "string";
}

/** An endpoint's slug, which its gateway path is named by (EP-55). */
function slugOf(endpoint: Manifest | undefined): string | undefined {
  return typeof endpoint?.spec.slug === "string" ? endpoint.spec.slug : undefined;
}

/** The probe statuses that mean the space surface answers this person nothing (T-5601). */
const SURFACE_REFUSED = new Set([401, 403, 404]);
/** The longest Retry-After the Retry button waits for: a page stays usable after five minutes. */
const MAX_RETRY_WAIT_S = 300;

/**
 * Why the space's rows did not come. Only a refusal (401, 403, 404) means the person reads this
 * space through its endpoints, so only it lists them; a failure to reach the surface (0, 408,
 * 429, 5xx) is a retry, and a 429's Retry-After holds the Retry button until it has passed
 * (T-5093, T-5601).
 */
function ProbeFailure({
  error,
  endpoints,
  retrying,
  onRetry,
}: {
  error: Error;
  endpoints: Manifest[];
  retrying: boolean;
  onRetry: () => void;
}): JSX.Element {
  const { t } = useTranslation();
  const status = error instanceof SourceError ? error.status : 0;
  const wait = Math.min(error instanceof SourceError ? (error.retryAfter ?? 0) : 0, MAX_RETRY_WAIT_S);
  const [waiting, setWaiting] = useState(wait > 0);
  // The parent keys this by the error's time, so each new refusal starts its own wait.
  useEffect(() => {
    if (wait <= 0) return undefined;
    const timer = setTimeout(() => setWaiting(false), wait * 1000);
    return () => clearTimeout(timer);
  }, [wait]);

  if (SURFACE_REFUSED.has(status)) {
    return (
      <div className="text-body text-fg-muted">
        <p>{t("spaces.inside.dataThroughEndpoints")}</p>
        <ul className="mt-1 flex flex-wrap gap-2">
          {endpoints.map((endpoint) => {
            const slug = slugOf(endpoint);
            return (
              <li key={endpoint.metadata.name}>
                {slug ? (
                  <EndpointLink href={endpointUrl(slug, "")}>{endpoint.metadata.name}</EndpointLink>
                ) : (
                  <Badge mono>{endpoint.metadata.name}</Badge>
                )}
              </li>
            );
          })}
        </ul>
      </div>
    );
  }
  const busy = status === 0 || status === 408 || status === 429;
  const hint = !busy
    ? t("spaces.inside.dataFailedHint", { status, detail: error.message })
    : wait > 0
      ? t("spaces.inside.dataBusyWait", { count: wait })
      : t("spaces.inside.dataBusy");
  return (
    <Alert
      tone="danger"
      title={t("spaces.inside.dataFailed")}
      actions={
        <Button
          size="sm"
          variant="secondary"
          onClick={onRetry}
          loading={retrying}
          disabled={waiting}
          disabledReason={waiting ? t("spaces.inside.dataBusyWaiting") : undefined}
        >
          {t("app.error.retry")}
        </Button>
      }
    >
      {hint}
    </Alert>
  );
}

function SpaceData({
  project,
  space,
  types,
  endpoints,
  model,
  retention,
}: {
  project: string;
  space: string;
  types: string[];
  endpoints: Manifest[];
  /** The space's DataModel: its enum slots are filtered by picking (UI-86). */
  model?: Manifest;
  /** The space's retention rules, which the import names for the imported rows (T-4822). */
  retention?: RetentionRule[];
}): JSX.Element {
  const { t, i18n } = useTranslation();
  // The type is in the address like the view, so back, reload and a sent link open it (T-4825).
  const [type, setType] = useUrlParam<string>("type", types[0] ?? "", types);
  // Rows per page (owner 2026-11-01): 100 by default, 5000 at most, read 1000 at a time.
  const [pageRows, setPageRows] = useUrlParam<string>("rows", "100", PAGE_SIZES.map(String));
  // The open row is in the address too, so a link opens it in its dialog (T-4828), with its tab and
  // History attribute (T-5602); the dialog reads a tab or attribute it does not know as its default.
  const [openRow, setOpenRow] = useUrlUrn("row", ["tab", "attr"]);
  const [rowTab, setRowTab] = useUrlText("tab");
  const [rowAttr, setRowAttr] = useUrlText("attr");
  const detailPlace = useMemo(() => ({ tab: rowTab, attr: rowAttr }), [rowTab, rowAttr]);
  const onDetailPlace = useCallback(
    (next: { tab: string; attr: string }) => {
      setRowTab(next.tab === "fields" ? null : next.tab);
      setRowAttr(next.tab === "history" || next.tab === "metadata" ? next.attr : null);
    },
    [setRowTab, setRowAttr],
  );
  // "+ Row": the type's form in a dialog, creating through the space surface (T-4828).
  const [creating, setCreating] = useState(false);
  // The type's alerts are being chosen (T-3261).
  const [alertingType, setAlertingType] = useState(false);
  const modelSource = useModelSource(project, model);
  const enums = useMemo(
    () => enumsOfModel(modelSource, type, i18n.language),
    [modelSource, type, i18n.language],
  );
  const modelRelations = useMemo(() => relationsOfModel(modelSource, type), [modelSource, type]);
  const rules = useMemo(() => rulesOfModel(modelSource, type), [modelSource, type]);
  const slots = useMemo(() => filterSlotsOf(modelSource, type), [modelSource, type]);
  // The grid, or a view over the same rows (ADR-N-042 §3.2): the other views share one filter.
  // The view is in the address, so back, reload and a sent link open it (T-3239).
  const [view, setView] = useUrlParam<DataView>("view", "grid", DATA_VIEWS);
  const [q, setQ] = useState<string | undefined>(undefined);
  // A field is a slot of the type's class: offered only when the space's model declares the type.
  const ownClass = useMemo(
    () => modelSource !== undefined && parseModel(modelSource).classes.some((c) => c.name === type),
    [modelSource, type],
  );
  const [adding, setAdding] = useState(false);
  // Every Change the field dialog proposed: a formula field is two (DM-80).
  const [proposed, setProposed] = useState<Change[]>([]);
  // Import and export of the type (T-3109): rows created through the gateway with this session.
  const orgDomain = useOrgDomain(project);
  const importSlots = useMemo(() => importSlotsOf(modelSource, type), [modelSource, type]);
  const [importing, setImporting] = useState(false);
  const [imported, setImported] = useState(0);
  const queryClient = useQueryClient();

  // The saved view applied, and what of the grid a view keeps: the typed query and the order
  // (API/01 §30). The filter row's query arrives through `onQuery`.
  const [saved, setSaved] = useState<SavedView | null>(null);
  const [gridView, setGridView] = useState<Pick<GridState, "filterText" | "sort">>({ filterText: null, sort: null });
  const [asked, setAsked] = useState<{ q?: string; idPattern?: string }>({});
  const phone = useIsPhone();
  // What the view hides, colours and groups by (T-3099), and the group the grid is narrowed to.
  const [extras, setExtras] = useState<ViewExtras>(() => readLayout(project, space, type) ?? {});
  const [group, setGroup] = useState<string | null>(null);
  const [seen, setSeen] = useState<string[]>([]);
  // A form view's fields, conditions and prefill (T-3103, API/01 §30): its `settings`.
  // The grid's toolbar (T-4883): the filter panel's conditions, the search, the row size and each
  // column's footer, all but the search kept in the view's settings.
  const [filters, setFilters] = useState<FilterGroup>(EMPTY_GROUP);
  const [filterColumns, setFilterColumns] = useState<FilterColumn[]>([]);
  // The search is in the address in place (T-5602), so a reload or a sent link asks it again.
  const [urlSearch, setUrlSearch] = useUrlText("q");
  const [searchText, setSearchText] = useState(urlSearch ?? "");
  const [search, setSearch] = useState(urlSearch ?? "");
  const [rowSize, setRowSize] = useState<RowSize>("medium");
  const [aggregations, setAggregations] = useState<Record<string, Aggregation>>({});
  // The Sort and Group panels (T-5188): the keys after the header's own sort, and the fields the
  // rows are grouped by. `steered` hands the order to this page once the panel set it.
  const [thenBy, setThenBy] = useState<SortKey[]>([]);
  const [groupBy, setGroupBy] = useState<string[]>([]);
  const [steered, setSteered] = useState(false);
  // The query box under the rows, opened from the toolbar's "Query" (T-5093).
  const [showQuery, setShowQuery] = useState(false);
  const [selectedRows, setSelectedRows] = useState<string[]>([]);
  const [loadedRows, setLoadedRows] = useState<RichRow[]>([]);
  // Live changes (T-5186): a change of the type reads the rows again where they are, and the rows
  // that differ from what was shown flash for a few seconds, the reason named for a screen reader.
  const [reloadKey, setReloadKey] = useState(0);
  const [live, setLive] = useState<LiveChange | null>(null);
  const [flashed, setFlashed] = useState<ReadonlySet<string>>(NO_ROWS);
  const shownBefore = useRef<Map<string, string> | null>(null);
  const applyView = useCallback((next: SavedView | null, forType: string = type) => {
    setSaved(next);
    const settings = (next?.kind === "grid" ? next.config.settings : undefined) as Record<string, unknown> | undefined;
    const tree = readGroup(settings?.filters);
    setFilters(tree ?? EMPTY_GROUP);
    setRowSize((ROW_SIZES as readonly unknown[]).includes(settings?.rowSize) ? (settings?.rowSize as RowSize) : "medium");
    setAggregations(readAggregations(settings?.aggregations));
    setGroupBy(readGroupBy(settings?.groupBy) ?? (next?.config.group ? [next.config.group] : []));
    setThenBy((next?.config.sort ?? []).slice(1, MAX_SORT_KEYS).map((key) => ({ attr: key.attr, dir: key.desc ? "desc" : "asc" })));
    setSteered(false);
    setSearchText("");
    setSearch("");
    setSelectedRows([]);
    // A saved view opens as the kind it was saved as; one of a kind the page no longer has (a board,
    // a calendar, T-6210) opens as the grid, its stored row untouched.
    if (next) setView((DATA_VIEWS as readonly string[]).includes(next.kind) ? (next.kind as DataView) : "grid");
    setQ(next?.config.q ?? undefined);
    // "All rows" keeps the person's own columns in this browser; a saved view brings its own.
    const kept = next ? undefined : readLayout(project, space, forType);
    setExtras({
      hidden: next ? next.config.hidden : kept?.hidden,
      colour: next?.config.colour,
      group: next?.config.group,
      width: next ? next.config.width : kept?.width,
      order: next ? next.config.order : kept?.order,
      labels: next ? next.config.labels : kept?.labels,
    });
    setGroup(null);
    const first = next?.config.sort?.[0];
    setGridView({
      // A view saved with the filter panel holds its conditions; its `q` is their translation.
      filterText: tree ? null : (next?.config.q ?? null),
      sort: first ? { attr: first.attr, dir: first.desc ? "desc" : "asc" } : next ? null : rememberedSort(project, forType),
    });
  }, [setView, project, space, type]);
  const pickType = (next: string) => {
    if (next === type) return;
    setType(next);
    applyView(null, next);
    setSeen([]);
  };
  const savedViews = useQuery({ queryKey: dataViewsKey(project, space), retry: false, queryFn: () => listViews(project, space) });
  const hasSavedViews = (savedViews.data ?? []).some((each) => each.type === type);
  // The Sort panel's keys: the header's sort first, the rest this page's own.
  const sortKeys = useMemo(
    () => (gridView.sort ? [gridView.sort, ...thenBy.filter((key) => key.attr !== gridView.sort?.attr)] : thenBy),
    [gridView.sort, thenBy],
  );
  const onSortKeys = useCallback((next: SortKey[]) => {
    setSteered(true);
    setGridView((before) => ({ ...before, sort: next[0] ?? null }));
    setThenBy(next.slice(1));
  }, []);
  const onGridState = useCallback((next: GridState) => {
    setGridView((before) =>
      before.filterText === next.filterText && before.sort?.attr === next.sort?.attr && before.sort?.dir === next.sort?.dir
        ? before
        : { filterText: next.filterText, sort: next.sort },
    );
  }, []);
  useEffect(() => {
    const wait = setTimeout(() => setSearch(searchText), 400);
    return () => clearTimeout(wait);
  }, [searchText]);
  useEffect(() => setUrlSearch(search), [search, setUrlSearch]);
  const panelQ = useMemo(() => queryFromGroup(filterColumns, filters), [filterColumns, filters]);
  // The search asks the type's text fields at the endpoint, a name in any of its languages.
  const searchQ = useMemo(
    () => searchQuery(search, filterColumns),
    [search, filterColumns],
  );
  const onFilterColumns = useCallback(
    (next: FilterColumn[]) => setFilterColumns((before) => (JSON.stringify(before) === JSON.stringify(next) ? before : next)),
    [],
  );
  const current = useMemo<ViewConfig>(
    () => ({
      ...(saved?.config ?? {}),
      ...extras,
      q: view === "grid" ? andQ(asked.q, panelQ) : q,
      sort: sortKeys.map((key) => ({ attr: key.attr, desc: key.dir === "desc" })),
      // The outer group keeps the older `group` (the counts above the grid) where it is an enum.
      group: groupBy[0] !== undefined && enums[groupBy[0]] ? groupBy[0] : undefined,
      ...(view === "grid"
        ? { settings: { filters, rowSize, aggregations, ...(groupBy.length > 0 ? { groupBy } : {}) } as unknown as Record<string, never> }
        : {}),
    }),
    [saved, extras, view, asked.q, panelQ, q, sortKeys, groupBy, enums, filters, rowSize, aggregations],
  );
  // The attributes a view can hide: the model's slots of the type and whatever the rows carry.
  const onRows = useCallback((rows: RichRow[]) => {
    setLoadedRows(rows);
    if (shownBefore.current) {
      setFlashed(changedRows(shownBefore.current, rows));
      shownBefore.current = null;
    }
    setSeen((before) => {
      const next = [...new Set([...before, ...attributesOf(rows)])];
      return next.length === before.length ? before : next;
    });
  }, []);
  // The slots of the type's class, in the model's order: what a form asks for (T-3103).
  const typeSlots = useMemo(() => {
    if (modelSource === undefined) return [];
    const parsed = parseModel(modelSource);
    const cls = parsed.classes.find((c) => c.name === type);
    return cls ? classSlots(parsed, cls) : [];
  }, [modelSource, type]);
  // The columns in the model's display order (DM-98), of what the person's own answer carries
  // (DM-102): a slot their grant leaves out never becomes a column, a label or a chip's end.
  const attributes = useMemo(() => displayOrder(typeSlots, seen), [typeSlots, seen]);
  const relations = useMemo(
    () => Object.fromEntries(Object.entries(modelRelations).filter(([attr]) => seen.includes(attr))),
    [modelRelations, seen],
  );
  useLive(project, space, view === "grid" ? type : "", (change) => {
    setLive(change);
    // Before the first rows arrive there is nothing to compare: the read alone is enough.
    shownBefore.current = loadedRows.length > 0 ? rowSignatures(loadedRows) : null;
    setReloadKey((n) => n + 1);
  });
  useEffect(() => {
    if (flashed.size === 0) return;
    const wait = setTimeout(() => setFlashed(NO_ROWS), FLASH_MS);
    return () => clearTimeout(wait);
  }, [flashed]);
  // The first colour rule a row matches marks it, with the rule as the reason a screen reader hears;
  // a row a live change just touched is marked first.
  const rowTone = useCallback(
    (row: RichRow) => {
      if (flashed.has(row.id)) return { tone: "info", label: t("spaces.toolbar.changedLive") };
      const rule = (extras.colour ?? []).find((each) => each.when.trim() !== "" && matchesQ(row, each.when) === true);
      return rule ? { tone: rule.colour, label: t("spaces.saved.toneLabel", { when: rule.when }) } : undefined;
    },
    [extras.colour, flashed, t],
  );
  // What the grid asks beyond its own query: the panel, the search and the chosen group (T-4883).
  const gridQuery = useMemo(() => {
    const asked = andQ(panelQ, searchQ, groupBy[0] !== undefined && group !== null ? groupTerm(groupBy[0], group) : undefined);
    return asked ? { q: asked } : undefined;
  }, [panelQ, searchQ, groupBy, group]);
  const source = useMemo(
    () => sourceFor({ kind: "space", space }, originTransport(), i18n.language),
    [space, i18n.language],
  );
  // Where a correction is written: the first of the space's endpoints whose grant, asked of the
  // gateway once per endpoint, names a write on this type (T-4826). The grid reads the space and
  // writes through that endpoint with the person's session, so its Policy decides every value
  // (EP-55, UI-67); a person no endpoint lets write, a viewer, gets no editor at all.
  // Those reads cost two per endpoint (20 on Helsinki, 1-1.7 s each), so they are asked only once the
  // person turns on Edit, selects rows or opens one, whose Correct values tab needs the writer,
  // never on load (T-6209): reading needs none of them.
  const [editing, setEditing] = useState(false);
  const askAccess = editing || selectedRows.length > 0 || Boolean(openRow);
  const accessed = askAccess ? endpoints.slice(0, MAX_ACCESS_READS) : NO_ENDPOINTS;
  const grants = useQueries({
    queries: accessed.map((endpoint) => accessQuery(slugOf(endpoint))),
    combine: grantsOf,
  });
  // Entities are not part of a copy (T-4868): inside one the grid reads the live space, so it
  // offers no write at all; the copy's own data lives in its preview (Try it).
  const { name: copy } = useWorkspace();
  // Decisions are asked only for a type with a field a person could correct: a class of no slot
  // (a draft model, like a catalogue's) has no attribute an endpoint serves, and the gateway
  // answers that question with a 400 (T-5753). The gateway reads the served attributes from the
  // model's generated JSON Schema, so a model that ships none (the catalogue's) is not asked
  // either, whatever slots its LinkML names (T-5769).
  const correctable = !copy && hasJsonSchema(model) && slots.some((slot) => !slot.formula);
  const tables = useQueries({
    queries: accessed.map((endpoint) => attributeAccessQuery(slugOf(endpoint), correctable && type ? type : undefined)),
    combine: tablesOf,
  });
  const writer = useMemo(
    () => (copy ? null : writerOf(tables.rows, endpoints, slots)),
    [copy, tables.rows, endpoints, slots],
  );
  // The grid reads again whenever its source changes, and the writer's grant lands after the first
  // read: the source keeps one identity and its patch asks the current writer, so the grant moves
  // where a correction goes without a second read of the type (T-6161, 4.4 s twice on Helsinki).
  // A viewer gets no editor anyway: the grid edits only with `mode: "edit"`, the views only the
  // attributes `editable` names, both empty without a writer.
  // A plain value of a person who may write it: what the row's "Correct values" tab offers (T-5326).
  const writtenSlots = slots.filter((slot) => writer?.attrs.includes(slot.name) && slot.kind === "Property").map((slot) => slot.name);
  const writerSlug = useRef<string | null>(null);
  useEffect(() => {
    writerSlug.current = writer?.slug ?? null;
  }, [writer]);
  const gridSource = useMemo(
    () => ({
      ...source,
      patch: (id: string, attrs: Record<string, unknown>, reason?: string) => {
        const slug = writerSlug.current;
        const write = slug === null ? undefined : endpointSource(slug, originTransport(), i18n.language).patch;
        return write ? write(id, attrs, reason) : Promise.reject(new SourceError(403, t("spaces.views.readOnly")));
      },
    }),
    [source, i18n.language, t],
  );

  const probe = useQuery({
    queryKey: ["space-surface", space, type],
    enabled: type !== "",
    retry: false,
    queryFn: async () => {
      // Only whether the surface answers: a count would scan the whole type once more (T-6161).
      await source.query({ type }, { offset: 0, limit: 1, count: false });
      return true;
    },
  });
  // The views are drawn while the probe is under way (T-5598): waiting for it put one more round
  // trip before the grid's own read, 0.6 s on a phone's network. A refusal still replaces them.
  const readable = type !== "" && !probe.isError;
  // A phone hides the grid's filter row (T-6463), the Type column's own place, so it keeps the picker.
  const typeInGrid = view === "grid" && readable && !hasSavedViews && !phone;

  // The type and its attributes as the model titles them, the key where it gives no title; an
  // entity is led by its name, its URN the open button's tooltip (T-4264, T-4160 #6).
  const titleOf = useCallback(
    (name: string) => {
      const cls = modelSource === undefined ? undefined : parseModel(modelSource).classes.find((c) => c.name === name);
      return localized(cls?.title, i18n.language, name);
    },
    [modelSource, i18n.language],
  );
  const typeTitle = titleOf(type);
  const columns = useMemo(() => {
    const titled = (attr: string) => columnLabel(typeSlots.find((slot) => slot.name === attr), attr, i18n.language);
    const named = attributes.includes("name");
    if (attributes.length === 0) return undefined;
    return [
      ...(named ? [{ attr: "name", pinned: true, label: titled("name") }] : []),
      ...attributes.filter((attr) => attr !== "name").map((attr) => ({ attr, label: titled(attr) })),
    ];
  }, [attributes, typeSlots, i18n.language]);
  // What the header menu changed, as the view keeps it (T-4827): on "All rows" in this browser too.
  const layout = useMemo(() => ({ order: extras.order, width: extras.width, labels: extras.labels }), [extras.order, extras.width, extras.labels]);
  const onLayout = useCallback(
    (next: { order?: readonly string[]; width?: Readonly<Record<string, number>>; labels?: Readonly<Record<string, string>> }) =>
      setExtras((before) => ({ ...before, order: next.order ? [...next.order] : before.order, width: next.width ? { ...next.width } : before.width, labels: next.labels ? { ...next.labels } : before.labels })),
    [],
  );
  const onHide = useCallback((attr: string) => setExtras((before) => ({ ...before, hidden: [...new Set([...(before.hidden ?? []), attr])] })), []);
  // The kept layout is read for each type before anything is written for it: the type can arrive
  // after the first render (the model loads), and writing first would store an empty layout. The
  // read happens while rendering, React's way to follow a changed input without an extra pass.
  const layoutKey = `${project}\u0000${space}\u0000${type}`;
  const [loadedFor, setLoadedFor] = useState(type === "" ? null : layoutKey);
  if (saved === null && type !== "" && loadedFor !== layoutKey) {
    setLoadedFor(layoutKey);
    const kept = readLayout(project, space, type);
    if (kept) setExtras((before) => ({ ...before, ...kept }));
  }
  useEffect(() => {
    if (saved !== null || type === "" || loadedFor !== layoutKey) return;
    writeLayout(project, space, type, { hidden: extras.hidden, width: extras.width, order: extras.order, labels: extras.labels });
  }, [saved, loadedFor, layoutKey, project, space, type, extras.hidden, extras.width, extras.order, extras.labels]);
  // A chip opens its target in this grid, read through the person's own grant (G-R2GRID2-017).
  const config = useMemo(() => {
    if (type === "") {
      return null;
    }
    return (
      parseGridConfig({
        source: { kind: "space", space },
        type,
        ...(columns ? { columns } : {}),
        pageSize: Number(pageRows),
        // A selection asks for the writer too (the bulk bar's fields), but only Edit turns the cells into editors.
        ...(editing && writer ? { mode: "edit" as const, editableAttrs: writer.attrs } : { mode: "view" as const }),
        history: { enabled: true },
      }).config ?? null
    );
  }, [space, type, columns, editing, writer, pageRows]);

  // The toolbar's actions, in its row for the other views and under "More" for the grid (T-4883).
  const addField =
    model && modelSource !== undefined && ownClass ? (
      <PermissionGuard project={project} kind="DataModel" verb="propose">
        <Button className="w-fit" onClick={() => setAdding(true)}>
          {t("spaces.fields.add")}
        </Button>
      </PermissionGuard>
    ) : null;
  const addRow =
    typeSlots.length > 0 ? (
      // The same right as Import beside it; the space's Policy decides the write itself.
      <PermissionGuard project={project} kind="ContextSpace" verb="propose">
        <Button size="sm" onClick={() => setCreating(true)} disabled={!!copy} disabledReason={t("spaces.inside.copyLive")}>
          {t("spaces.inside.addRow")}
        </Button>
      </PermissionGuard>
    ) : null;
  const importButton =
    importSlots.length > 0 ? (
      // The same right as the upload link: the space has no grant document of its own (T-4263).
      <PermissionGuard project={project} kind="ContextSpace" verb="propose">
        <Button variant="secondary" onClick={() => setImporting(true)} disabled={!!copy} disabledReason={t("spaces.inside.copyLive")}>
          {t("spaces.import.open")}
        </Button>
      </PermissionGuard>
    ) : null;
  const alertsButton = (
    <Button variant="ghost" onClick={() => setAlertingType(true)}>
      {t("alerts.forType", { type: typeTitle })}
    </Button>
  );
  const filterTitle = (column: FilterColumn) => {
    const title = columnLabel(typeSlots.find((slot) => slot.name === column.attr), column.attr ?? column.key, i18n.language);
    return column.meta ? `${title} · ${t(`entityGrid.${column.meta}`)}` : title;
  };
  // What the Sort and Group panels offer: the fields the person's own answer carries (DM-102).
  const panelFields = attributes.map((attr) => ({
    value: attr,
    label: columnLabel(typeSlots.find((slot) => slot.name === attr), attr, i18n.language),
  }));
  const viewTabs = (
    <Tabs
      id="space-data-view"
      label={t("spaces.views.label")}
      variant="pill"
      tabs={DATA_VIEWS.map((value) => ({ value, label: t(`spaces.views.kind.${value}`) }))}
      value={view}
      onChange={setView}
    />
  );
  const rowNumberLabels = { selectRow: t("spaces.toolbar.selectRow"), selectAll: t("spaces.toolbar.selectAll") };

  if (types.length === 0) {
    return (
      <div className="flex flex-col gap-2">
        <p className="text-body text-fg-muted">{t("spaces.inside.dataNoTypes")}</p>
        {/* A space with no type yet takes its first rows from a CSV (DM-81). */}
        <PermissionGuard project={project} kind="ContextSpace" verb="propose">
          <Link to="/projects/$project/spaces/upload" params={{ project }} search={{ space }} className="focus-ring w-fit text-body text-primary-soft-fg underline-offset-2 hover:underline">
            {t("spaces.upload.open")}
          </Link>
        </PermissionGuard>
      </div>
    );
  }
  return (
    // Baserow's shape (T-4825): the types and their saved views on the left, the rows beside them.
    // The type is picked in the grid's Type column (owner 2026-11-01); the list stays for the
    // other views, for saved views, which the grid has no place for, and while no grid is drawn (a
    // type whose read failed would otherwise leave no way to another type).
    <div className={typeInGrid ? "grid gap-4" : "grid gap-4 lg:grid-cols-[14rem_minmax(0,1fr)]"}>
      {typeInGrid ? null : <TypeList
        project={project}
        space={space}
        types={types}
        type={type}
        titleOf={titleOf}
        selected={saved}
        onType={pickType}
        onView={applyView}
      />}
      <div className="flex min-w-0 flex-col gap-3">
      {copy ? (
        <Alert tone="info" role="status" title={t("spaces.inside.copyLive")}>
          {t("spaces.inside.copyLiveHint")}{" "}
          <Link
            to="/projects/$project/workspaces/$name/try-it"
            params={{ project, name: copy }}
            className="focus-ring underline hover:no-underline"
          >
            {t("workspaces.bar.tryIt")}
          </Link>
        </Alert>
      ) : null}
      {model && modelSource !== undefined && ownClass ? (
        <div className="flex flex-col gap-2">
          <AddFieldDialog
            project={project}
            modelName={model.metadata.name}
            source={modelSource}
            type={type}
            open={adding}
            onOpenChange={(next) => {
              if (next) setProposed([]);
              setAdding(next);
            }}
            onProposed={(change) => setProposed((before) => [...before, change])}
            space={space}
            endpoints={endpoints}
            orgDomain={orgDomain}
          />
          {proposed.map((change) => (
            <ChangeNotice key={change.metadata.name} change={change} project={project} />
          ))}
        </div>
      ) : null}

      {probe.isError ? (
        <ProbeFailure key={probe.errorUpdatedAt} error={probe.error} endpoints={endpoints} retrying={probe.isFetching} onRetry={() => void probe.refetch()} />
      ) : null}
      {/* The view switch sits in the toolbar's row (owner 2026-10-30: more height for the rows);
          the API view has no toolbar, so there it stands alone. */}
      {readable && view === "api" ? viewTabs : null}
      {readable && view !== "api" ? (
        <>
          <Dialog
            open={creating}
            onOpenChange={setCreating}
            title={t("spaces.inside.addRowTitle", { type: typeTitle })}
            closeLabel={t("app.close")}
            size="md"
          >
            <FormView
              space={space}
              type={type}
              slots={typeSlots}
              enums={enums}
              search={new URLSearchParams()}
              onCreated={() => setImported((n) => n + 1)}
            />
          </Dialog>
          {alertingType ? (
            <AlertDialog
              project={project}
              scope="type"
              target={`${space}/${type}`}
              label={type}
              onClose={() => setAlertingType(false)}
            />
          ) : null}
          <ImportRowsDialog
            open={importing}
            onOpenChange={setImporting}
            target={{ type, orgDomain, space }}
            slots={importSlots}
            send={originTransport()}
            retention={retention}
            onImported={() => {
              setImported((n) => n + 1);
              void queryClient.invalidateQueries({ queryKey: ["space-view-rows", space] });
            }}
          />
        </>
      ) : null}
      {readable && !copy ? <SourceRemovalsPanel project={project} space={space} /> : null}
      {readable && !copy ? (
        <TrashPanel
          project={project}
          space={space}
          restore={async (entity) => {
            const answer = await originTransport()({
              method: "POST",
              path: `/cs/${encodeURIComponent(space)}/ngsi-ld/v1/entities`,
              body: entity,
              reason: t("spaces.inside.restoreReason"),
            });
            if (answer.status < 200 || answer.status >= 300) {
              const detail = (answer.body as { detail?: unknown; title?: unknown } | undefined) ?? {};
              throw new Error(String(detail.detail ?? detail.title ?? `HTTP ${answer.status}`));
            }
          }}
        />
      ) : null}
      {/* One toolbar row for every view, as Baserow keeps it (T-5093): the grid's own tools first,
          the row, share and the rest of the type's actions at its end. */}
      {readable && view !== "api" ? (
        <div className="flex flex-wrap items-center gap-1 border-b border-border pb-2" data-testid="grid-toolbar">
          <div className="mr-2">{viewTabs}</div>
          {/* A saved view names itself; "All rows" said nothing the tabs did not (owner 2026-10-30: one row). */}
          {saved ? (
            <span className="mr-2 max-w-48 truncate text-body font-semibold" title={saved.title}>
              {saved.title}
            </span>
          ) : null}
          {view === "grid" ? (
            <DataToolbar
              filterColumns={filterColumns}
              filterTitle={filterTitle}
              filters={filters}
              onFilters={setFilters}
              attributes={attributes}
              enums={enums}
              extras={extras}
              onExtras={setExtras}
              fields={panelFields}
              sortKeys={sortKeys}
              onSortKeys={onSortKeys}
              groupBy={groupBy}
              onGroupBy={(next) => {
                setGroupBy(next);
                setGroup(null);
              }}
              rowSize={rowSize}
              onRowSize={setRowSize}
              searchText={searchText}
              onSearchText={setSearchText}
              showQuery={showQuery}
              onShowQuery={setShowQuery}
            />
          ) : null}
          <div className="ml-auto flex items-center gap-1">
            {!copy && correctable && endpoints.length > 0 ? (
              <Button
                variant={editing ? "primary" : "secondary"}
                size="sm"
                aria-pressed={editing}
                onClick={() => setEditing((on) => !on)}
              >
                {t("spaces.toolbar.edit")}
              </Button>
            ) : null}
            {addRow}
            <PermissionGuard project={project} kind="Endpoint" verb="propose">
              <ToolbarPopover label={t("spaces.toolbar.share")} wide end outlined={phone}>
                <SharePanel
                  key={type}
                  project={project}
                  space={space}
                  type={type}
                  attributes={slots.map((slot) => slot.name)}
                  shownAtFirst={slots.map((slot) => slot.name).filter((attr) => !(extras.hidden ?? []).includes(attr))}
                  open
                />
              </ToolbarPopover>
            </PermissionGuard>
            <ToolbarPopover label={t("spaces.toolbar.more")} wide end outlined={phone}>
              <div className="flex flex-col gap-3">
                {config ? (
                  <ViewBar
                    project={project}
                    space={space}
                    type={type}
                    kind={view}
                    selected={saved}
                    onSelect={applyView}
                    current={current}
                    unsaved={view === "grid" && asked.idPattern ? t("spaces.saved.idNotKept") : undefined}
                  />
                ) : null}
                <div className="flex flex-wrap items-end gap-2 border-t border-border pt-3">
                  {addField}
                  {importButton}
                  {alertsButton}
                  <ExportLinks
                    endpoints={endpoints}
                    type={type}
                    q={view === "grid" ? andQ(asked.q, gridQuery?.q) : q}
                    idPattern={view === "grid" ? asked.idPattern : undefined}
                    attrs={slots.map((slot) => slot.name)}
                  />
                </div>
              </div>
            </ToolbarPopover>
          </div>
        </div>
      ) : null}
      {readable && editing && !writer ? (
        <Alert tone="info" role="status">
          {t(tables.asking ? "spaces.toolbar.editChecking" : "spaces.toolbar.editNone", { type: typeTitle })}
        </Alert>
      ) : null}
      {readable && view === "grid" ? <FilterChips columns={filterColumns} titleOf={filterTitle} value={filters} onChange={setFilters} /> : null}
      {readable && config && view === "grid" && selectedRows.length > 0 ? (
        <BulkBar
          project={project}
          space={space}
          source={source}
          count={selectedRows.length}
          rows={loadedRows.filter((row) => selectedRows.includes(row.id))}
          canDelete={grants.some((doc) => mayDelete(doc, type))}
          writable={writer ? slots.filter((slot) => writer.attrs.includes(slot.name) && slot.kind === "Property" && !slot.formula) : []}
          writeSource={gridSource}
          onChanged={() => setReloadKey((n) => n + 1)}
          exportLinks={
            <ExportLinks
              endpoints={endpoints}
              type={type}
              q={undefined}
              attrs={slots.map((slot) => slot.name)}
              ids={selectedRows.slice(0, MAX_EXPORT_IDS)}
              label={t("spaces.toolbar.exportSelected")}
            />
          }
          tooManyToExport={selectedRows.length > MAX_EXPORT_IDS}
          onClear={() => setSelectedRows([])}
          onDeleted={(kept) => {
            // The refused rows stay checked, so the bar keeps naming them and they can be tried again.
            setSelectedRows(kept);
            setImported((n) => n + 1);
          }}
        />
      ) : null}
      {readable && config && view === "grid" ? <LiveNotice change={live} /> : null}
      {readable && config && view === "grid" && groupBy[0] !== undefined && enums[groupBy[0]] ? (
        <GroupCounts
          scope={`${project}/${space}`}
          source={source}
          type={type}
          attr={groupBy[0]}
          options={enums[groupBy[0]]}
          q={andQ(asked.q, panelQ, searchQ)}
          chosen={group}
          onChoose={setGroup}
        />
      ) : null}
      {readable && view === "api" ? (
        <div {...tabPanelProps("space-data-view", view)}>
          <TypeApi
            project={project}
            space={space}
            type={type}
            endpoints={endpoints}
            q={andQ(asked.q, gridQuery?.q) ?? q}
          />
          <SharePanel key={type} project={project} space={space} type={type} attributes={slots.map((slot) => slot.name)} />
        </div>
      ) : null}
      {/* The grid is a view like the others: its tab names this panel (axe aria-valid-attr-value, T-3851). */}
      {readable && config && view === "grid" ? (
        <div {...tabPanelProps("space-data-view", view)}>
        <PortalEntityGrid
          // A new key after an import, or a view applied, reads the type again: nothing typed under
          // the last one carries over.
          key={`${space}-${type}-${saved?.id ?? ""}-${imported}`}
          detailTabs={(row) => [
            ...(writer && writtenSlots.length > 0
              ? [{ id: "values", label: t("attributeHistory.tab"), content: <AttributeHistory slug={writer.slug} entityId={row.id} attributes={writtenSlots} /> }]
              : []),
            { id: "comments", label: t("spaces.comments.title"), content: <CommentsPanel project={project} space={space} urn={row.id} /> },
          ]}
          openRow={openRow}
          onOpenRow={setOpenRow}
          detailPlace={detailPlace}
          onDetailPlace={onDetailPlace}
          project={project}
          config={config}
          source={gridSource}
          // A page at a time, drawn a window at a time (T-3097); the pager picks the page's size.
          virtual
          paged
          pageSizes={PAGE_SIZES}
          onPageSize={(size) => setPageRows(String(size))}
          enums={enums}
          relations={relations}
          rules={rules}
          // Without a saved view the grid keeps the person's own remembered order.
          view={saved || steered ? gridView : undefined}
          thenBy={thenBy}
          groupBy={groupBy}
          groupHeader={(each) => <GroupCount scope={`${project}/${space}`} source={source} type={type} q={andQ(asked.q, gridQuery?.q)} path={each.path} />}
          onGridState={onGridState}
          onQuery={setAsked}
          onRows={onRows}
          hidden={extras.hidden}
          onHide={onHide}
          layout={layout}
          onLayout={onLayout}
          rowTone={rowTone}
          reloadKey={reloadKey}
          query={gridQuery}
         
          queryBox={showQuery}
          filterRow={!phone}
          onFilterColumns={onFilterColumns}
          rowNumbers={rowNumberLabels}
          typeChoice={{
            value: type,
            label: t("spaces.inside.type"),
            options: types.map((each) => ({ value: each, label: titleOf(each) })),
            onChange: pickType,
          }}
          selected={selectedRows}
          onSelectedChange={setSelectedRows}
          rowSize={rowSize}
          footer={(column) =>
            column.attr && !column.meta ? (
              <FooterAggregate
                scope={`${project}/${space}`}
                source={source}
                type={type}
                attr={column.attr}
                label={column.label}
                q={andQ(asked.q, gridQuery?.q)}
                kind={filterColumns.find((each) => each.attr === column.attr && !each.meta)?.kind}
                value={aggregations[column.attr] ?? "none"}
                onChange={(next) => setAggregations((before) => ({ ...before, [column.attr!]: next }))}
              />
            ) : null
          }
          empty={
            // An empty answer to a search or a filter is no match, not an empty space (T-5600).
            search.trim() !== "" ? (
              <NoMatch
                text={t("spaces.inside.searchNoMatch", { type: typeTitle, text: search.trim() })}
                clear={t("spaces.inside.clearSearch")}
                onClear={() => {
                  setSearchText("");
                  setSearch("");
                }}
              />
            ) : panelQ ? (
              <NoMatch
                text={t("spaces.inside.filterNoMatch", { type: typeTitle })}
                clear={t("spaces.inside.clearFilters")}
                onClear={() => setFilters(EMPTY_GROUP)}
              />
            ) : (
              <p className="text-body text-fg-muted">{t("spaces.inside.dataEmpty")}</p>
            )
          }
        />
        </div>
      ) : null}
      </div>
    </div>
  );
}

/** The fields a saved view groups by (`settings.groupBy`), `null` where it names none. */
/** What a search or filter that matched no row says, and the way back to every row. */
function NoMatch({ text, clear, onClear }: { text: string; clear: string; onClear: () => void }): JSX.Element {
  return (
    <div role="status" className="flex flex-wrap items-center gap-2">
      <p className="text-body text-fg-muted">{text}</p>
      <Button variant="secondary" size="sm" onClick={onClear}>
        {clear}
      </Button>
    </div>
  );
}

function readGroupBy(raw: unknown): string[] | null {
  if (!Array.isArray(raw)) return null;
  return raw.filter((each): each is string => typeof each === "string").slice(0, MAX_GROUP_FIELDS);
}

/** The footer's aggregations a saved view holds, only the ones the toolbar knows. */
function readAggregations(raw: unknown): Record<string, Aggregation> {
  if (typeof raw !== "object" || raw === null) return {};
  return Object.fromEntries(
    Object.entries(raw).filter((entry): entry is [string, Aggregation] => (AGGREGATIONS as readonly unknown[]).includes(entry[1])),
  );
}

/**
 * What a person does with the rows they checked (T-4883): delete them, each a write through the
 * space surface with their session, so the Policy decides every one; each deleted row keeps the
 * person's copy in the trash first (T-3107). A refused row stays, named with the reason.
 */
export function BulkBar({
  project,
  space,
  source,
  count,
  rows,
  canDelete,
  writable,
  writeSource,
  onChanged,
  exportLinks,
  tooManyToExport,
  onClear,
  onDeleted,
}: {
  project: string;
  space: string;
  source: ReturnType<typeof sourceFor>;
  /** How many rows are checked; `rows` holds those of them the grid has loaded. */
  count: number;
  rows: RichRow[];
  canDelete: boolean;
  /** The fields a grant lets the person write: the only ones "Set a field" offers (T-5186). */
  writable: FilterSlot[];
  /** Where the values are written: the granting Endpoint, whose Policy judges each row again. */
  writeSource: ReturnType<typeof sourceFor>;
  onChanged: () => void;
  exportLinks: ReactNode;
  tooManyToExport: boolean;
  onClear: () => void;
  onDeleted: (kept: string[]) => void;
}): JSX.Element {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [setting, setSetting] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const [running, setRunning] = useState(false);
  const [refused, setRefused] = useState<{ name: string; reason: string }[]>([]);
  const remove = async () => {
    setRunning(true);
    const failed: { id: string; name: string; reason: string }[] = [];
    for (const row of rows) {
      try {
        await deleteRow(project, space, source, row);
      } catch (error) {
        failed.push({ id: row.id, name: cellText(row.cells.name) || row.id, reason: error instanceof Error ? error.message : String(error) });
      }
    }
    setRunning(false);
    setConfirming(false);
    setRefused(failed);
    await queryClient.invalidateQueries({ queryKey: trashKey(project, space) });
    if (failed.length < rows.length) onDeleted(failed.map((each) => each.id));
  };
  return (
    <div className="flex flex-col gap-2">
      <div className="flex flex-wrap items-center gap-2 rounded-md bg-primary-soft px-3 py-2" role="region" aria-label={t("spaces.toolbar.selection")}>
        <span className="text-body font-medium">{t("spaces.toolbar.selected", { n: count })}</span>
        <Button
          size="sm"
          variant="danger"
          disabled={!canDelete || rows.length === 0}
          disabledReason={t("spaces.toolbar.deleteNotGranted")}
          onClick={() => setConfirming(true)}
        >
          {t("spaces.toolbar.deleteSelected")}
        </Button>
        <Button
          size="sm"
          variant="secondary"
          disabled={writable.length === 0 || rows.length === 0}
          disabledReason={t("spaces.toolbar.setNotGranted")}
          onClick={() => setSetting(true)}
        >
          {t("spaces.toolbar.setField")}
        </Button>
        {tooManyToExport ? <span className="text-caption text-fg-muted">{t("spaces.toolbar.exportTooMany", { n: MAX_EXPORT_IDS })}</span> : exportLinks}
        {/* Delete and Set act on loaded rows only: said, so no checked row is skipped unseen (F-UI3-014). */}
        {rows.length < count ? (
          <span className="basis-full text-caption text-fg-muted">{t("spaces.toolbar.selectedNotLoaded", { loaded: rows.length, n: count })}</span>
        ) : null}
        <Button size="sm" variant="ghost" onClick={onClear}>
          {t("spaces.toolbar.clearSelection")}
        </Button>
      </div>
      {refused.length > 0 ? (
        <Alert tone="danger" title={t("spaces.toolbar.deleteRefused", { n: refused.length })}>
          <ul className="list-disc pl-5">
            {refused.map((each) => (
              <li key={each.name}>{`${each.name}: ${each.reason}`}</li>
            ))}
          </ul>
        </Alert>
      ) : null}
      <ConfirmDialog
        open={confirming}
        onOpenChange={setConfirming}
        title={t("spaces.toolbar.deleteTitle", { n: rows.length })}
        description={t("spaces.toolbar.deleteBody")}
        confirmLabel={t("spaces.toolbar.deleteSelected")}
        tone="danger"
        pending={running}
        onConfirm={() => void remove()}
      />
      <BulkSetDialog open={setting} onOpenChange={setSetting} rows={rows} slots={writable} source={writeSource} onDone={onChanged} />
    </div>
  );
}

/** How long a row a live change touched stays marked. */
const FLASH_MS = 4000;
const NO_ROWS: ReadonlySet<string> = new Set();

/** What each shown row holds, by id, to tell after a read which rows changed. */
export function rowSignatures(rows: readonly RichRow[]): Map<string, string> {
  return new Map(rows.map((row) => [row.id, JSON.stringify(row.cells)]));
}

/** The rows that are new or hold something else than before. */
export function changedRows(before: ReadonlyMap<string, string>, rows: readonly RichRow[]): ReadonlySet<string> {
  return new Set(rows.filter((row) => before.get(row.id) !== JSON.stringify(row.cells)).map((row) => row.id));
}

/**
 * The value typed for "Set a field", as the field's range takes it, or undefined while it is none:
 * an enum's own value, a finite number, true or false, or text that is not empty.
 */
export function bulkValue(slot: FilterSlot, text: string): string | number | boolean | undefined {
  const typed = text.trim();
  if (typed === "") return undefined;
  if (slot.values) return slot.values.includes(typed) ? typed : undefined;
  if (["integer", "float", "double", "decimal"].includes(slot.range ?? "")) {
    const n = Number(typed);
    return Number.isFinite(n) && (slot.range !== "integer" || Number.isInteger(n)) ? n : undefined;
  }
  if (slot.range === "boolean") return typed === "true" ? true : typed === "false" ? false : undefined;
  return typed;
}

/**
 * One value written into a field of every checked row (T-5186): a PATCH per row through the
 * granting Endpoint with the person's session and their reason, so its Policy judges each row and
 * each value; a refused row is named with the Endpoint's own sentence.
 */
export function BulkSetDialog({
  open,
  onOpenChange,
  rows,
  slots,
  source,
  onDone,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  rows: RichRow[];
  slots: FilterSlot[];
  source: Pick<ReturnType<typeof sourceFor>, "patch">;
  onDone: () => void;
}): JSX.Element {
  const { t } = useTranslation();
  const id = useId();
  const [attr, setAttr] = useState("");
  const [text, setText] = useState("");
  const [reason, setReason] = useState("");
  const [running, setRunning] = useState(false);
  const [refused, setRefused] = useState<{ name: string; reason: string }[] | null>(null);
  const slot = slots.find((each) => each.name === attr);
  const value = slot ? bulkValue(slot, text) : undefined;
  const nameOf = (rowId: string) => cellText(rows.find((row) => row.id === rowId)?.cells.name) || rowId;
  const apply = async () => {
    if (!slot || value === undefined) return;
    setRunning(true);
    const failed: { name: string; reason: string }[] = [];
    for (let at = 0; at < rows.length; at += MAX_ENTITIES) {
      const entities = rows.slice(at, at + MAX_ENTITIES).map((row) => ({ id: row.id, changes: [{ attribute: slot.name, after: value }] }));
      try {
        const result = await applyChanges({ source, entities, reason, fallback: t("spaces.toolbar.setRefusedRow") });
        failed.push(...result.refused.map((each) => ({ name: nameOf(each.id), reason: each.detail })));
      } catch (error) {
        failed.push(...entities.map((each) => ({ name: nameOf(each.id), reason: error instanceof Error ? error.message : String(error) })));
      }
    }
    setRunning(false);
    setRefused(failed);
    if (failed.length < rows.length) onDone();
  };
  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (next) setRefused(null);
        onOpenChange(next);
      }}
      title={t("spaces.toolbar.setTitle", { n: rows.length })}
      closeLabel={t("app.close")}
    >
      <div className="flex flex-col gap-3">
        <p className="text-body text-fg-muted">{t("spaces.toolbar.setLead")}</p>
        <Field id={`${id}-attr`} label={t("spaces.toolbar.setFieldLabel")} help={t("spaces.toolbar.setFieldHint")}>
          <Select
            id={`${id}-attr`}
            value={attr}
            onChange={(e) => {
              setAttr(e.target.value);
              setText("");
            }}
          >
            <option value="">{t("spaces.toolbar.setFieldPick")}</option>
            {slots.map((each) => (
              <option key={each.name} value={each.name}>
                {each.name}
              </option>
            ))}
          </Select>
        </Field>
        {slot ? (
          <Field
            id={`${id}-value`}
            label={t("spaces.toolbar.setValue")}
            help={slot.values ? t("spaces.toolbar.setValueEnumHint") : t("spaces.toolbar.setValueHint")}
            errors={text.trim() !== "" && value === undefined ? [t("spaces.toolbar.setValueInvalid", { range: slot.range ?? "" })] : undefined}
          >
            {slot.values ? (
              <Select id={`${id}-value`} value={text} onChange={(e) => setText(e.target.value)}>
                <option value="">{t("spaces.toolbar.setValuePick")}</option>
                {slot.values.map((option) => (
                  <option key={option} value={option}>
                    {option}
                  </option>
                ))}
              </Select>
            ) : (
              <Input id={`${id}-value`} value={text} onChange={(e) => setText(e.target.value)} />
            )}
          </Field>
        ) : null}
        <Field id={`${id}-reason`} label={t("spaces.toolbar.setReason")} help={t("spaces.toolbar.setReasonHint")}>
          <Input id={`${id}-reason`} value={reason} maxLength={500} onChange={(e) => setReason(e.target.value)} />
        </Field>
        {refused && refused.length > 0 ? (
          <Alert tone="danger" title={t("spaces.toolbar.setRefused", { n: refused.length })}>
            <ul className="list-disc pl-5">
              {refused.map((each) => (
                <li key={each.name}>{`${each.name}: ${each.reason}`}</li>
              ))}
            </ul>
          </Alert>
        ) : null}
        {refused && refused.length === 0 ? (
          <Alert tone="success" role="status" title={t("spaces.toolbar.setDone", { n: rows.length })} />
        ) : null}
        <Button
          className="w-fit"
          disabled={value === undefined || reason.trim() === "" || running}
          disabledReason={running ? t("app.loading") : t("spaces.toolbar.setIncomplete")}
          onClick={() => void apply()}
        >
          {t("spaces.toolbar.setApply", { n: rows.length })}
        </Button>
      </div>
    </Dialog>
  );
}

/**
 * The space's entity types, each a button that opens it, and under the open one its saved views
 * (API/01 §30): the left list of the data workspace (T-4825). The views come from the same query
 * the view bar reads, so a view saved there appears here at once.
 */
const PAGE_SIZES = [100, 500, 1000, 5000] as const;

function TypeList({
  project,
  space,
  types,
  type,
  titleOf,
  selected,
  onType,
  onView,
}: {
  project: string;
  space: string;
  types: string[];
  type: string;
  titleOf: (type: string) => string;
  selected: SavedView | null;
  onType: (type: string) => void;
  onView: (view: SavedView | null) => void;
}): JSX.Element {
  const { t } = useTranslation();
  const views = useQuery({
    queryKey: dataViewsKey(project, space),
    retry: false,
    queryFn: () => listViews(project, space),
  });
  const ofType = (views.data ?? []).filter((view) => view.type === type);
  const item = (current: boolean) =>
    clsx("w-full justify-start truncate text-left text-body", current && "bg-primary-soft font-medium text-primary-soft-fg");
  return (
    <>
    {/* Below lg the list of types is a picker above the grid (T-4992): twenty types listed one
        under the other put the first row two screens down on a phone. The saved views have
        their own picker beside the grid. */}
    <Field id="space-type-picker" label={t("spaces.inside.type")} description={t("spaces.inside.typeHint")} className="lg:hidden">
      <Select id="space-type-picker" value={type} onChange={(event) => onType(event.target.value)}>
        {types.map((each) => (
          <option key={each} value={each}>
            {titleOf(each)}
          </option>
        ))}
      </Select>
    </Field>
    <nav aria-label={t("spaces.inside.types")} className="max-lg:hidden lg:border-r lg:border-border lg:pr-3">
      <ul className="flex flex-col gap-0.5">
        {types.map((each) => (
          <li key={each}>
            <Button
              variant="ghost"
              size="sm"
              className={item(each === type)}
              aria-current={each === type ? "true" : undefined}
              title={each}
              onClick={() => onType(each)}
            >
              {titleOf(each)}
            </Button>
            {/* Without saved views the type itself is all its rows: a lone "All rows" under it
                only repeated it (T-6084). */}
            {each === type && ofType.length > 0 ? (
              <ul aria-label={t("spaces.inside.savedViews")} className="ml-3 mt-0.5 flex flex-col gap-0.5 border-l border-border pl-2">
                <li>
                  <Button
                    variant="ghost"
                    size="sm"
                    className={item(selected === null)}
                    aria-current={selected === null ? "true" : undefined}
                    onClick={() => onView(null)}
                  >
                    {t("spaces.inside.allRows")}
                  </Button>
                </li>
                {ofType.map((view) => (
                  <li key={view.id}>
                    <Button
                      variant="ghost"
                      size="sm"
                      className={item(selected?.id === view.id)}
                      aria-current={selected?.id === view.id ? "true" : undefined}
                      onClick={() => onView(view)}
                    >
                      {view.title}
                    </Button>
                  </li>
                ))}
              </ul>
            ) : null}
          </li>
        ))}
      </ul>
    </nav>
    </>
  );
}

/**
 * The views of a space's entities (ADR-N-042 §3.2): the grid and the API (owner 2026-10-31, T-6210:
 * one grid for every page; the gallery, board, calendar, timeline, map and form views are gone).
 */
const DATA_VIEWS = ["grid", "api"] as const;
type DataView = (typeof DATA_VIEWS)[number];

/**
 * What one endpoint answers, folded into one line (T-2280, UI-26, EP-51).
 *
 * Every representation on its own line made two endpoints fill the screen, and a column that tall
 * cannot be read down — which is the only reason to put audience and state in a table at all. The
 * summary names the first two and counts the rest; opening it shows each representation where it can
 * be clicked, and the documents that belong to the endpoint beside them. `<details>` rather than a
 * menu of our own, because the browser already gives it a keyboard, a role and a state a screen
 * reader announces.
 */
function Representations({
  slug,
  representations,
}: {
  slug: string;
  representations: string[];
}): JSX.Element {
  const { t } = useTranslation();
  const { domain } = useBranding();
  const named = representations.slice(0, 2);
  const rest = representations.length - named.length;
  const link = (rep: string) =>
    slug && REPRESENTATION_PATHS[rep] ? (
      <EndpointLink href={representationUrl(slug, rep, domain)}>{rep}</EndpointLink>
    ) : (
      <Badge mono>{rep}</Badge>
    );

  if (representations.length === 0 && !slug) {
    return <span className="text-body text-fg-subtle">{t("endpoints.field.noRepresentations")}</span>;
  }

  return (
    <details className="group">
      <summary className="focus-ring cursor-pointer list-none text-body">
        <span className="font-mono">{named.join(", ")}</span>
        {rest > 0 ? (
          <span className="ml-1 text-fg-muted">{t("endpoints.field.more", { count: rest })}</span>
        ) : null}
      </summary>
      <ul className="mt-1 flex flex-wrap gap-1">
        {representations.map((rep) => (
          <li key={rep}>{link(rep)}</li>
        ))}
      </ul>
      {slug ? (
        <ul className="mt-1 flex flex-wrap gap-1">
          {ENDPOINT_LINKS.map((entry) => (
            <li key={entry.key}>
              <EndpointLink href={endpointUrl(slug, entry.path)}>
                {t(`endpoints.link.${entry.key}`)}
              </EndpointLink>
            </li>
          ))}
        </ul>
      ) : null}
    </details>
  );
}

/**
 * What a section shows in place of its table: the wait, the failure, or the words for empty.
 *
 * The three sections below each wrote `query.isPending ? loading : "there are none"`, so a
 * list that came back 403 or never came back at all said "this space has no endpoints" — the
 * most reassuring possible rendering of a failed read (UI-15, T-1854).
 */
function SectionState({
  query,
  empty,
}: {
  query: Pick<UseQueryResult, "isPending" | "isError" | "error" | "refetch">;
  empty: ReactNode;
}): JSX.Element {
  const { t } = useTranslation();
  if (query.isPending) {
    return (
      <p role="status" className="text-body text-fg-muted">
        {t("app.loading")}
      </p>
    );
  }
  if (query.isError) {
    return (
      <PageFailed
        error={query.error}
        onRetry={() => {
          void query.refetch();
        }}
      />
    );
  }
  return <p className="text-body text-fg-muted">{empty}</p>;
}

function Section({ title, children }: { title: ReactNode; children: ReactNode }): JSX.Element {
  return (
    <section className="space-y-2">
      <h2 className="text-title font-semibold text-fg">{title}</h2>
      {children}
    </section>
  );
}

/**
 * The space's data model, one click from the space (T-2762; DM-61, DM-62): its title, version and
 * classes, and one link to the model's own page, where it is read and edited (T-6090: the tab
 * embedded the whole modeler, a second home for one model). A space without one offers Create
 * and Import, prefilled for it.
 */
function SpaceModel({
  project,
  space,
  model,
  models,
}: {
  project: string;
  space: string;
  model: Manifest | undefined;
  models: Pick<UseQueryResult, "isPending" | "isError" | "error" | "refetch">;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const source = useSourceOf(project, model);
  if (model === undefined) {
    if (models.isPending || models.isError) {
      return <SectionState query={models} empty={null} />;
    }
    return (
      <EmptyState
        icon="models"
        title={t("spaces.inside.modelEmpty")}
        description={t("spaces.inside.modelEmptyLead")}
        action={
          <div className="flex flex-wrap gap-2">
            <ProposeLink project={project} search={{ new: "blank", space }} variant="primary">
              {t("spaces.inside.modelCreate")}
            </ProposeLink>
            <ProposeLink project={project} search={{ new: "sdm", space }}>
              {t("spaces.inside.modelImport")}
            </ProposeLink>
          </div>
        }
      />
    );
  }
  const name = model.metadata.name;
  const title = localized(model.metadata.title, i18n.language, name);
  const version = typeof model.spec.version === "string" ? model.spec.version : undefined;
  return (
    <Card className="flex flex-col gap-3">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <p className="flex flex-wrap items-center gap-2 text-body">
          <span className="font-medium text-fg">{title}</span>
          {title !== name ? <span className="font-mono text-caption text-fg-muted">{name}</span> : null}
          {version ? <span className="font-mono text-caption text-fg-muted">v{version}</span> : null}
        </p>
        <Link to="/projects/$project/models/$name" params={{ project, name }} className={buttonClass("primary", "sm")}>
          {t("spaces.inside.modelOpen")}
        </Link>
      </div>
      {source.isError ? (
        <Alert tone="danger" role="alert">
          <p>{t("models.page.sourceFailed", { reason: source.error instanceof Error ? source.error.message : "" })}</p>
          <Button size="sm" className="mt-2" onClick={() => void source.refetch()}>
            {t("app.error.retry")}
          </Button>
        </Alert>
      ) : source.data === undefined ? (
        <p role="status" className="text-body text-fg-muted">
          {t("models.source.loading")}
        </p>
      ) : (
        <ModelClassSummary project={project} name={name} source={source.data} />
      )}
    </Card>
  );
}

/** How many classes the model has, and the first few as links into the model's own page. */
const SUMMARY_CLASSES = 8;

/**
 * The class names of a LinkML source, `null` when it does not parse. `linkml:Any` is a value
 * helper, not a class a space holds; the diagram leaves it out too.
 */
export function classNamesOf(source: string | undefined): string[] | null {
  if (source === undefined) return null;
  try {
    return parseModel(source).classes.filter((klass) => !isAnyClass(klass)).map((klass) => klass.name);
  } catch {
    return null;
  }
}

function ModelClassSummary({ project, name, source }: { project: string; name: string; source: string }): JSX.Element {
  const { t } = useTranslation();
  const classes = useMemo(() => classNamesOf(source), [source]);
  if (classes === null) {
    return <p className="text-body text-fg-muted">{t("spaces.inside.modelUnreadable")}</p>;
  }
  const shown = classes.slice(0, SUMMARY_CLASSES);
  return (
    <div className="flex flex-col gap-2">
      <p className="text-body text-fg">{t("spaces.inside.modelClasses", { count: classes.length })}</p>
      {shown.length > 0 ? (
        <ul aria-label={t("spaces.inside.modelClassList")} className="flex flex-wrap gap-1.5">
          {shown.map((klass) => (
            <li key={klass}>
              <Link
                to="/projects/$project/models/$name"
                params={{ project, name }}
                search={{ class: klass }}
                className="focus-ring inline-flex rounded-sm border border-border bg-bg px-2 py-0.5 font-mono text-caption text-primary-soft-fg hover:underline"
              >
                {klass}
              </Link>
            </li>
          ))}
          {classes.length > shown.length ? (
            <li className="px-1 text-caption text-fg-muted">
              {t("spaces.inside.modelMore", { count: classes.length - shown.length })}
            </li>
          ) : null}
        </ul>
      ) : null}
    </div>
  );
}

/** What a Context Space holds: its entity types with live counts, its endpoints and policies. */
export function SpaceInside({ project, name }: { project: string; name: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const identity = useIdentity();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "sk";
  // The space's alerts are being chosen (T-3261).
  const [alerting, setAlerting] = useState(false);
  const { can: mayPropose, checking: permissionsChecking } = usePermissions(project);
  // The page opens on the rows; the model, quality, retention, endpoints and policies are tabs
  // beside them, kept in the address (T-4825: the whole model came before the grid, 14,515 px).
  const [tab, setTab] = useUrlParam<SpaceTab>("tab", "data", SPACE_TABS);
  // What the whole space holds, the broker's count (T-2889), asked only on the model tab beside
  // the types counted through the person's own endpoint. The data tab counts only through the
  // grid's own answer (T-4885): this whole-tenant scan took longer than the page waits on
  // helsinki and put "the count did not come" under a grid that had its count.
  const usage = useQuery({ ...spaceUsageQuery(project, name), enabled: tab === "model" });

  const space = useQuery(spaceQuery(project, name));
  const models = useProjectList(project, "datamodels");
  const endpoints = useProjectList(project, "endpoints");
  const policies = useProjectList(project, "policies");
  const catalogueLink = useCatalogueLinks([project]);

  // Every DataModel that names this space, not only the one the space names back. A model's
  // `contextSpaceRef` is required and a space's `dataModelRef` is an optional pointer at the
  // primary one (kinds: `DataModelSpec`, `SpaceSpec`), and no seeded space sets it — so reading
  // the pointer alone left this table empty for every space on dev, Helsinki's included, while
  // the space held hundreds of entities (T-2450). The pointer still decides which model is
  // first, and so which type the grid below opens on.
  const named = space.data ? modelsOfSpace(space.data as Manifest, asManifests(models.data?.items ?? [])) : [];
  const model = named[0];
  // `spec.classes` is optional: a model whose manifest lists none still defines types, its source's
  // classes, the ones the Data model tab counts (T-6090 review: "3 classes" above "no entity type").
  const listed = [...new Set(named.flatMap(entityTypesOf))];
  const modelSource = useSourceOf(project, listed.length === 0 ? model : undefined);
  const sourced = useMemo(() => classNamesOf(modelSource.data) ?? [], [modelSource.data]);
  const types = listed.length > 0 ? listed : sourced;

  if (space.isPending) {
    return <PageLoading label={t("app.loading")} />;
  }
  if (space.isError) {
    return (
      <ResourcePageFailed
        title={name}
        description={t("spaces.lead")}
        error={space.error}
        onRetry={() => {
          void space.refetch();
        }}
        back={
          <Link
            to="/projects/$project/$plural"
            params={{ project, plural: "spaces" }}
            className="focus-ring touch-target text-body text-primary-soft-fg underline hover:no-underline"
          >
            {t("spaces.inside.back")}
          </Link>
        }
      />
    );
  }

  const manifest = space.data as Manifest;
  const spaceEndpoints = asManifests(endpoints.data?.items ?? []).filter(
    (endpoint) => spaceOf(endpoint) === name,
  );
  const readEndpoint = pickReadEndpoint(spaceEndpoints, identity ? (identity.groups ?? []) : null, project);
  const slug =
    typeof readEndpoint?.spec.slug === "string" ? (readEndpoint.spec.slug as string) : undefined;
  const spacePolicies = asManifests(policies.data?.items ?? []).filter(
    (policy) => spaceOf(policy) === name,
  );

  return (
    <div className="space-y-4">
      {/* One row (owner 2026-10-31): the name, what the space is for behind the info mark, the state,
          the actions at the right; the shell's crumb is the way back. */}
      <PageHeader
        compact
        title={localized(manifest.metadata.title, locale, manifest.metadata.name)}
        titleAction={<LifecycleBadge kind="phase" value={manifest.status?.phase} />}
        description={localized(manifest.metadata.description, locale, "") || t("spaces.inside.lead")}
        actions={
          <>
            {/* Completing works on one space, so it lives here and not on the list (T-6084). */}
            {/* Allowed: a Link (opens in a new tab too); refused or checking: a Button the guard
                disables with its reason. A guarded Link stayed clickable and leaked `disabledReason`
                onto the <a>. */}
            {!permissionsChecking && mayPropose("ContextSpace", "propose") ? (
              <Link
                to="/projects/$project/spaces/complete"
                params={{ project }}
                search={{ space: manifest.metadata.name }}
                className={buttonClass("secondary", "sm")}
              >
                {t("spaces.complete.title")}
              </Link>
            ) : (
              <PermissionGuard project={project} kind="ContextSpace" verb="propose">
                <Button variant="secondary" size="sm">
                  {t("spaces.complete.title")}
                </Button>
              </PermissionGuard>
            )}
            <Button variant="secondary" size="sm" onClick={() => setAlerting(true)}>
              {t("alerts.forSpace")}
            </Button>
          </>
        }
      />
      {alerting ? (
        <AlertDialog
          project={project}
          scope="space"
          target={manifest.metadata.name}
          label={localized(manifest.metadata.title, locale, manifest.metadata.name)}
          onClose={() => setAlerting(false)}
        />
      ) : null}

      <Tabs
        id="space-inside"
        label={t("spaces.inside.sections")}
        tabs={SPACE_TABS.map((value) => ({ value, label: t(TAB_LABELS[value]) }))}
        value={tab}
        onChange={setTab}
      />

      {tab === "data" ? (
        <div {...tabPanelProps("space-inside", tab)}>
          <SpaceData
            project={project}
            space={name}
            types={types}
            endpoints={spaceEndpoints}
            model={model}
            retention={rulesOf(manifest)}
          />
        </div>
      ) : null}

      {tab === "model" ? (
        <div {...tabPanelProps("space-inside", tab)} className="space-y-6">
      <Section title={t("spaces.inside.model")}>
        <SpaceModel project={project} space={name} model={model} models={models} />
      </Section>

      <Section title={<Term name="entityType">{t("spaces.inside.types")}</Term>}>
        {model === undefined ? (
          <SectionState query={models} empty={t("spaces.inside.noModel")} />
        ) : types.length === 0 ? (
          <p className="text-body text-fg-muted">
            {t("spaces.inside.noTypes", { model: model.metadata.name })}
          </p>
        ) : (
          <>
            <p className="text-body text-fg-muted">
              {t("spaces.field.dataModel")}:{" "}
              <Link
                to="/projects/$project/models/$name"
                params={{ project, name: model.metadata.name }}
                className="focus-ring font-mono text-primary-soft-fg underline-offset-2 hover:underline"
              >
                {model.metadata.name}
              </Link>
              {readEndpoint ? (
                <>
                  {" · "}
                  {t("spaces.inside.readThrough", { endpoint: readEndpoint.metadata.name })}
                </>
              ) : (
                <>
                  {" · "}
                  {endpoints.isPending
                    ? t("app.loading")
                    : endpoints.isError
                      ? t("app.error.generic")
                      : t("spaces.inside.noEndpoint")}
                </>
              )}
            </p>
            <Table caption={t("spaces.inside.types")}>
              <TableHead>
                <TableHeaderCell>{t("spaces.inside.type")}</TableHeaderCell>
                <TableHeaderCell align="right">{t("spaces.inside.count")}</TableHeaderCell>
                <TableHeaderCell align="right">{t("spaces.inside.size")}</TableHeaderCell>
              </TableHead>
              <TableBody>
                {types.map((type) => (
                  <TypeRow
                    key={type}
                    project={project}
                    space={name}
                    slug={slug}
                    type={type}
                    bytes={bytesOfType(usage.data?.bytes?.byType, type)}
                  />
                ))}
              </TableBody>
            </Table>
            {/* what a size counts, said only beside sizes (T-6156) */}
            {Object.keys(usage.data?.bytes?.byType ?? {}).length > 0 ? (
              <p className="text-caption text-fg-muted">{t("spaces.inside.sizeHint")}</p>
            ) : null}
            <SpaceTotal usage={usage} />
          </>
        )}
      </Section>

        </div>
      ) : null}

      {tab === "quality" ? (
        <div {...tabPanelProps("space-inside", tab)} className="space-y-6">
          <div className="space-y-4">
            <SpaceQuality project={project} space={name} />
            <SpaceChecks project={project} space={name} />
          </div>
          <Section title={t("drift.section.title")}>
            <SpaceDrift project={project} space={name} />
          </Section>
        </div>
      ) : null}

      {tab === "retention" ? (
        <div {...tabPanelProps("space-inside", tab)}>
          <SpaceRetention project={project} space={name} manifest={manifest} types={types} />
          <div className="mt-6">
            <Section title={t("spaces.temporal.title")}>
              <SpaceTemporalAudit project={project} space={name} />
            </Section>
          </div>
          <div className="mt-6">
            <Section title={t("spaces.personal.title")}>
              <SpacePersonalData project={project} space={name} />
            </Section>
          </div>
        </div>
      ) : null}

      {tab === "endpoints" ? (
      <div {...tabPanelProps("space-inside", tab)}>
      <Section title={<Term name="endpoint">{t("endpoints.title")}</Term>}>
        {spaceEndpoints.length === 0 ? (
          <SectionState query={endpoints} empty={t("spaces.inside.noEndpoints")} />
        ) : (
          <Table caption={t("endpoints.title")}>
            <TableHead>
              <TableHeaderCell>{t("endpoints.field.name")}</TableHeaderCell>
              <TableHeaderCell>{t("endpoints.field.audience")}</TableHeaderCell>
              <TableHeaderCell>{t("endpoints.field.representations")}</TableHeaderCell>
              <TableHeaderCell>{t("spaces.inside.catalogue")}</TableHeaderCell>
            </TableHead>
            <TableBody>
              {spaceEndpoints.map((endpoint) => {
                const spec = endpoint.spec as {
                  slug?: string;
                  audience?: string;
                  enabledRepresentations?: string[];
                };
                const endpointSlug = spec.slug ?? "";
                const policyRefs = policyRefsOf(endpoint);
                const catalogue = catalogueLink(project, endpoint);
                return (
                  <TableRow key={endpoint.metadata.name}>
                    <TableCell>
                      <Link
                        to="/projects/$project/$plural/$name"
                        params={{ project, plural: "endpoints", name: endpoint.metadata.name }}
                        className="focus-ring font-medium text-primary-soft-fg underline-offset-2 hover:underline"
                      >
                        {localized(endpoint.metadata.title, locale, endpoint.metadata.name)}
                      </Link>
                      <div className="font-mono text-caption text-fg-subtle">
                        {endpoint.metadata.title ? endpoint.metadata.name : null}
                        {policyRefs
                          ? `${endpoint.metadata.title ? " · " : ""}${policyRefs.map(refName).join(", ")}`
                          : ""}
                      </div>
                    </TableCell>
                    <TableCell>
                      <SharedWithBadge endpoint={endpoint} />
                    </TableCell>
                    <TableCell>
                      <Representations
                        slug={endpointSlug}
                        representations={servedRepresentations(spec)}
                      />
                    </TableCell>
                    <TableCell>
                      {catalogue ? (
                        <EndpointLink href={catalogue}>{t("spaces.inside.catalogueLink")}</EndpointLink>
                      ) : (
                        <span className="text-fg-subtle">{t("spaces.inside.notPublished")}</span>
                      )}
                    </TableCell>
                  </TableRow>
                );
              })}
            </TableBody>
          </Table>
        )}
      </Section>
      </div>
      ) : null}

      {tab === "policies" ? (
      <div {...tabPanelProps("space-inside", tab)}>
      <Section title={<Term name="policy">{t("spaces.inside.policies")}</Term>}>
        {spacePolicies.length === 0 ? (
          <SectionState query={policies} empty={t("spaces.inside.noPolicies")} />
        ) : (
          <Table caption={t("spaces.inside.policies")} stack>
            <TableHead>
              <TableHeaderCell>{t("spaces.field.name")}</TableHeaderCell>
              <TableHeaderCell>{t("spaces.inside.assignee")}</TableHeaderCell>
              <TableHeaderCell>{t("spaces.inside.operations")}</TableHeaderCell>
              <TableHeaderCell>{t("spaces.inside.type")}</TableHeaderCell>
            </TableHead>
            <TableBody>
              {spacePolicies.map((policy) => {
                const spec = policy.spec as {
                  assignee?: { kind?: string; id?: string };
                  operations?: string[];
                  information?: Array<{ entities?: Array<{ type?: string }> }>;
                };
                const policyTypes = [
                  ...new Set(
                    (spec.information ?? [])
                      .flatMap((info) => info.entities ?? [])
                      .map((entity) => entity.type)
                      .filter((type): type is string => typeof type === "string"),
                  ),
                ];
                const kinds = operationKinds(spec.operations ?? []);
                return (
                  <TableRow key={policy.metadata.name}>
                    <TableCell>
                      <div className="font-medium">
                        {localized(policy.metadata.title, locale, policy.metadata.name)}
                      </div>
                      {policy.metadata.title ? (
                        <div className="font-mono text-caption text-fg-subtle">
                          {policy.metadata.name}
                        </div>
                      ) : null}
                    </TableCell>
                    <TableCell data-label={t("spaces.inside.assignee")}>
                      <span title={spec.assignee ? `${spec.assignee.kind ?? ""}:${spec.assignee.id ?? ""}` : undefined}>
                        {assigneeWords(spec.assignee, t)}
                      </span>
                    </TableCell>
                    <TableCell data-label={t("spaces.inside.operations")}>
                      {kinds.length === 0 ? (
                        "—"
                      ) : (
                        <span className="flex flex-wrap gap-1">
                          {kinds.map(({ kind, operations }) => (
                            <Badge
                              key={kind}
                              tone={kind === "write" || kind === "delete" ? "warning" : "neutral"}
                              title={operations.join(", ")}
                            >
                              {t(`spaces.inside.operationKind.${kind}`)}
                            </Badge>
                          ))}
                        </span>
                      )}
                    </TableCell>
                    <TableCell data-label={t("spaces.inside.type")}>
                      {policyTypes.length > 0 ? (
                        <span className="flex flex-col">
                          {policyTypes.map((type) => (
                            <span key={type}>{type}</span>
                          ))}
                        </span>
                      ) : (
                        <span className="text-fg-muted">{t("spaces.inside.everyType")}</span>
                      )}
                    </TableCell>
                  </TableRow>
                );
              })}
            </TableBody>
          </Table>
        )}
      </Section>
      </div>
      ) : null}

      {tab === "activity" ? (
        <div {...tabPanelProps("space-inside", tab)}>
          <ActivityFeed project={project} compact fixed={{ space: name }} limit={10} />
        </div>
      ) : null}
    </div>
  );
}

const SPACE_TABS = ["data", "model", "quality", "retention", "endpoints", "policies", "activity"] as const;
type SpaceTab = (typeof SPACE_TABS)[number];
const TAB_LABELS: Record<SpaceTab, string> = {
  data: "spaces.inside.data",
  model: "spaces.inside.model",
  quality: "spaces.quality.title",
  retention: "spaces.retention.title",
  endpoints: "endpoints.title",
  policies: "spaces.inside.policies",
  activity: "activity.panelTitle",
};

/**
 * What the whole space holds, the broker's count (T-2889), under the types table: counting, the
 * entity and history counts with their sizes and broker, or why there is none (T-4825). A history
 * too long to count is the broker's estimate, and says so (T-6085).
 */
function SpaceTotal({ usage }: { usage: UseQueryResult<SpaceUsage> }): JSX.Element {
  const { t, i18n } = useTranslation();
  if (usage.data !== undefined) {
    const { bytes, attrInstances, estimated } = usage.data;
    return (
      <footer className="flex flex-wrap gap-x-4 gap-y-1 border-t border-border pt-2 text-caption text-fg-muted">
        <span className="text-body text-fg" data-testid="space-total">
          {t("spaces.inside.total", { count: usage.data.entities })}
        </span>
        {attrInstances != null ? (
          <span className="text-body text-fg" data-testid="space-history">
            {t("spaces.inside.history", { count: attrInstances })}
          </span>
        ) : null}
        {bytes ? (
          <span data-testid="space-size" title={exactBytes(bytes.entities + bytes.attrInstances, i18n.language)}>
            {t("spaces.inside.totalSize", {
              size: sizeText(bytes.entities + bytes.attrInstances, i18n.language),
              entities: sizeText(bytes.entities, i18n.language),
              history: sizeText(bytes.attrInstances, i18n.language),
            })}
          </span>
        ) : null}
        {estimated ? <span data-testid="space-estimated">{t("spaces.inside.estimated")}</span> : null}
        <span data-testid="space-broker">{t("spaces.inside.broker", { broker: usage.data.broker })}</span>
      </footer>
    );
  }
  return (
    // a div: a footer may not carry role="status" (axe aria-allowed-role, T-6145)
    <div className="border-t border-border pt-2 text-body text-fg-muted" data-testid="space-total" role="status">
      {usage.isError ? t("spaces.entitiesUnknown", { reason: usageRefusal(usage.error) }) : t("spaces.inside.counting")}
    </div>
  );
}
