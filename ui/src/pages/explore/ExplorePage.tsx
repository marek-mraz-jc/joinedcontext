import { useCallback, useEffect, useMemo, useState } from "react";
import type { JSX } from "react";
import { useQueries, useQuery } from "@tanstack/react-query";
import { ListFailed } from "../../components/forms/widgets/ListFailed";
import { useTranslation } from "react-i18next";
import { parseGridConfig } from "@joinedcontext/sdk";
import type { ResolvedGridConfig, RichRow } from "@joinedcontext/sdk";
import { api, queryKeys, unwrap, reasonOf } from "../../api/client";
import { spaceUsageQuery } from "../../api/spaceUsage";
import { asManifests, localized, refName } from "../../api/manifest";
import type { Manifest } from "../../api/manifest";
import { AccessPanel, useAccess } from "../../components/entities/AccessPanel";
import { AttributeHistory } from "../../components/entities/history/AttributeHistory";
import { deniedOf, explainAttribute, useAccessCheck, useAttributeAccess, writableOf } from "../../api/access";
import { EntityFilters } from "../../components/entities/EntityFilters";
import { PortalEntityGrid } from "../../components/entities/PortalEntityGrid";
import { enumsOfModel, fetchEntity, filterSlotsOf, qProblem, relationsOfModel, rulesOfModel, useModelSource } from "../../components/entities/filters";
import type { EntityQuery } from "../../components/entities/filters";
import { Alert, Button, Card, EmptyState, PageHeader, Select, Tabs, tabPanelProps } from "../../components/ui";
import { ToolbarPopover } from "../../components/entities/GridToolbar";
import { servedRepresentations } from "../../components/endpoints/links";
import { ExportLinks } from "../spaces/ImportRows";
import { MAX_EXPORT_IDS } from "../spaces/importRows";
import { TypeQuality } from "./TypeQuality";
import { TypeLink } from "../models/ModelLinks";
import { ExploreRowActions } from "./ExploreRowActions";
import { UseThisData } from "./UseThisData";
import { ChartFromView } from "./ChartFromView";
import { ExploreMap, isLocated } from "./ExploreMap";
import { andQ } from "./exportView";
import { entityTypesOf, isAppEndpoint, pickReadEndpoint, spaceOf } from "../spaces/spaceFacts";
import { useIdentity } from "../../auth/AuthProvider";
import { replaceOwnSearch } from "../../assistant/HandOff";

const PAGE_SIZE = 50;

function useProjectList(project: string, plural: string) {
  return useQuery({
    queryKey: queryKeys.list(project, plural),
    // The key is shared with every other page that lists this kind, so the cache holds the
    // list as the API answers it, whichever page asked first; the manifests are read off it
    // here (T-0625).
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural } },
        }),
      ),
    select: (list) => asManifests(list.items ?? []),
  });
}

/**
 * The data explorer (UI-33, UI-64): a space, one of its endpoints, filters generated from the
 * space's DataModel, and the entities the endpoint returns for the signed-in user, shown in the
 * one entity grid of the product. The page owns what is a page's business — the space and the
 * endpoint, the type and its filters, the export of what is on screen, and the removal of one
 * entity (UI-60) — and the grid owns the table: the columns, the metadata a person opens, the
 * paging, the per-column filter row and the correction of a value where the grant allows one.
 */
/** How many of the project's spaces are counted for the one the page opens on. */
const COUNTED_SPACES = 5;

/**
 * The space the page opens on: the first, in the list's order, whose count says it holds
 * entities; the first space when none does or a count failed. Undefined while an earlier
 * space's count is still on its way, so the page never jumps from one space to another.
 */
export function firstWithData(
  names: string[],
  counts: { isPending: boolean; data?: { entities?: number } }[],
): string | undefined {
  for (const [at, name] of names.entries()) {
    const count = counts[at];
    if (count === undefined || count.isPending) return undefined;
    if ((count.data?.entities ?? 0) > 0) return name;
  }
  return names[0];
}

