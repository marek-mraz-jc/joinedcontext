/**
 * The SDK's entity grid inside the Portal (T-1439; UI-71, SDK-29): one component, linked and not
 * copied, with everything a host owes it supplied here.
 *
 * The grid itself knows nothing about the Portal: it takes its strings, its source and its state
 * from whoever renders it. This adapter is that whoever — the Portal's i18n for every label, the
 * Portal's own origin transport (so the person's session and CSRF header travel as with every other
 * Portal call, and the Endpoint's Policy decides each read), and the column layout a person set
 * kept per project and per type. Every Portal place that shows entities renders this, so a change
 * to the grid reaches all of them and none of them forks it.
 */
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { useSignIn } from "../../auth/AuthProvider";
import { EntityGrid, originTransport, SourceError, sourceFor } from "@joinedcontext/sdk/grid";
import type { ColumnLayout, EntityGridProps, GeoLabels, GridLabels, GridMapLabels, GridSource, GridState, ResolvedGridConfig } from "@joinedcontext/sdk/grid";

/** Where the layout a person set is kept, per project and per type. */
function layoutKey(project: string, type: string): string {
  return `jc.grid.${project}.${type}`;
}

/**
 * The parts of the grid's state that belong to the person rather than to the query: which metadata
 * columns are shown and how the page is ordered. The offset, the filters and the pending edits are
 * deliberately not kept: coming back to a page three of a filtered answer that no longer matches is
 * worse than starting at the top, and an unapplied edit must not outlive the page it was typed on.
 */
type Layout = Pick<GridState, "shown" | "sort">;

/**
 * The column order, widths and renames a person set from the header (T-5192), kept per project and
 * type in this browser beside the layout above; a host that keeps its own passes `layout`.
 */
function columnsKey(project: string, type: string): string {
  return `${layoutKey(project, type)}.columns`;
}

function readColumns(project: string, type: string): ColumnLayout {
  try {
    const raw = window.localStorage.getItem(columnsKey(project, type));
    const parsed = raw ? (JSON.parse(raw) as unknown) : null;
    return typeof parsed === "object" && parsed !== null && !Array.isArray(parsed) ? (parsed as ColumnLayout) : {};
  } catch {
    return {};
  }
}

function writeColumns(project: string, type: string, columns: ColumnLayout): void {
  try {
    window.localStorage.setItem(columnsKey(project, type), JSON.stringify(columns));
  } catch {
    // Kept for this page only: the widths are a convenience, not the data.
  }
}

/** The order this browser remembers for a type's grid, for a host that shows it (T-5188). */
export function rememberedSort(project: string, type: string): GridState["sort"] {
  return readLayout(project, type)?.sort ?? null;
}

function readLayout(project: string, type: string): Layout | null {
  try {
    const raw = window.localStorage.getItem(layoutKey(project, type));
    if (!raw) {
      return null;
    }
    const parsed = JSON.parse(raw) as Layout;
    return typeof parsed === "object" && parsed !== null ? parsed : null;
  } catch {
    // A private window, blocked site data, or a value another version wrote: the grid opens with
    // its own defaults rather than failing to open.
    return null;
  }
}

function writeLayout(project: string, type: string, layout: Layout): void {
  try {
    window.localStorage.setItem(layoutKey(project, type), JSON.stringify(layout));
  } catch {
    // Nothing else changes: a layout that cannot be kept is a convenience, not the data.
  }
}

/** Every string the grid shows, from the Portal's own catalogue (`entityGrid.*`). */
/** The cell inspector's words (T-5528), each under `entityGrid.inspector`. */
/** The Changes tab's words read one to one from `entityGrid.changes` (T-6333). */
const CHANGES_KEYS = [
  "tab", "attribute", "person", "all", "before", "after", "channel", "organization", "reason", "noValue", "empty", "emptyFiltered", "showAll",
  "loading", "error", "retry", "copy", "copied", "changed",
] as const satisfies readonly (keyof NonNullable<GridLabels["changesLabels"]>)[];

const INSPECTOR_KEYS = [
  "title", "close", "read", "write", "delete", "allowed", "notAllowed", "loading", "failed", "grant", "notInGrant",
  "noGrant", "formula", "prohibited", "prohibitedBy", "prohibitedHidden", "naming", "namingFailed", "askAccess",
] as const satisfies readonly (keyof NonNullable<GridLabels["inspectorLabels"]>)[];

