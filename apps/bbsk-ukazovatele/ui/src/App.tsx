/**
 * The indicators of the region and of the city, side by side (T-2308, AP-07, UI-15, UI-30).
 *
 * One section per publisher, because the whole point of the screen is that these are two bodies
 * and not one dataset: the region's ~600 000 inhabitants and the city's ~72 000 answer the same
 * question with numbers eight times apart, so every card names its territory and every section
 * names its publisher. An indicator both bodies published would appear once under each, never
 * merged.
 *
 * The two sections load independently. One publisher's endpoint being down leaves the other's
 * cards on screen and turns its own section into a sentence saying what failed — a dashboard
 * that blanks because one of two sources is unreachable tells a reader less than nothing.
 *
 * It sits in the SDK's shell (SDK-39). A card's territory, a district's bar and a district on the
 * map open that indicator in the shell's entity panel (SDK-40), read fresh through its body's
 * endpoint. The indicators are their pipelines' (T-2307), so the App writes none of them: the
 * panel links each to the Portal (README).
 */
import { useEffect, useState } from "react";
import type { CSSProperties } from "react";
import {
  AppShell,
  currentTokens,
  Empty,
  endpointSource,
  Grid,
  Loading,
  Page,
  Problem,
  transportFor,
  useClient,
  useEntitySelection,
} from "@joinedcontext/sdk";
import type { DesignTokens, JcEndpoint, RichRow } from "@joinedcontext/sdk";
import { DistrictChart } from "./DistrictChart";
import { DistrictMap } from "./DistrictMap";
import { choropleth, districtsOf, rangeOf } from "./districts";
import type { District } from "./districts";
import { byKey, districtBars, isWhole, toIndicator, unitAsContracted } from "./indicators";
import type { Body, Indicator, State } from "./indicators";
import { stringsFor } from "./locales";
import type { Strings } from "./locales";
import { linePoints, RAW_SPACE, SERIES_QUERY, seriesKey, seriesOf } from "./trends";
import type { Point } from "./trends";

/** The space each body publishes its indicators in (`Development/10` §2). */
const SPACE_OF: Record<Body, string> = {
  bbsk: "bbsk-kpi",
  banskabystrica: "banskabystrica-kpi",
};

const BODIES: Body[] = ["bbsk", "banskabystrica"];

/**
 * The register space whose public endpoint serves the district outlines, the App's one further
 * space (AP-04, T-2933); only the region's indicators have districts.
 */
const REGISTER_SPACE = "bbsk-registre";

/**
 * Each body's colour, from the design tokens (AP-123): the region takes the accent, the city the
 * palette's second colour, so the two publishers read apart at a glance and a theme recolours both.
 */
export function bodyColor(body: Body, tokens: DesignTokens = currentTokens()): string {
  return body === "bbsk" ? tokens.color.accent : (tokens.chart.palette[1] ?? tokens.color.accent);
}

/** One page is enough for the 31 indicators the contract declares, and caps a hostile answer. */
const LIMIT = 200;

/** What a card, a bar or a district opens: the indicator, through its body's endpoint. */
type Open = (indicator: Indicator) => void;

type Load =
  | { status: "loading" }
  | { status: "ready"; indicators: Indicator[] }
  | { status: "failed"; reason: string }
  | { status: "unreachable" };

export default function App() {
  const { config } = useClient();
  const s = stringsFor(config.language);
  return <AppShell title={s.title} pages={[{ id: "indicators", label: s.title, render: () => <Indicators /> }]} language={config.language} />;
}

function Indicators() {
  const { config } = useClient();
  const s = stringsFor(config.language);
  const trends = useTrends();

  return (
    <Page>
      <p className="subtitle">{s.subtitle}</p>
      {trends.status === "failed" && <Problem error={new Error(`${s.trendUnavailable}: ${trends.reason}`)} />}
      {BODIES.map((body) => (
        <BodySection key={body} body={body} s={s} series={body === "bbsk" && trends.status === "ready" ? trends.series : undefined} />
      ))}
    </Page>
  );
}

