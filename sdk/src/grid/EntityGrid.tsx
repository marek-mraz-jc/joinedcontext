import React, { useCallback, useEffect, useRef, useState } from "react";
import type { RichRow, RichCell } from "./model";
import type { ColumnLayout, GridLabels, GridRefusal, UseEntityGridOptions, VisibleColumn } from "./useEntityGrid";
import { DEFAULT_LABELS } from "./useEntityGrid";
import { targetName } from "./cells";
import type { FilterColumn } from "./filters";
import type { GridMapLabels } from "./GridMap";
import type { DrawEngine, GeoLabels } from "../geo/GeoEditor";
import type { GeometryType } from "../geo/validate";
import type { AccessAction, AccessExplanation } from "./CellInspector";
import type { AttributeAccess } from "./source";
import type { RowDetailTab } from "./RowDetail";
import type { ValueRule } from "./rules";
import type { TypeChoice } from "./GridFilterRow";
import { GridCellPanels, GridDetail } from "./GridDetail";
import { GridMapPanel } from "./GridMapPanel";
import { GridNotices, GridStatus } from "./GridNotices";
import { GridPager } from "./GridPager";
import { GridQueryBox } from "./GridQueryBox";
import { GridReview } from "./GridReview";
import type { GridGroup } from "./GridRows";
import { GridTable } from "./GridTable";
import { useGridView } from "./useGridView";
import type { GridView } from "./useGridView";
import "./grid.css";

export { pageNumbers } from "./GridPager";
export { moveTo } from "./GridHead";
export type { GridGroup } from "./GridRows";
export type { TypeChoice } from "./GridFilterRow";

export interface EntityGridProps extends UseEntityGridOptions {
  /**
   * How one column's cells are drawn, by attribute name, by cell kind, or `id` for the pinned
   * identifier column — which is how a host makes a row open something of its own (a detail pane,
   * a row action) without a second table of its own.
   */
  renderers?: Record<string, (cell: RichCell | RichCell[] | undefined, row: RichRow) => React.ReactNode>;
  /**
   * Opens a relationship's target in the host's own place. Without it the grid opens the target
   * itself, in the same grid, with a breadcrumb back to the rows it came from (G-R2GRID2-017).
   */
  onOpenRelationship?: (urn: string) => void;
  /**
   * Where an enum value's term reads (G-R2GRID2-019): the address of the model term `meaning`
   * names, or `undefined` for none. Only an http(s) address becomes a link.
   */
  termHref?: (meaning: string) => string | undefined;
  /**
   * The page of rows as the source answered it, with the offset it starts at, for a host that
   * exports or counts what is shown.
   */
  onRows?: (rows: RichRow[], offset: number) => void;
  /**
   * The query the person's filters ask the source for, the typed one or the filter row's, each
   * time it changes: what a host saves as a view's filter (API/01 §30).
   */
  onQuery?: (query: { q?: string; idPattern?: string }) => void;
  /**
   * What is different about a row or a column, and why, in the host's own words: a comparison marks
   * what the other side does not answer (T-1435). Each value is the sentence a person reads on it.
   */
  marks?: { rows?: Record<string, string>; columns?: Record<string, string> };
  /**
   * The map beside the rows, where `config.map.enabled` asked for one (UI-72). The drawing library
   * is injected the way `GeoEditor` takes it, so a host that never draws ships none of it, and the
   * geometries a type accepts come from the DataModel's range where the space has one.
   */
  mapEngine?: DrawEngine;
  mapAllowed?: readonly GeometryType[];
  mapLabels?: Partial<GridMapLabels>;
  geoLabels?: Partial<GeoLabels>;
  basemap?: string;
  /** What the host adds under a row's fields in its detail (T-3106). */
  detailExtra?: (row: RichRow) => React.ReactNode;
  /** The host's own tabs in a row's detail after Fields and History, such as its comments (T-4828). */
  detailTabs?: (row: RichRow) => RowDetailTab[];
  /**
   * The row whose detail is open, by id, for a host that keeps it in the address (`?row=<urn>`,
   * T-4828): `undefined` leaves it to the grid, `null` closes it. A row not on the loaded page is
   * read by its id and shown without editors until the grid loads it.
   */
  openRow?: string | null;
  onOpenRow?: (id: string | null) => void;
  /** The open row's tab and History attribute, for a host that keeps them in the address (T-5602). */
  detailPlace?: { tab?: string | null; attr?: string | null };
  onDetailPlace?: (place: { tab: string; attr: string }) => void;
  toolbar?: React.ReactNode;
  /** The bounds the "Draw area" action asks about; the host owns the map's viewport. */
  mapBounds?: () => [number, number, number, number] | null;
  empty?: React.ReactNode;
  /**
   * Where a private dataset read without a session sends the person (T-6376): the host's own
   * sign-in, which comes back to this page. Without it the signed-out state has no button.
   */
  onSignIn?: () => void;
  /**
   * Each attribute's rule as its model states it (range, bounds, pattern, required), by name: a
   * pending value that breaks it is marked at its cell and holds Apply back (T-3097).
   */
  rules?: Record<string, ValueRule>;
  /**
   * The tone a row is marked with and why, as a view's colour rules decide it (API/01 §30): drawn
   * as a swatch in the row's first cell that names the reason, so the colour is never the only
   * way the row says it.
   */
  rowTone?: (row: RichRow) => { tone: string; label: string } | undefined;
  /** Each new value reads the rows again where they are, keeping the scroll and the checked rows: a host's live change. */
  reloadKey?: number;
  className?: string;
  classNames?: Partial<Record<"root" | "table" | "header" | "row" | "cell" | "pager", string>>;
  /** The reader's language tag, for numbers and dates (`fi`, `en-GB`); the browser's without it. */
  locale?: string;
  /**
   * Every change the header menu makes to the order, widths or labels (T-4827), for a host that
   * keeps them in a saved view; without it the grid keeps them itself until it unmounts.
   */
  onLayout?: (next: ColumnLayout) => void;
  /** "Hide field" in the header menu, for a host that holds `hidden`; without it the grid hides it itself. */
  onHide?: (attr: string) => void;
  /**
   * The per-column filter row; `false` for a host that asks through a filter panel of its own and
   * hands the query in as `query` (T-4883).
   */
  filterRow?: boolean;
  /** The box under the rows that shows the asked query; `false` for a host that opens it from its own toolbar. */
  queryBox?: boolean;
  /** Every column the endpoint can be asked about, with its kind, for a host's own filter panel. */
  onFilterColumns?: (columns: FilterColumn[]) => void;
  /**
   * A leading column with the row's number, a checkbox and an expand button (T-4883), with the
   * names a screen reader hears; the checked rows are the host's (`selected`).
   */
  rowNumbers?: { selectRow: string; selectAll: string };
  /** The entity type the grid reads, picked in the Type column's filter cell; the host reads the new type (owner 2026-11-01). */
  typeChoice?: TypeChoice;
  /** Rows per page offered in the pager (with `paged`); the host sets `config.pageSize` from the pick. */
  pageSizes?: readonly number[];
  onPageSize?: (size: number) => void;
  selected?: readonly string[];
  onSelectedChange?: (ids: string[]) => void;
  /** Rows of one line (`small`), two or four (T-4883); without it a cell keeps the grid's own wrapping. */
  rowSize?: "small" | "medium" | "large";
  /** What a column's footer cell shows, such as the host's aggregation of the whole filtered set. */
  footer?: (column: VisibleColumn) => React.ReactNode;
  /** What a group's header shows after its name and value (T-5188), such as the group's count. */
  groupHeader?: (group: GridGroup) => React.ReactNode;
  /**
   * The person's decisions per attribute (T-5528), for a host that already read them; `null` when
   * they could not be read. Without it the grid asks its source's `access`, where it has one. An
   * attribute opens an editor only where its row says `write`.
   */
  access?: AttributeAccess[] | null;
  /** Names the Policy behind a prohibited attribute in the cell inspector (API/01 §58). */
  explainAccess?: (attribute: string, action: AccessAction) => Promise<AccessExplanation>;
  /** Where the inspector sends a person to ask for access. */
  accessRequestHref?: string;
}