export function gridLabels(t: (key: string, values?: Record<string, string>) => string): GridLabels {
  return {
    id: t("entityGrid.id"),
    type: t("entityGrid.type"),
    scope: t("entityGrid.scope"),
    createdAt: t("entityGrid.createdAt"),
    modifiedAt: t("entityGrid.modifiedAt"),
    observedAt: t("entityGrid.observedAt"),
    unit: t("entityGrid.unit"),
    datasetId: t("entityGrid.datasetId"),
    empty: t("entityGrid.empty"),
    loading: t("entityGrid.loading"),
    filtering: t("entityGrid.filtering"),
    previous: t("entityGrid.previous"),
    next: t("entityGrid.next"),
    page: t("entityGrid.page"),
    showMetadata: t("entityGrid.showMetadata"),
    error: t("entityGrid.error"),
    filter: t("entityGrid.filter"),
    noFilter: t("entityGrid.noFilter"),
    filterHintText: t("entityGrid.hint.filterText"),
    filterHintNumber: t("entityGrid.hint.filterNumber"),
    filterHintDate: t("entityGrid.hint.filterDate"),
    filterHintEnum: t("entityGrid.hint.filterEnum"),
    filterExampleText: t("entityGrid.filterExampleText"),
    filterExampleNumber: t("entityGrid.filterExampleNumber"),
    filterExampleId: t("entityGrid.filterExampleId"),
    filterHintOther: t("entityGrid.hint.filterOther"),
    editAsTextHint: t("entityGrid.hint.editAsText"),
    exampleLead: t("entityGrid.exampleLead"),
    queryAll: t("entityGrid.queryAll"),
    copied: t("entityGrid.copied"),
    ops: {
      contains: t("entityGrid.ops.contains"),
      equals: t("entityGrid.ops.equals"),
      notEquals: t("entityGrid.ops.notEquals"),
      // The comparisons in words, as a person says them (T-6328).
      gt: t("entityGrid.ops.gt"),
      gte: t("entityGrid.ops.gte"),
      lt: t("entityGrid.ops.lt"),
      lte: t("entityGrid.ops.lte"),
      between: t("entityGrid.ops.between"),
      empty: t("entityGrid.ops.isEmpty"),
      present: t("entityGrid.ops.present"),
      pattern: t("entityGrid.ops.pattern"),
      startsWith: t("entityGrid.ops.startsWith"),
      anyOf: t("entityGrid.ops.anyOf"),
    },
    value: t("entityGrid.value"),
    upperValue: t("entityGrid.upperValue"),
    query: t("entityGrid.query"),
    copyQuery: t("entityGrid.copyQuery"),
    editAsText: t("entityGrid.editAsText"),
    filterRow: t("entityGrid.filterRow"),
    sortPage: t("entityGrid.sortPage"),
    sortAll: t("entityGrid.sortAll"),
    sortedLoaded: t("entityGrid.sortedLoaded"),
    notReadable: t("entityGrid.notReadable"),
    queryRefused: t("entityGrid.queryRefused"),
    errorDetail: t("entityGrid.errorDetail"),
    matching: t("entityGrid.matching", { n: "{n}" }),
    noTotal: t("entityGrid.noTotal"),
    history: t("entityGrid.history"),
    areaPick: t("entityGrid.areaPick"),
    signedOutTitle: t("entityGrid.signedOutTitle"),
    signedOutBody: t("entityGrid.signedOutBody"),
    signIn: t("entityGrid.signIn"),
    noAccessTitle: t("entityGrid.noAccessTitle"),
    noAccessBody: t("entityGrid.noAccessBody"),
    rowsPerPage: t("entityGrid.rowsPerPage"),
    areaClear: t("entityGrid.areaClear"),
    historyLabels: {
      title: t("entityGrid.history"),
      window: {
        hour: t("entityGrid.historyWindow.hour"),
        day: t("entityGrid.historyWindow.day"),
        week: t("entityGrid.historyWindow.week"),
        custom: t("entityGrid.historyWindow.custom"),
      },
      from: t("entityGrid.from"),
      to: t("entityGrid.to"),
      at: t("entityGrid.observedAt"),
      value: t("entityGrid.value"),
      unit: t("entityGrid.unit"),
      empty: t("entityGrid.historyEmpty"),
      cut: t("entityGrid.historyCut"),
      copy: t("entityGrid.historyCopy"),
      close: t("entityGrid.close"),
      loading: t("entityGrid.loading"),
      error: t("entityGrid.historyNotLoaded"),
      retry: t("entityGrid.historyRetry"),
      refused: t("entityGrid.historyRefused"),
      versions: t("entityGrid.historyVersions"),
      versionsRefused: t("entityGrid.historyVersionsRefused"),
      supersededAt: t("entityGrid.historySuperseded"),
      changedBy: t("entityGrid.historyChangedBy"),
      reason: t("entityGrid.historyReason"),
    },
    changesLabels: {
      ...Object.fromEntries(CHANGES_KEYS.map((key) => [key, t(`entityGrid.changes.${key}`)])),
      // The placeholder goes through as it is: the dialog fills it with the field's label.
      showChanges: t("entityGrid.changes.showChanges", { attr: "{attr}" }),
      actorKinds: {
        person: t("entityGrid.changes.kindPerson"),
        pipeline: t("entityGrid.changes.kindPipeline"),
        serviceAccount: t("entityGrid.changes.kindServiceAccount"),
        agent: t("entityGrid.changes.kindAgent"),
      },
    },
    // The placeholders go through as they are: the inspector fills them with the policy or the reason.
    inspectorLabels: Object.fromEntries(
      INSPECTOR_KEYS.map((key) => [key, t(`entityGrid.inspector.${key}`, { policy: "{policy}", reason: "{reason}" })]),
    ) as Record<(typeof INSPECTOR_KEYS)[number], string>,
    metadata: t("entityGrid.metadata"),
    metadataLabels: {
      created: t("entityGrid.metaCreated"),
      modified: t("entityGrid.metaModified"),
      changedBy: t("entityGrid.metaChangedBy"),
      organization: t("entityGrid.metaOrganization"),
      channel: t("entityGrid.metaChannel"),
      reason: t("entityGrid.metaReason"),
      hidden: t("entityGrid.metaHidden"),
      hiddenWhy: {
        history: t("entityGrid.metaHiddenHistory"),
        member: t("entityGrid.metaHiddenMember"),
        public: t("entityGrid.metaHiddenPublic"),
      },
      notRecorded: t("entityGrid.metaNotRecorded"),
      noReason: t("entityGrid.metaNoReason"),
      loading: t("entityGrid.loading"),
      error: t("entityGrid.metaError"),
      actorKind: {
        person: t("entityGrid.metaKindPerson"),
        pipeline: t("entityGrid.metaKindPipeline"),
        agent: t("entityGrid.metaKindAgent"),
        serviceAccount: t("entityGrid.metaKindServiceAccount"),
      },
      channels: {
        ui: t("entityGrid.metaChannelUi"),
        api: t("entityGrid.metaChannelApi"),
        pipeline: t("entityGrid.metaChannelPipeline"),
        mqtt: t("entityGrid.metaChannelMqtt"),
        assistant: t("entityGrid.metaChannelAssistant"),
      },
    },
    edit: t("entityGrid.edit"),
    pending: t("entityGrid.pending"),
    review: t("entityGrid.review"),
    apply: t("entityGrid.apply"),
    discard: t("entityGrid.discard"),
    entity: t("entityGrid.entity"),
    copyId: t("entityGrid.copyId"),
    attribute: t("entityGrid.attribute"),
    before: t("entityGrid.before"),
    after: t("entityGrid.after"),
    observedKeep: t("entityGrid.observedKeep"),
    observedNow: t("entityGrid.observedNow"),
    applying: t("entityGrid.applying"),
    reason: t("entityGrid.reason"),
    reasonDefault: t("entityGrid.reasonDefault"),
    reasonNeeded: t("entityGrid.reasonNeeded"),
    saved: t("entityGrid.saved"),
    undo: t("entityGrid.undo"),
    undoReason: t("entityGrid.undoReason"),
    undone: t("entityGrid.undone"),
    refusedHere: t("entityGrid.refusedHere"),
    notInList: t("entityGrid.notInList"),
    relationSearch: t("entityGrid.relationSearch"),
    relationNone: t("entityGrid.relationNone"),
    relationRemove: t("entityGrid.relationRemove"),
    relationFailed: t("entityGrid.relationFailed"),
    relationMore: t("entityGrid.relationMore"),
    openRow: t("entityGrid.openRow"),
    groupNoValue: t("entityGrid.groupNoValue"),
    rowDetail: t("entityGrid.rowDetail"),
    rowFields: t("entityGrid.rowFields"),
    rowCopyId: t("entityGrid.rowCopyId"),
    rowNotSet: t("entityGrid.rowNotSet"),
    close: t("entityGrid.close"),
    pasted: t("entityGrid.pasted"),
    skipped: t("entityGrid.skipped"),
    notAllowed: t("entityGrid.notAllowed"),
    locked: t("entityGrid.locked"),
    lockReadOnly: t("entityGrid.lockReadOnly"),
    lockNotOffered: t("entityGrid.lockNotOffered"),
    pasteTooLarge: t("entityGrid.pasteTooLarge"),
    loaded: t("entityGrid.loaded"),
    of: t("entityGrid.of"),
    mustBeInteger: t("entityGrid.mustBeInteger"),
    mustBeNumber: t("entityGrid.mustBeNumber"),
    mustBeBoolean: t("entityGrid.mustBeBoolean"),
    mustBeDate: t("entityGrid.mustBeDate"),
    mustBeUri: t("entityGrid.mustBeUri"),
    atLeast: t("entityGrid.atLeast"),
    atMost: t("entityGrid.atMost"),
    patternMismatch: t("entityGrid.patternMismatch"),
    required: t("entityGrid.required"),
    toCorrect: t("entityGrid.toCorrect"),
    yes: t("entityGrid.yes"),
    no: t("entityGrid.no"),
    columnMenu: t("entityGrid.columnMenu"),
    sortAsc: t("entityGrid.sortAsc"),
    sortDesc: t("entityGrid.sortDesc"),
    filterColumn: t("entityGrid.filterColumn"),
    hideColumn: t("entityGrid.hideColumn"),
    hiddenFields: t("entityGrid.hiddenFields"),
    showColumn: t("entityGrid.showColumn"),
    showAllColumns: t("entityGrid.showAllColumns"),
    renameColumn: t("entityGrid.renameColumn"),
    renameReset: t("entityGrid.renameReset"),
    columnWidth: t("entityGrid.columnWidth"),
    narrower: t("entityGrid.narrower"),
    wider: t("entityGrid.wider"),
    moveLeft: t("entityGrid.moveLeft"),
    moveRight: t("entityGrid.moveRight"),
    details: t("entityGrid.details"),
    filterValues: t("entityGrid.filterValues"),
    searchValues: t("entityGrid.searchValues"),
    searchValuesHint: t("entityGrid.searchValuesHint"),
    selectAll: t("entityGrid.selectAll"),
    onlyValue: t("entityGrid.onlyValue"),
    valuesLoading: t("entityGrid.valuesLoading"),
    valuesFailed: t("entityGrid.valuesFailed"),
    noValues: t("entityGrid.noValues"),
    valuesCapped: t("entityGrid.valuesCapped"),
    filterCondition: t("entityGrid.filterCondition"),
    applyFilter: t("entityGrid.applyFilter"),
    clearFilter: t("entityGrid.clearFilter"),
    openFilter: t("entityGrid.openFilter"),
    filtered: t("entityGrid.filtered"),
    activeFilters: t("entityGrid.activeFilters"),
    removeFilter: t("entityGrid.removeFilter"),
    clearFilters: t("entityGrid.clearFilters"),
    kindText: t("entityGrid.kindText"),
    kindNumber: t("entityGrid.kindNumber"),
    kindBoolean: t("entityGrid.kindBoolean"),
    kindDatetime: t("entityGrid.kindDatetime"),
    kindGeo: t("entityGrid.kindGeo"),
    kindRelationship: t("entityGrid.kindRelationship"),
    kindLanguage: t("entityGrid.kindLanguage"),
    kindStructured: t("entityGrid.kindStructured"),
    kindList: t("entityGrid.kindList"),
    points: t("entityGrid.points"),
    openTarget: t("entityGrid.openTarget"),
    moreInstances: t("entityGrid.moreInstances"),
    wrongKind: t("entityGrid.wrongKind"),
    drillTrail: t("entityGrid.drillTrail"),
    drillBack: t("entityGrid.drillBack"),
    defaultInstance: t("entityGrid.defaultInstance"),
  };
}