export function ExplorePage({
  project,
  initialSpace,
  initialEndpoint,
  initialEntityId,
  initialType,
  initialQ,
  initialView,
  colorBy,
}: {
  project: string;
  /** Chosen on arrival, the way a catalog card opens the page (UI-46). */
  initialSpace?: string;
  initialEndpoint?: string;
  /** One entity of the endpoint, opened on its detail as the page mounts (UI-46, T-1017). */
  initialEntityId?: string;
  /**
   * The type and the filter the grid opens narrowed by (UI-64, UI-67): the assistant's `entities`
   * hand-off carries the question's own filter, so nobody types it again.
   */
  initialType?: string;
  initialQ?: string;
  /** Opens on the map when the type has a place (T-6442). */
  initialView?: "map";
  /** The number attribute the map colours its points by. */
  colorBy?: string;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const identity = useIdentity();
  const locale = i18n.language;
  const spaces = useProjectList(project, "spaces");
  const endpoints = useProjectList(project, "endpoints");
  const models = useProjectList(project, "datamodels");

  // `null` is "nobody has chosen a space yet", which is not the same as the empty choice: a person
  // who picks "—" keeps it, while an arrival that named only an endpoint takes that endpoint's own
  // space below.
  const [chosenSpace, setSpace] = useState<string | null>(initialSpace ?? null);
  const [endpointChoice, setEndpointChoice] = useState<string>(initialEndpoint ?? "");
  const [askedQuery, setQuery] = useState<EntityQuery>(
    initialType ? { type: initialType, q: initialQ } : {},
  );
  const [selected, setSelected] = useState<string | null>(initialEntityId ?? null);
  // The address follows `selected`, so the prop does too: only its first value is a link.
  const [linkedEntity] = useState(initialEntityId);
  // The type's entities, or its data-quality report (T-3252).
  const [askedView, setView] = useState<"entities" | "quality" | "map">(initialView ?? "entities");
  /** What the grid's filter row asks, for the map and the export of the view (T-3253). */
  const [gridAsk, setGridAsk] = useState<{ q?: string; idPattern?: string }>({});
  /** The grid's checked rows: the export then holds only them, as on a space's page (T-6107). */
  const [checked, setChecked] = useState<string[]>([]);
  // The grid's read was refused: "Your access" would contradict it, so it is not offered (T-6107).
  const [refused, setRefused] = useState(false);
  /** Bumped when an entity is removed, so the grid reads the endpoint again. */
  const [generation, setGeneration] = useState(0);

  /**
   * The `entities` hand-off names the endpoint and not its space (UI-59), and the page reads an
   * endpoint only inside a space: without this the grid would open on nothing. The endpoint's own
   * space is known once the endpoints have loaded, so it is derived rather than assigned.
   */
  const handedSpace = (endpoints.data ?? []).find(
    (e) => e.metadata.name === initialEndpoint,
  );
  // Nobody chose and nothing was handed: the page opens on the first of the project's first spaces
  // that holds entities, so it shows data rather than a card asking for a pick (T-6107) or an
  // empty catalogue space that sorts first. A count that has not come yet is waited for.
  const opening = chosenSpace === null && !initialEndpoint;
  const candidates = (spaces.data ?? []).slice(0, COUNTED_SPACES).map((item) => item.metadata.name);
  const usage = useQueries({ queries: (opening ? candidates : []).map((name) => spaceUsageQuery(project, name)) });
  const firstSpace = firstWithData(candidates, usage) ?? "";
  const space = chosenSpace ?? (handedSpace ? spaceOf(handedSpace) : initialEndpoint ? "" : firstSpace);
  const spaceEndpoints = (endpoints.data ?? []).filter((e) => spaceOf(e) === space);
  const chosenEndpoint = spaceEndpoints.find((e) => e.metadata.name === endpointChoice);
  // A link that names an Endpoint this person is not shown in the space reads through nothing
  // and says so, rather than through another Endpoint with another Policy (T-5604).
  const unshownEndpoint =
    endpointChoice !== "" && endpoints.data !== undefined && chosenEndpoint === undefined ? endpointChoice : undefined;
  const endpoint =
    unshownEndpoint !== undefined
      ? undefined
      : (chosenEndpoint ?? pickReadEndpoint(spaceEndpoints, identity ? (identity.groups ?? []) : null, project));
  const endpointName = endpoint?.metadata.name;

  const slug = typeof endpoint?.spec.slug === "string" ? String(endpoint.spec.slug) : undefined;
  // A link naming an entity and not its type (the command palette's "Open entity") reads the entity
  // once through the endpoint to learn its type, so the grid opens on it and shows its detail.
  const linkedType = useQuery({
    queryKey: ["explore-entity", slug, selected],
    queryFn: () => fetchEntity(slug!, selected!),
    enabled: Boolean(slug && selected && !askedQuery.type),
  });
  const typeOfLink = askedQuery.type ? undefined : linkedType.data?.type;

  const spaceManifest = (spaces.data ?? []).find((s) => s.metadata.name === space);
  const model = (models.data ?? []).find(
    (m) => m.metadata.name === refName(spaceManifest?.spec.dataModelRef),
  );
  const types = useMemo(() => (model ? entityTypesOf(model) : []), [model]);
  // Only the model's own types make an address's type unknown: the broker's list is the person's
  // readable types, so a type missing there may exist and be refused (T-5622).
  const knownTypes = types.length > 0 ? types : undefined;
  const access = useAccess(slug);
  // The types this endpoint lets the person read (T-3218): a grant without a type covers every
  // one, and an access document not read yet says nothing, so the list then stays as it is.
  const readable = useMemo(() => {
    const grants = (access.data?.permissions ?? []).filter((entry) =>
      (entry.actions ?? []).some((action) => action.startsWith("query") || action.startsWith("retrieve")),
    );
    if (access.data === undefined || grants.some((entry) => !entry.resource?.type || entry.resource.type === "*")) {
      return undefined;
    }
    return [...new Set(grants.map((entry) => entry.resource?.type).filter((type): type is string => Boolean(type)))];
  }, [access.data]);
  // Without a type asked or linked the grid opens on the first one the person may read (T-6107);
  // a linked entity waits for its own type instead. A row opened here stays on the page's type: the
  // grid is keyed by it, so losing it for a moment threw the person's filters away (T-6454).
  const firstType = (selected && selected === linkedEntity) || !slug || access.isPending ? undefined : (readable ? types.find((type) => readable.includes(type)) : types[0]);
  const query: EntityQuery = useMemo(
    () =>
      askedQuery.type
        ? askedQuery
        : typeof typeOfLink === "string"
          ? { ...askedQuery, type: typeOfLink }
          : firstType
            ? { ...askedQuery, type: firstType }
            : askedQuery,
    [askedQuery, typeOfLink, firstType],
  );
  // What is chosen stands in the address as it changes (UI-89, T-3239): a reload or a link sent
  // to a colleague opens the same space, endpoint, type, search and entity. `space` and
  // `endpoint` are also the assistant's hand-off; written as the page's own, they mount nothing
  // afresh. Replaced, not pushed: each pick is not a step back needs to retrace. The endpoint
  // written is the one read, picked by hand or for the person, so a colleague opening the link
  // reads through the same one or is told why not (T-5604).
  useEffect(() => {
    const address = new URLSearchParams(window.location.search);
    const put = (key: string, value: string | null | undefined) => {
      if (value) address.set(key, value);
      else address.delete(key);
    };
    if (chosenSpace !== null) put("space", chosenSpace);
    put("endpoint", endpointChoice || endpointName);
    put("type", query.type);
    put("q", query.q);
    put("entityId", selected);
    replaceOwnSearch(address);
  }, [chosenSpace, endpointChoice, endpointName, query.type, query.q, selected]);
  /** The space's model is not known yet: the space or the model list is still on its way. */
  const modelPending = spaces.isPending || models.isPending;
  const unknownType =
    query.type !== undefined && knownTypes !== undefined && !knownTypes.includes(query.type) ? query.type : undefined;
  // The slots and the denied list are memoized because the grid's config is built from them: a new
  // array on every render would make the grid rebuild its source and read the endpoint again, on
  // and on. It also stops the LinkML being parsed once per render.
  const modelSource = useModelSource(project, model);
  const slots = useMemo(() => filterSlotsOf(modelSource, query.type), [modelSource, query.type]);
  const located = isLocated(slots);
  // A hand-off to the map of a type without a place opens its table.
  const view = askedView === "map" && !located ? "entities" : askedView;
  const geoAttrs = useMemo(() => {
    // The model's GeoProperties, `location` first; a model that marks none still has `location`.
    const named = slots.filter((slot) => slot.kind === "GeoProperty").map((slot) => slot.name);
    return named.length === 0 ? ["location"] : [...named].sort((a, b) => Number(b === "location") - Number(a === "location"));
  }, [slots]);
  const popupColumns = useMemo(() => {
    const plain = slots.filter((slot) => slot.kind !== "GeoProperty" && slot.kind !== "Relationship").map((slot) => slot.name);
    const chosen = (query.attrs ?? []).filter((attr) => plain.includes(attr));
    return [...new Set(["name", ...chosen, ...plain])].filter((attr) => plain.includes(attr));
  }, [slots, query.attrs]);
  // An enum slot is edited and filtered in the grid by picking its values (UI-86).
  const enums = useMemo(() => enumsOfModel(modelSource, query.type, locale), [modelSource, query.type, locale]);
  const rules = useMemo(() => rulesOfModel(modelSource, query.type), [modelSource, query.type]);
  // A relationship end is picked from the target's entities the person can read (UI-84).
  const relations = useMemo(() => relationsOfModel(modelSource, query.type), [modelSource, query.type]);
  // What the person may do with each attribute, as the endpoint decides it (T-5528): the grid's
  // editors, the filters' hidden marks and the cell inspector all render this one answer.
  const attributeAccess = useAttributeAccess(slug, query.type);
  const denied = useMemo(() => deniedOf(attributeAccess.data, t), [attributeAccess.data, t]);
  // UI-44, EP-55: a control the grant denies stays visible and says why, rather than working
  // until the gateway refuses it. The grant is the endpoint's own `/access` document, already
  // read above for the attributes it hides (T-1021).
  // Delete is offered on the endpoint's own decision (T-5530), never on a grant read in the browser.
  const deleteDecision = useAccessCheck(slug, query.type, "deleteEntity");
  const mayDelete = deleteDecision.data === true;
  const deleteRefusal = deleteDecision.isPending
    ? t("entityGrid.inspector.loading")
    : deleteDecision.isError
      ? t("explore.deleteUnchecked")
      : t("explore.deleteDenied", { type: query.type ?? "" });
  // The attributes this person may correct in place, by the endpoint's own grant (UI-67).
  // A formula field is its pipeline's to write (DM-80): shown, never edited in place.
  const editableAttrs = useMemo(
    () => writableOf(attributeAccess.data).filter((attr) => slots.some((slot) => slot.name === attr && !slot.formula)),
    [attributeAccess.data, slots],
  );
  // The React Compiler memoizes this; a manual useCallback over `endpoint` could not be preserved.
  const explainAccess = (attribute: string, action: "read" | "write" | "delete") =>
    explainAttribute(project, endpointName ?? "", query.type ?? "", attribute, action);

  /**
   * What the grid is told to show. The endpoint is a slug and never a URL (EP-55), and the filters
   * the page composed from the DataModel arrive as the grid's preset, so the grid's own filter row
   * narrows what the page already asked for rather than replacing it.
   */
  const config: ResolvedGridConfig | null = useMemo(() => {
    // A q the broker would refuse is not sent: the field says how to fix it (T-4264, T-4160 #22).
    // Nor is a type read before the model says whether the space has it (T-5622).
    if (!slug || !query.type || modelPending || unknownType !== undefined || qProblem(query.q)) {
      return null;
    }
    // The grid waits for the person's decisions (T-5530): an attribute they may not read is then
    // never asked for, and no editor opens before the server said it may.
    if (attributeAccess.isPending) {
      return null;
    }
    const parsed = parseGridConfig({
      source: { kind: "endpoint", slug },
      type: query.type,
      // `name`, the Smart Data Models label, is the primary field: first, pinned, opening the row.
      columns: (query.attrs ?? []).map((attr) => ({ attr, ...(attr === "name" ? { pinned: true } : {}) })),
      filters: { preset: { q: query.q, attrs: query.attrs, scopeQ: query.scopeQ } },
      pageSize: PAGE_SIZE,
      history: { enabled: true },
      ...(editableAttrs.length > 0 ? { mode: "edit" as const, editableAttrs } : {}),
    });
    return parsed.config ?? null;
  }, [slug, query.type, modelPending, unknownType, query.q, query.attrs, query.scopeQ, editableAttrs, attributeAccess.isPending]);

  // A stable callback and the same object back when nothing moved: the grid hands its page over
  // from an effect, so a new function or a new object on every render would read and re-render
  // without end.
  const onQuery = useCallback((asked: { q?: string; idPattern?: string }) => {
    setGridAsk((previous) => (previous.q === asked.q && previous.idPattern === asked.idPattern ? previous : asked));
  }, []);
  // A row opens in the grid's own detail (T-5619): its fields as the grid draws them, its history,
  // and under them what the page adds, the removal of UI-60 and the entity as the endpoint holds it.
  const detailExtra = (row: RichRow) =>
    slug ? (
      <ExploreRowActions
        key={row.id}
        slug={slug}
        id={row.id}
        deniedReason={mayDelete ? undefined : deleteRefusal}
        onDeleted={() => {
          setSelected(null);
          // The grid holds a page that still lists the entity; a new key makes it read the endpoint
          // again rather than show a row that is gone.
          setGeneration((each) => each + 1);
        }}
      />
    ) : null;

  // A plain value of a person who may write it: what the row's "Correct values" tab offers (T-5326).
  const correctable = slots.filter((slot) => slot.kind === "Property" && editableAttrs.includes(slot.name)).map((slot) => slot.name);
  const detailTabs = (row: RichRow) =>
    slug && correctable.length > 0
      ? [{ id: "values", label: t("attributeHistory.tab"), content: <AttributeHistory slug={slug} entityId={row.id} attributes={correctable} /> }]
      : [];

  function changeQuery(next: EntityQuery) {
    setQuery(next);
    setSelected(null);
    setChecked([]);
  }

  return (
    <div className="flex flex-col gap-4">
      {/* The pickers sit on the description's line, small selects with their captions (T-6107):
          the grid below takes the width and the height. */}
      <PageHeader
        title={t("explore.title")}
        description={t("explore.description")}
        aside={
          <div className="flex flex-wrap items-center gap-x-4 gap-y-2">
            <span className="flex items-center gap-2">
              <label className="text-caption text-fg-muted" htmlFor="explore-space">
                {t("explore.space")}
              </label>
              <Select
                id="explore-space"
                className="w-auto max-w-64"
                value={space}
                // A project without a space has nothing to pick; the empty state below says why.
                disabled={spaces.data !== undefined && spaces.data.length === 0}
                title={spaces.data !== undefined && spaces.data.length === 0 ? t("explore.noSpaces") : undefined}
                onChange={(event) => {
                  setSpace(event.target.value);
                  setEndpointChoice("");
                  changeQuery({});
                }}
              >
                {space === "" ? <option value="">—</option> : null}
                {(spaces.data ?? []).map((s) => (
                  <option key={s.metadata.name} value={s.metadata.name}>
                    {localized(s.metadata.title, locale, s.metadata.name)}
                  </option>
                ))}
              </Select>
            </span>
            <span className="flex items-center gap-2">
              <label className="text-caption text-fg-muted" htmlFor="explore-endpoint">
                {t("explore.endpoint")}
              </label>
              <Select
                id="explore-endpoint"
                className="w-auto max-w-64"
                value={endpoint?.metadata.name ?? ""}
                disabled={!space}
                title={!space ? t("explore.pickSpaceFirst") : undefined}
                onChange={(event) => {
                  setEndpointChoice(event.target.value);
                  setSelected(null);
                  setChecked([]);
                }}
              >
                <option value="">—</option>
                {/* The space's own Endpoints first, an App's under "Applications": the group tells a
                    space's Endpoint from an App's of one title, where a slug in brackets did (T-5622). */}
                {endpointOptions(spaceEndpoints.filter((e) => !isAppEndpoint(e)), locale)}
                {spaceEndpoints.some(isAppEndpoint) ? (
                  <optgroup label={t("explore.endpointApps")}>{endpointOptions(spaceEndpoints.filter(isAppEndpoint), locale)}</optgroup>
                ) : null}
              </Select>
            </span>
          </div>
        }
      />
      {/* Without these a failed list left its picker at "—" and the page read like a project with
          nothing in it (T-1768). */}
      {spaces.isError ? (
        <ListFailed
          what={t("nav.spaces")}
          reason={reasonOf(spaces.error, t("explore.loadFailed"))}
          onRetry={() => void spaces.refetch()}
        />
      ) : null}
      {endpoints.isError ? (
        <ListFailed
          what={t("nav.endpoints")}
          reason={reasonOf(endpoints.error, t("explore.loadFailed"))}
          onRetry={() => void endpoints.refetch()}
        />
      ) : null}
      {space && spaceEndpoints.length === 0 && endpoints.isSuccess ? (
        <Alert tone="info">{t("explore.noEndpoint")}</Alert>
      ) : null}
      {unshownEndpoint !== undefined ? (
        <Alert tone="warning" title={t("explore.endpointUnshown", { name: unshownEndpoint })}>
          {t("explore.endpointUnshownPick", { name: unshownEndpoint })}
        </Alert>
      ) : null}

      {space ? (
        // The type, the area and the filters fold under one line once a type is chosen, so the rows
        // are the first thing on the page (T-6107).
        <Card padding="sm">
          <details open={!query.type}>
            <summary className="cursor-pointer text-body font-medium">
              {query.type ? t("explore.filtersOf", { type: query.type }) : t("explore.filtersPick")}
            </summary>
            <div className="mt-3 flex flex-col gap-3">
              <EntityFilters
                id="explore"
                types={types}
                space={space || undefined}
                slots={slots}
                value={query}
                onChange={changeQuery}
                denied={denied}
                readable={readable}
              />
              {query.type && unknownType === undefined ? (
                <p className="text-caption text-fg-muted">
                  {t("explore.typeInModel")} <TypeLink project={project} type={query.type} space={space} className="font-mono" />
                  {endpointName ? (
                    <>
                      {" · "}
                      {t("explore.readThrough")} {endpoint ? endpointTitle(endpoint, locale) : endpointName}
                    </>
                  ) : null}
                </p>
              ) : null}
            </div>
          </details>
        </Card>
      ) : null}

      {config && space && query.type ? (
        <Tabs
          id="explore-view"
          label={t("explore.views")}
          tabs={[
            { value: "entities", label: t("explore.entities") },
            // A type with a place opens on the map too (T-3256).
            ...(located ? [{ value: "map" as const, label: t("explore.map.tab") }] : []),
            { value: "quality", label: t("explore.quality.tab") },
          ]}
          value={view}
          onChange={setView}
        />
      ) : null}
      {config && space && query.type && view === "quality" ? (
        <div {...tabPanelProps("explore-view", "quality")}>
          <TypeQuality project={project} space={space} type={query.type} />
        </div>
      ) : null}
      {config && space && query.type && slug && located && view === "map" ? (
        <div {...tabPanelProps("explore-view", "map")}>
          <ExploreMap
            project={project}
            slug={slug}
            query={{ ...query, q: andQ(query.q, gridAsk.q), idPattern: gridAsk.idPattern }}
            columns={popupColumns}
            geo={geoAttrs}
            colorBy={colorBy}
            onOpen={setSelected}
          />
        </div>
      ) : null}
      {slug && query.type && !qProblem(query.q) && attributeAccess.isPending ? (
        <p role="status" className="text-caption text-fg-muted">
          {t("entityGrid.inspector.loading")}
        </p>
      ) : null}
      {config ? (
        // The table stays mounted under the other views: its filter row is the map's filter too,
        // and coming back finds it as it was left (T-3256).
        <div hidden={Boolean(space && query.type) && view !== "entities"} {...(space && query.type ? tabPanelProps("explore-view", "entities") : {})}>
          <PortalEntityGrid
            key={`${slug}-${query.type}-${generation}`}
            project={project}
            config={config}
            // Scroll through every matching entity, a window drawn at a time (T-3097).
            virtual
            enums={enums}
            relations={relations}
            rules={rules}
            onOpenRelationship={setSelected}
            onQuery={onQuery}
            // Numbered rows with a check box each, as on a space's page (T-6107).
            rowNumbers={{ selectRow: t("spaces.toolbar.selectRow"), selectAll: t("spaces.toolbar.selectAll") }}
            selected={checked}
            onSelectedChange={setChecked}
            // A filter that excludes everything says so, and how to get rows back (T-4160 #22).
            emptyText={query.q ? t("explore.noMatch") : undefined}
            openRow={selected}
            onOpenRow={setSelected}
            detailExtra={detailExtra}
            detailTabs={detailTabs}
            access={attributeAccess.isError ? null : attributeAccess.data}
            explainAccess={endpointName ? explainAccess : undefined}
            accessRequestHref={`/projects/${encodeURIComponent(project)}/access`}
            onRefused={setRefused}
            toolbar={
              slug ? (
                <div className="flex flex-wrap items-center gap-2">
                  {endpoint && query.type && !servedRepresentations(endpoint.spec ?? {}).includes("csv") ? (
                    // Nothing to download: the button says why instead of opening an empty menu.
                    <span className="flex items-center gap-2">
                      <Button size="sm" variant="ghost" disabled disabledReason={t("explore.exportNone")}>
                        {t("explore.export")}
                      </Button>
                      {/* The reason in sight too; the button already says it to a screen reader. */}
                      <span aria-hidden="true" className="text-caption text-fg-muted">
                        {t("explore.exportNone")}
                      </span>
                    </span>
                  ) : endpoint && query.type ? (
                    // The space page's own export (T-6107): the formats this Endpoint serves, the
                    // checked rows alone once some are checked.
                    <ToolbarPopover label={t("explore.export")} count={checked.length || undefined} wide anchored>
                      {checked.length > MAX_EXPORT_IDS ? (
                        <p className="text-caption text-fg-muted">{t("spaces.toolbar.exportTooMany", { n: MAX_EXPORT_IDS })}</p>
                      ) : (
                        <ExportLinks
                          endpoints={[endpoint]}
                          type={query.type}
                          q={andQ(query.q, gridAsk.q)}
                          idPattern={gridAsk.idPattern}
                          attrs={query.attrs ?? []}
                          ids={checked}
                          label={checked.length > 0 ? t("spaces.toolbar.exportSelected") : undefined}
                        />
                      )}
                    </ToolbarPopover>
                  ) : null}
                  {/* A refused read offers nothing built on it: no access summary, no reuse, no chart. */}
                  {refused ? null : (
                    <>
                      <ToolbarPopover label={t("explore.yourAccess")} wide>
                        <AccessPanel slug={slug} type={unknownType === undefined ? query.type : undefined} access={access} />
                      </ToolbarPopover>
                      <UseThisData
                        project={project}
                        slug={slug}
                        query={{ ...query, q: andQ(query.q, gridAsk.q), idPattern: gridAsk.idPattern }}
                        open={endpoint?.spec.audience === "public"}
                      />
                      {endpoint && query.type ? (
                        <ChartFromView
                          project={project}
                          endpoint={endpoint.metadata.name}
                          slug={slug}
                          type={query.type}
                          q={andQ(query.q, gridAsk.q)}
                          slots={slots}
                        />
                      ) : null}
                    </>
                  )}
                </div>
              ) : null
            }
          />
        </div>
      ) : space && models.isError ? (
        <ListFailed
          what={t("nav.models")}
          reason={reasonOf(models.error, t("explore.loadFailed"))}
          onRetry={() => void models.refetch()}
        />
      ) : unshownEndpoint !== undefined || (query.type && modelPending) ? null : space && unknownType !== undefined ? (
        // An address naming a type the space has none of says so, rather than an empty grid that
        // reads like a type with nothing in it yet (T-5622).
        <EmptyState
          title={t("explore.unknownType", { type: unknownType })}
          description={(knownTypes ?? []).length > 0 ? t("explore.unknownTypeHint") : t("entities.noTypes")}
          icon="explore"
          action={
            <div className="flex flex-wrap justify-center gap-2">
              {(knownTypes ?? []).map((type) => (
                <Button key={type} size="sm" variant="secondary" onClick={() => changeQuery({ type, scopeQ: query.scopeQ })}>
                  {type}
                </Button>
              ))}
            </div>
          }
        />
      ) : space ? (
        // A linked Endpoint this person is not shown has its own warning above.
        <EmptyState
          title={t("explore.noType")}
          description={t("explore.noTypeHint")}
          icon="explore"
        />
      ) : (spaces.data ?? []).length > 0 ? (
        // The first pick, one click away: an empty page under two empty selects said nothing
        // about where to start (T-2755).
        <EmptyState
          title={t("explore.noSpace")}
          description={t("explore.noSpaceHint")}
          icon="explore"
          action={
            <div className="flex flex-wrap justify-center gap-2">
              {(spaces.data ?? []).map((s) => (
                <Button
                  key={s.metadata.name}
                  size="sm"
                  variant="secondary"
                  onClick={() => {
                    setSpace(s.metadata.name);
                    setEndpointChoice("");
                    changeQuery({});
                  }}
                >
                  {localized(s.metadata.title, locale, s.metadata.name)}
                </Button>
              ))}
            </div>
          }
        />
      ) : spaces.isSuccess ? (
        // The page opens on the first space (T-6107): only a project with none ends up here.
        <EmptyState title={t("explore.noSpaces")} description={t("explore.noSpacesHint")} icon="explore" />
      ) : null}

    </div>
  );
}

/** An Endpoint as a person reads it: its title, else the App it serves or its own name. */
function endpointTitle(endpoint: Manifest, locale: string): string {
  const name = endpoint.metadata.name;
  return localized(endpoint.metadata.title, locale, isAppEndpoint(endpoint) ? name.replace(/^app-/, "") : name);
}

/** One group's options; two of one title in a group are told apart by their names (T-3218). */
function endpointOptions(group: Manifest[], locale: string): JSX.Element[] {
  return group.map((e) => {
    const title = endpointTitle(e, locale);
    const twin = group.some((other) => other !== e && endpointTitle(other, locale) === title);
    return (
      <option key={e.metadata.name} value={e.metadata.name}>
        {twin ? `${title} (${e.metadata.name})` : title}
      </option>
    );
  });
}