/**
 * Reads one body's indicators through the endpoint the served configuration names for its space.
 *
 * The endpoint is found by space and never by position, so the application behaves the same
 * whether its own endpoint or the shared one is listed first, and a configuration that names only
 * one leaves the other body honestly marked unreachable instead of showing the wrong body's rows
 * under its heading.
 */
function useIndicators(body: Body): Load {
  const { config } = useClient();
  const [load, setLoad] = useState<Load>({ status: "loading" });

  const endpoints: JcEndpoint[] = config.endpoints ?? [];
  const endpoint = endpoints.find((candidate) => candidate.space === SPACE_OF[body]);
  const slug = endpoint?.slug;
  const language = config.language;

  useEffect(() => {
    if (!slug) {
      setLoad({ status: "unreachable" });
      return;
    }
    let live = true;
    setLoad({ status: "loading" });
    endpointSource(slug, transportFor(config), language)
      .query({ type: "KeyPerformanceIndicator" }, { offset: 0, limit: LIMIT })
      .then((page) => {
        if (!live) return;
        setLoad({ status: "ready", indicators: indicatorsOf(page.rows, body) });
      })
      .catch((cause: unknown) => {
        if (!live) return;
        setLoad({ status: "failed", reason: reasonOf(cause) });
      });
    return () => {
      live = false;
    };
    // `config` is the document the Portal served once; the slug and the language are what change.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [slug, language, body]);

  return load;
}

/**
 * The rows of one endpoint, as indicators of the body that endpoint belongs to.
 *
 * An entity whose own id says it belongs to the other body is dropped rather than shown: an
 * endpoint is only allowed to serve its own space, so such a row means something is wrong
 * upstream, and putting it under this heading would be the exact confusion this screen exists to
 * prevent.
 */
export function indicatorsOf(rows: RichRow[], body: Body): Indicator[] {
  return rows
    .map(toIndicator)
    .filter((indicator): indicator is Indicator => indicator !== null && indicator.body === body);
}

type Districts =
  | { status: "none" }
  | { status: "loading" }
  | { status: "ready"; districts: District[] }
  | { status: "failed"; reason: string };

/**
 * The district outlines, read through the register's endpoint when the served configuration names
 * one and `wanted` says the body has districts. Slovak whatever the page's language: the join key
 * is the Slovak name (`districts.ts`).
 */
function useDistricts(wanted: boolean): Districts {
  const { config } = useClient();
  const [districts, setDistricts] = useState<Districts>({ status: "none" });
  const slug = (config.endpoints ?? []).find((candidate) => candidate.space === REGISTER_SPACE)?.slug;

  useEffect(() => {
    if (!wanted || !slug) {
      setDistricts({ status: "none" });
      return;
    }
    let live = true;
    setDistricts({ status: "loading" });
    endpointSource(slug, transportFor(config), "sk")
      .query(
        { type: "AdministrativeArea", q: 'divisionLevel=="district"', attrs: ["name", "divisionLevel", "location"] },
        { offset: 0, limit: LIMIT },
      )
      .then((page) => {
        if (live) setDistricts({ status: "ready", districts: districtsOf(page.rows) });
      })
      .catch((cause: unknown) => {
        if (live) setDistricts({ status: "failed", reason: reasonOf(cause) });
      });
    return () => {
      live = false;
    };
    // As in `useIndicators`: the served configuration is read once, the slug is what changes.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [wanted, slug]);

  return districts;
}

type Trends = { status: "none" } | { status: "ready"; series: Map<string, Point[]> } | { status: "failed"; reason: string };

/** The most rows the trend read takes: 14 territories × ~35 years of two cells. */
const RAW_MOST = 4000;
const RAW_PAGE = 1000;

/**
 * The region's yearly rows, read through the public endpoint of `bbsk-kraj` when the served
 * configuration names one (AP-04, T-2934); without it every card keeps its one window.
 */
function useTrends(): Trends {
  const { config } = useClient();
  const [trends, setTrends] = useState<Trends>({ status: "none" });
  const slug = (config.endpoints ?? []).find((candidate) => candidate.space === RAW_SPACE)?.slug;

  useEffect(() => {
    if (!slug) {
      setTrends({ status: "none" });
      return;
    }
    let live = true;
    const source = endpointSource(slug, transportFor(config), "sk");
    (async () => {
      const rows: RichRow[] = [];
      for (let offset = 0; offset < RAW_MOST; offset += RAW_PAGE) {
        const page = await source.query(
          { type: "KeyPerformanceIndicator", q: SERIES_QUERY, attrs: ["dataSet", "indicator", "refArea", "refPeriod", "dimensionKey", "kpiValue"] },
          { offset, limit: RAW_PAGE },
        );
        rows.push(...page.rows);
        if (page.rows.length < RAW_PAGE) break;
      }
      return rows;
    })()
      .then((rows) => {
        if (live) setTrends({ status: "ready", series: seriesOf(rows) });
      })
      .catch((cause: unknown) => {
        if (live) setTrends({ status: "failed", reason: reasonOf(cause) });
      });
    return () => {
      live = false;
    };
    // As in `useIndicators`: the served configuration is read once, the slug is what changes.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [slug]);

  return trends;
}

/** A refusal's words (a `SourceError` is an `Error`), else whatever was thrown. */
function reasonOf(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}

function BodySection({ body, s, series }: { body: Body; s: Strings; series?: Map<string, Point[]> }) {
  const load = useIndicators(body);
  const { config } = useClient();
  const { select } = useEntitySelection();
  const endpoint = (config.endpoints ?? []).find((candidate) => candidate.space === SPACE_OF[body])?.name;
  const open: Open = (indicator) => select({ id: indicator.id, type: "KeyPerformanceIndicator", endpoint });
  const districts = useDistricts(body === "bbsk");
  const [picked, setPicked] = useState<string | null>(null);
  const [marked, setMarked] = useState<string | null>(null);
  const headingId = `body-${body}`;
  const groups = load.status === "ready" ? byKey(load.indicators) : [];
  // The indicators a map can colour: those with at least two measured districts, as for a chart.
  const mappable = groups.filter((group) => districtBars(group.rows).length >= 2);
  const mapped = mappable.find((group) => group.key === picked) ?? mappable[0];
  const shapes =
    mapped && districts.status === "ready" && districts.districts.length > 0
      ? choropleth(districts.districts, districtBars(mapped.rows))
      : null;
  const mapId = `map-${body}`;

  return (
    <section
      className={`body body-${body}`}
      aria-labelledby={headingId}
      style={{ "--body": bodyColor(body) } as CSSProperties}
    >
      <h2 id={headingId}>{s.body[body]}</h2>
      <p className="note">{s.bodyNote[body]}</p>
      {load.status === "loading" && <Loading label={s.loading} />}
      {load.status === "unreachable" && <Empty>{s.noEndpoint}</Empty>}
      {load.status === "failed" && <Problem error={new Error(`${s.unavailable} ${s.unavailableWhy}: ${load.reason}`)} />}
      {load.status === "ready" && load.indicators.length === 0 && <Empty>{s.empty}</Empty>}
      {districts.status === "failed" && mapped && <Problem error={new Error(`${s.mapUnavailable}: ${districts.reason}`)} />}
      {mapped && shapes && (
        <section className="map-panel" aria-labelledby={mapId}>
          <h3 id={mapId}>{s.map}</h3>
          <label className="map-pick">
            {s.mapIndicator}{" "}
            <select
              value={mapped.key}
              onChange={(event) => {
                setPicked(event.target.value);
                setMarked(null);
              }}
            >
              {mappable.map((group) => (
                <option key={group.key} value={group.key}>
                  {s.indicator[group.key]?.title ?? group.key}
                </option>
              ))}
            </select>
          </label>
          <p className="note">{s.mapHint}</p>
          <DistrictMap
            features={shapes.features}
            ramp={shapes.ramp}
            title={s.indicator[mapped.key]?.title ?? mapped.key}
            unit={unitOf(mapped.key, mapped.rows, s)}
            marked={marked}
            onMark={(territory) => {
              setMarked(territory);
              const indicator = mapped.rows.find((row) => row.territory === territory);
              if (indicator) open(indicator);
            }}
            basemap={config.basemap}
            s={s}
          />
        </section>
      )}
      {groups.map((group) => (
        <Group
          key={group.key}
          groupKey={group.key}
          rows={group.rows}
          s={s}
          body={body}
          series={series}
          onOpen={open}
          marked={shapes && group.key === mapped?.key ? marked : undefined}
          onMark={shapes && group.key === mapped?.key ? setMarked : undefined}
          // Under a map every chart takes its scale: the mapped indicator the map's own range, the
          // others their range, which is the map's the moment that indicator is picked (T-3005).
          ramp={shapes && (group.key === mapped?.key ? shapes.ramp : rangeOf(districtBars(group.rows).map((bar) => bar.value)))}
        />
      ))}
    </section>
  );
}

/**
 * The written unit of an indicator's districts, only where every one carries the contracted code;
 * otherwise no unit beside a bar is truer than the wrong one, and each card still shows its own.
 */
function unitOf(groupKey: string, rows: Indicator[], s: Strings): string {
  return rows.every((row) => row.value === null || unitAsContracted(row)) ? (s.indicator[groupKey]?.unit ?? "") : "";
}

function Group({
  groupKey,
  rows,
  s,
  body,
  marked,
  onMark,
  ramp,
  series,
  onOpen,
}: {
  groupKey: string;
  rows: Indicator[];
  s: Strings;
  body: Body;
  series?: Map<string, Point[]>;
  onOpen: Open;
  marked?: string | null;
  onMark?: (territory: string) => void;
  ramp?: [number, number] | null;
}) {
  const headingId = `group-${body}-${groupKey}`;
  const title = s.indicator[groupKey]?.title ?? groupKey;
  const bars = districtBars(rows);
  const unit = unitOf(groupKey, rows, s);
  return (
    <section className="group" aria-labelledby={headingId}>
      <h3 id={headingId}>{title}</h3>
      {bars.length >= 2 && (
        <DistrictChart
          id={`${body}-${groupKey}`}
          title={title}
          unit={unit}
          bars={bars}
          s={s}
          marked={marked}
          // A bar opens its district's indicator; under a map it marks the district there as well.
          onMark={(territory) => {
            onMark?.(territory);
            const indicator = rows.find((row) => row.territory === territory);
            if (indicator) onOpen(indicator);
          }}
          ramp={ramp}
        />
      )}
      <Grid columns={4}>
        {rows.map((indicator) => (
          <IndicatorCard
            key={indicator.id}
            indicator={indicator}
            s={s}
            points={series?.get(seriesKey(indicator.key, indicator.territory))}
            onOpen={() => onOpen(indicator)}
          />
        ))}
      </Grid>
    </section>
  );
}

/**
 * One indicator. Everything a reader needs to place the number is on the card and not in a
 * legend: whose it is, what it is measured in, the window it covers and when it was computed.
 */
function IndicatorCard({ indicator, s, points, onOpen }: { indicator: Indicator; s: Strings; points?: Point[]; onOpen: () => void }) {
  const territory = s.territory[indicator.territory] ?? indicator.territory;
  const unit = s.indicator[indicator.key]?.unit;
  const contracted = unitAsContracted(indicator);
  const headingId = `card-${indicator.id}`;

  return (
    <article className={isWhole(indicator.territory) ? "card whole" : "card"} aria-labelledby={headingId}>
      <h4 id={headingId} className="territory">
        {/* The territory opens the indicator in the entity panel: the card's one way in. */}
        <button type="button" className="card-open" onClick={onOpen}>
          {territory}
        </button>
      </h4>

      {indicator.value === null ? (
        <p className="value not-measured">
          <span className="reading">{s.notMeasured}</span>
          <span className="why">{s.notMeasuredWhy}</span>
        </p>
      ) : (
        <p className="value">
          <span className="reading">
            {number(indicator.value, s)}
            {/* The unit is read with the number, not left to a column heading. */}
            <span className="unit">
              {" "}
              {contracted && unit ? unit : (indicator.unitCode ?? "")}
            </span>
          </span>
          {!contracted && indicator.unitCode && (
            <span className="why">
              {s.rawUnit}: {indicator.unitCode}
            </span>
          )}
        </p>
      )}

      {indicator.state && <StateBadge state={indicator.state} s={s} />}

      {points && <Trend points={points} s={s} />}

      <details className="more">
        <summary>{s.details}</summary>
        <dl className="facts">
        {indicator.period && (
          <>
            <dt>{s.window}</dt>
            <dd>
              <time dateTime={indicator.period.start}>{day(indicator.period.start, s)}</time> –{" "}
              <time dateTime={indicator.period.end}>{day(indicator.period.end, s)}</time>
            </dd>
          </>
        )}
        {indicator.updatedAt && (
          <>
            <dt>{s.computedAt}</dt>
            <dd>
              <time dateTime={indicator.updatedAt}>{moment(indicator.updatedAt, s)}</time>
            </dd>
          </>
        )}
        {indicator.formula && (
          <>
            <dt>{s.formula}</dt>
            <dd className="formula">{indicator.formula}</dd>
          </>
        )}
        </dl>
      </details>
    </article>
  );
}

const TREND_WIDTH = 120;
const TREND_HEIGHT = 32;

/**
 * The indicator's published years as a line, drawn only where there are two or more (T-2934): one
 * point is no trend. The line is a picture; its years and end values are said in words beside it.
 */
function Trend({ points, s }: { points: Point[]; s: Strings }) {
  const line = linePoints(points, TREND_WIDTH, TREND_HEIGHT);
  if (!line) return null;
  const first = points[0];
  const last = points[points.length - 1];
  return (
    <p className="trend">
      <svg viewBox={`-2 -2 ${TREND_WIDTH + 4} ${TREND_HEIGHT + 4}`} width={TREND_WIDTH} height={TREND_HEIGHT} aria-hidden="true">
        <polyline points={line} fill="none" stroke="var(--body)" strokeWidth="2" vectorEffect="non-scaling-stroke" />
      </svg>
      <span className="trend-words">{s.trend(first.period, last.period, number(first.value, s), number(last.value, s))}</span>
    </p>
  );
}

/** The shape beside the word, so the state is never carried by colour alone (UI-30). */
const SHAPE: Record<State, string> = { green: "●", amber: "▲", red: "■" };

function StateBadge({ state, s }: { state: State; s: Strings }) {
  return (
    <p className={`state state-${state}`}>
      <span aria-hidden="true" className="shape">
        {SHAPE[state]}
      </span>{" "}
      <span>
        {s.stateOf}: {s.state[state]}
      </span>
    </p>
  );
}

function number(value: number, s: Strings): string {
  return new Intl.NumberFormat(s.locale, { maximumFractionDigits: 2 }).format(value);
}

function day(iso: string, s: Strings): string {
  const at = new Date(iso);
  return Number.isNaN(at.getTime()) ? iso : new Intl.DateTimeFormat(s.locale, { dateStyle: "medium" }).format(at);
}

function moment(iso: string, s: Strings): string {
  const at = new Date(iso);
  return Number.isNaN(at.getTime())
    ? iso
    : new Intl.DateTimeFormat(s.locale, { dateStyle: "medium", timeStyle: "short" }).format(at);
}