/** The geometry editor's words, for the editor the map panel opens (UI-72, T-1442). */
export function geoEditorLabels(t: (key: string) => string): GeoLabels {
  return {
    tools: t("entityGrid.geo.tools"),
    select: t("entityGrid.geo.select"),
    undo: t("entityGrid.geo.undo"),
    redo: t("entityGrid.geo.redo"),
    remove: t("entityGrid.geo.remove"),
    removeShape: t("entityGrid.geo.removeShape"),
    point: t("entityGrid.geo.point"),
    longitude: t("entityGrid.geo.longitude"),
    latitude: t("entityGrid.geo.latitude"),
    noGeometry: t("entityGrid.geo.noGeometry"),
    paste: t("entityGrid.geo.paste"),
    take: t("entityGrid.geo.take"),
    upload: t("entityGrid.geo.upload"),
    notJson: t("entityGrid.geo.notJson"),
    tooLarge: t("entityGrid.geo.tooLarge"),
    modes: {
      point: t("entityGrid.geo.modes.point"),
      linestring: t("entityGrid.geo.modes.linestring"),
      polygon: t("entityGrid.geo.modes.polygon"),
      rectangle: t("entityGrid.geo.modes.rectangle"),
      circle: t("entityGrid.geo.modes.circle"),
      freehand: t("entityGrid.geo.modes.freehand"),
    },
  };
}