/** One target opened from a relationship cell: the step of the breadcrumb back (G-R2GRID2-017). */
interface DrillStep {
  urn: string;
  type: string;
}

/**
 * The entity grid. A relationship target the host does not open itself opens in this same grid,
 * read by its id from the same source, so the target's own grants decide what shows; the
 * breadcrumb above it goes back to the rows it was opened from, which stay mounted and keep their
 * query, page and scroll.
 */
export function EntityGrid(props: EntityGridProps): React.JSX.Element {
  const [trail, setTrail] = useState<DrillStep[]>([]);
  const outerType = props.config.type;
  const [trailOf, setTrailOf] = useState(outerType);
  if (trailOf !== outerType) {
    // A new type in the host's picker starts from its own rows.
    setTrailOf(outerType);
    setTrail([]);
  }
  const host = useRef<HTMLDivElement>(null);
  const scroll = useRef<{ top: number; left: number }[]>([]);
  const drill = useCallback((urn: string, type: string) => {
    if (trail.length === 0) {
      scroll.current = [...(host.current?.querySelectorAll<HTMLElement>(".jc-grid-scroll") ?? [])].map((el) => ({ top: el.scrollTop, left: el.scrollLeft }));
    }
    setTrail((steps) => [...steps, { urn, type }]);
  }, [trail.length]);
  const back = useCallback((to: number) => setTrail((steps) => steps.slice(0, to)), []);
  useEffect(() => {
    if (trail.length !== 0 || scroll.current.length === 0) return;
    // Back on the first rows: where the person was when they opened the link.
    const saved = scroll.current;
    scroll.current = [];
    host.current?.querySelectorAll<HTMLElement>(".jc-grid-scroll").forEach((el, at) => {
      el.scrollTop = saved[at]?.top ?? 0;
      el.scrollLeft = saved[at]?.left ?? 0;
    });
  }, [trail.length]);
  if (props.onOpenRelationship) return <GridBody {...props} />;
  const top = trail.at(-1);
  const labels = { drillTrail: props.labels?.drillTrail ?? DEFAULT_LABELS.drillTrail, drillBack: props.labels?.drillBack ?? DEFAULT_LABELS.drillBack };
  return (
    <div ref={host}>
      {top && (
        <nav className="jc-grid-trail" aria-label={labels.drillTrail}>
          <button type="button" className="jc-grid-trail-back" onClick={() => back(trail.length - 1)}>
            {`← ${labels.drillBack}`}
          </button>
          <ol>
            <li>
              <button type="button" onClick={() => back(0)}>{outerType}</button>
            </li>
            {trail.map((step, at) => (
              <li key={`${step.urn}-${at}`}>
                {at === trail.length - 1 ? (
                  <span aria-current="page" title={step.urn}>{`${step.type} · ${targetName(step.urn).name}`}</span>
                ) : (
                  <button type="button" title={step.urn} onClick={() => back(at + 1)}>{`${step.type} · ${targetName(step.urn).name}`}</button>
                )}
              </li>
            ))}
          </ol>
        </nav>
      )}
      <div hidden={top !== undefined}>
        <GridBody {...props} onDrill={drill} />
      </div>
      {top && (
        // The target alone, read-only: its own type's columns, none of the host's per-type settings.
        <GridBody
          key={`${top.urn}-${trail.length}`}
          source={props.source}
          labels={props.labels}
          locale={props.locale}
          termHref={props.termHref}
          config={{ ...props.config, type: top.type, columns: [], mode: "view" }}
          query={{ ids: [top.urn] }}
          filterRow={false}
          queryBox={false}
          onDrill={drill}
        />
      )}
    </div>
  );
}