/** The map panel's words, in the person's language (UI-72). */
export function gridMapLabels(t: (key: string) => string): GridMapLabels {
  return {
    panel: t("entityGrid.map.panel"),
    offTheMap: t("entityGrid.map.offTheMap"),
    drawArea: t("entityGrid.map.drawArea"),
    clearArea: t("entityGrid.map.clearArea"),
    stopDrawing: t("entityGrid.map.stopDrawing"),
    areaAsked: t("entityGrid.map.areaAsked"),
    areaSimplified: t("entityGrid.map.areaSimplified"),
    editGeometry: t("entityGrid.map.editGeometry"),
    moved: t("entityGrid.map.moved"),
    reshaped: t("entityGrid.map.reshaped"),
    replaced: t("entityGrid.map.replaced"),
    noGeometry: t("entityGrid.map.noGeometry"),
    viewerCannotEdit: t("entityGrid.map.viewerCannotEdit"),
    pointsLoading: t("entityGrid.map.pointsLoading"),
    pointsShown: t("entityGrid.map.pointsShown"),
    pointsCapped: t("entityGrid.map.pointsCapped"),
    pointsFailed: t("entityGrid.map.pointsFailed"),
    selected: t("entityGrid.map.selected"),
    drawHint: t("entityGrid.map.drawHint"),
  };
}

export interface PortalEntityGridProps extends Omit<EntityGridProps, "labels" | "source" | "state" | "onStateChange"> {
  /** The project whose layout this is, so two projects do not share one person's columns. */
  project: string;
  config: ResolvedGridConfig;
  /** A source to use instead of the one the config names, for a test or a fixture. */
  source?: EntityGridProps["source"];
  /**
   * What a saved view sets (API/01 §30): the typed query and the order, held by the host so a view
   * can be applied and saved. The column layout stays the person's own either way.
   */
  view?: Pick<GridState, "filterText" | "sort"> & Partial<Pick<GridState, "filters">>;
  /** Every change of the grid's state, for the host that holds `view`. */
  onGridState?: (next: GridState) => void;
  /** What an empty answer says instead of "No entities here", e.g. that a filter excluded all. */
  emptyText?: string;
  /** Whether the last read was refused (401/403), for a host that shows the person's access beside it. */
  onRefused?: (refused: boolean) => void;
}

/**
 * The grid as the Portal renders it. The source comes from the config — an endpoint's slug or a
 * space's name, never a URL (EP-55) — over the Portal's own origin transport.
 */
export function PortalEntityGrid({ project, config, source, view, onGridState, emptyText, onRefused, ...rest }: PortalEntityGridProps): JSX.Element {
  const { t, i18n } = useTranslation();
  const signIn = useSignIn();
  // `t` changes identity on a language change, which is what rebuilds the labels; the language
  // itself is read below, for the source's own locale.
  const labels = useMemo(
    // A refused read (401/403) is the grid's own signed-out or no-access state, never a failure to retry (T-6107, T-6376).
    () => ({ ...gridLabels(t), ...(emptyText ? { empty: emptyText } : {}) }),
    [t, emptyText],
  );
  const mapLabels = useMemo(() => gridMapLabels(t), [t]);
  const geoLabels = useMemo(() => geoEditorLabels(t), [t]);

  // Keyed by what the source names, not by the config object: a page rebuilds its config when the
  // person's grant arrives (the editable attributes), and a new source would read the first page
  // again, twice more for an editor before the first row showed (T-5623).
  const sourceKey = JSON.stringify(config.source);
  const built = useMemo(() => {
    if (source) {
      return source;
    }
    const where = JSON.parse(sourceKey) as GridSource;
    if (where.kind === "fixture") {
      throw new Error("a fixture source is for the SDK's own examples; pass `source` instead");
    }
    return sourceFor(where, originTransport(), i18n.language);
  }, [source, sourceKey, i18n.language]);
  // The host's callback is read when a refusal is seen, not built into the source: a callback
  // written inline is new on each render, and a new source asks the endpoint again (T-6230).
  const refusedTo = useRef(onRefused);
  useEffect(() => {
    refusedTo.current = onRefused;
  }, [onRefused]);
  const watched = useMemo(() => {
    const seen = (now: boolean) => refusedTo.current?.(now);
    const wrapped: typeof built = Object.create(built);
    wrapped.query = (query, options) =>
      built.query(query, options).then(
        (page) => {
          seen(false);
          return page;
        },
        (err: unknown) => {
          seen(err instanceof SourceError && (err.status === 401 || err.status === 403));
          throw err;
        },
      );
    return wrapped;
  }, [built]);

  const [layout, setLayout] = useState<Layout>(
    () => readLayout(project, config.type) ?? { shown: {}, sort: null },
  );
  const onStateChange = useCallback(
    (next: GridState) => {
      const kept: Layout = { shown: next.shown, sort: next.sort };
      setLayout(kept);
      writeLayout(project, config.type, kept);
      onGridState?.(next);
    },
    [project, config.type, onGridState],
  );
  const state = useMemo(() => (view ? { ...layout, ...view } : layout), [layout, view]);
  const [columns, setColumns] = useState<ColumnLayout>(() => readColumns(project, config.type));
  const onColumns = useCallback(
    (next: ColumnLayout) => {
      setColumns(next);
      writeColumns(project, config.type, next);
    },
    [project, config.type],
  );

  return (
    <EntityGrid
      // An enum value links to the model term it stands for, where that is a web address (G-R2GRID2-019).
      termHref={(meaning) => meaning}
      // The project's own basemap (AP-67) under the grid's map, so an area is drawn on streets.
      basemap={`/api/v1/projects/${encodeURIComponent(project)}/basemap/default/style.json`}
      {...rest}
      config={config}
      source={watched}
      labels={labels}
      // Numbers and dates read in the person's language (T-4827).
      locale={i18n.language}
      // The map's own strings. No `mapEngine`: `GeoEditor`'s own default imports Terra Draw when a
      // map loads, so a page whose config asks for no map never downloads the library at all.
      mapLabels={mapLabels}
      geoLabels={geoLabels}
      // A private dataset read without a session offers the Portal's sign-in, back to this page (T-6376).
      onSignIn={rest.onSignIn ?? (signIn ? () => signIn() : undefined)}
      // Only the layout is controlled from here: the offset, the filters and the edits stay the
      // grid's own, so nothing a person is in the middle of survives a reload it should not.
      state={state}
      onStateChange={onStateChange}
      layout={rest.layout ?? columns}
      onLayout={rest.onLayout ?? onColumns}
    />
  );
}