/**
 * The frame every part sits in: the grid's root element with its density, row size and map side,
 * and the designed refusal in place of them (T-6376): never an empty grid, no row of an earlier answer.
 */
export function GridFrame({ view, children }: { view: GridView; children: React.ReactNode }): React.JSX.Element {
  const { props, grid, labels, options, mapAttr, mapPosition, virtual, filtersOpen } = view;
  const { className, classNames, onSignIn, rowSize, rowNumbers } = props;
  if (grid.refusal) {
    return (
      <div className={`jc-grid${className ? ` ${className}` : ""}`} data-jc-sdk="grid">
        <GridRefusalState refusal={grid.refusal} labels={labels} onSignIn={onSignIn} detail={grid.refusalDetail} />
      </div>
    );
  }
  const rootClass = `jc-grid${className ? ` ${className}` : ""}${classNames?.root ? ` ${classNames.root}` : ""}${mapAttr ? ` jc-grid--map-${mapPosition}` : ""}${virtual ? " jc-grid--virtual" : ""}`;
  return (
    // `data-jc-sdk`: the grid's own controls are the SDK suite's to test, not each App's (T-3373).
    <div className={rootClass} data-density={options.config.density} data-row-size={rowSize} data-row-numbers={rowNumbers ? "" : undefined} data-jc-sdk="grid" data-filters={filtersOpen ? "open" : undefined}>
      {children}
    </div>
  );
}

function GridBody(props: EntityGridProps & { onDrill?: (urn: string, type: string) => void }): React.JSX.Element {
  const view = useGridView(props);
  const { toolbar } = props;
  return (
    <GridFrame view={view}>
      {toolbar && <div className="jc-grid-toolbar">{toolbar}</div>}
      <GridNotices view={view} />
      <GridTable view={view} />
      <GridMapPanel view={view} />
      <GridStatus view={view} />
      <GridDetail view={view} />
      <GridCellPanels view={view} />
      <GridReview view={view} />
      <GridQueryBox view={view} />
      <GridPager view={view} />
    </GridFrame>
  );
}

/** The signed-out or no-access state: a title, what to do, and the way to sign in where there is one. */
export function GridRefusalState({
  refusal,
  labels,
  onSignIn,
  detail,
}: {
  refusal: GridRefusal;
  /** The Endpoint's own sentence, for a refusal the person can act on with its publisher. */
  detail?: string | null;
  labels: Pick<GridLabels, "signedOutTitle" | "signedOutBody" | "signIn" | "noAccessTitle" | "noAccessBody">;
  onSignIn?: () => void;
}): React.JSX.Element {
  const signedOut = refusal === "signedOut";
  const titleId = React.useId();
  return (
    <section className="jc-grid-refusal" data-refusal={refusal} role="status" aria-labelledby={titleId}>
      <h3 id={titleId} className="jc-grid-refusal__title">
        {signedOut ? labels.signedOutTitle : labels.noAccessTitle}
      </h3>
      <p className="jc-grid-refusal__body">{signedOut ? labels.signedOutBody : labels.noAccessBody}</p>
      {detail && !signedOut ? <p className="jc-grid-refusal__detail">{detail}</p> : null}
      {signedOut && onSignIn ? (
        <button type="button" className="jc-grid-refusal__action" onClick={onSignIn}>
          {labels.signIn}
        </button>
      ) : null}
    </section>
  );
}
