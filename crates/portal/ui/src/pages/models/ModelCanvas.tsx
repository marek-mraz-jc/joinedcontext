/**
 * A model as a full-screen canvas (T-6094): the diagram fills the screen, an overlay shows which
 * classes and attributes an Endpoint serves or a Pipeline writes, a panel shows the picked class,
 * and tabs below split a model with several areas.
 *
 * Read-only: what a person moves is kept in this browser only, and "Edit" opens the model page's
 * form, where a change is a proposal behind propose rights. Every list here is the one the
 * Portal answered for the person, so an Endpoint they may not read never appears.
 */
import { useId, useMemo, useRef, useState } from "react";
import type { JSX } from "react";
import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { localized } from "../../api/manifest";
import { usePermissions } from "../../api/permissions";
import { RecordLink } from "../../components/RecordLink";
import {
  Badge,
  Button,
  EmptyState,
  Icon,
  PageHeader,
  Select,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeaderCell,
  TableRow,
  Tabs,
} from "../../components/ui";
import { ResourcePageFailed } from "../../components/ui/PageState";
import { spaceOf } from "../spaces/spaceFacts";
import {
  LAYOUT_STORAGE,
  MAIN_AREA,
  PERSONAL_AREA,
  areaTabs,
  areasOf,
  readPositions,
  servedBy,
  servingCounts,
  writePositions,
  writtenBy,
} from "./canvasOverlay";
import type { Positions, Served } from "./canvasOverlay";
import { LinkmlGraphView } from "./LinkmlGraphView";
import { classSlots, graphData, parseModel, withImports } from "./linkml";
import type { LinkmlClass, LinkmlModel, LinkmlSlot } from "./linkml";
import { useSourceOf } from "./ModelPage";
import { spaceOfModel } from "./modelUsage";
import { useImportSources } from "./ModelViews";
import { useProjectList } from "./ModelsList";

/** Which model a kept layout belongs to, inside the one browser key. */
export function layoutKey(project: string, name: string): string {
  return `${project}/${name}`;
}

function loadPositions(key: string): Positions {
  try {
    return readPositions(window.localStorage.getItem(LAYOUT_STORAGE), key);
  } catch {
    return {};
  }
}

function savePositions(key: string, positions: Positions): void {
  try {
    window.localStorage.setItem(LAYOUT_STORAGE, writePositions(window.localStorage.getItem(LAYOUT_STORAGE), key, positions));
  } catch {
    // A browser that keeps nothing (private window, blocked storage) still moves the box for now.
  }
}

/** A class's own fields, then the ones it inherits along `is_a`, each once, marked as inherited. */
export function fieldsOf(model: LinkmlModel, klass: LinkmlClass): { slot: LinkmlSlot; inherited?: string }[] {
  const byName = new Map(model.classes.map((one) => [one.name, one]));
  const seen = new Set<string>();
  const rows: { slot: LinkmlSlot; inherited?: string }[] = [];
  let at: LinkmlClass | undefined = klass;
  const walked = new Set<string>();
  while (at && !walked.has(at.name)) {
    walked.add(at.name);
    for (const slot of classSlots(model, at)) {
      if (seen.has(slot.name)) continue;
      seen.add(slot.name);
      rows.push(at === klass ? { slot } : { slot, inherited: at.name });
    }
    at = at.is_a === undefined ? undefined : byName.get(at.is_a);
  }
  return rows;
}

type Overlay = "" | `endpoint:${string}` | `pipeline:${string}`;

export function ModelCanvas({ project, name }: { project: string; name: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const { can } = usePermissions(project);
  const models = useProjectList(project, "datamodels");
  const endpoints = useProjectList(project, "endpoints");
  const policies = useProjectList(project, "policies");
  const spaces = useProjectList(project, "spaces");
  const pipelines = useProjectList(project, "pipelines");
  const model = models.data?.find((one) => one.metadata.name === name);
  const source = useSourceOf(project, model);
  const imports = useImportSources(project, name, source.data ?? "");
  const key = layoutKey(project, name);
  const [positions, setPositions] = useState<Positions>(() => loadPositions(key));
  const [overlay, setOverlay] = useState<Overlay>("");
  const [selected, setSelected] = useState<string>();
  const [area, setArea] = useState<string>(MAIN_AREA);
  const [diagramOnPhone, setDiagramOnPhone] = useState(false);
  const hintId = useId();
  const { details, diagram, open, backToDiagram } = useDetailsInView(setSelected);

  const parsed = useMemo(() => {
    const own = parseModel(source.data ?? "");
    const others = Object.fromEntries(Object.entries(imports).map(([one, text]) => [one, parseModel(text)]));
    const whole = withImports(own, others);
    return { model: whole, ...graphData(whole) };
  }, [source.data, imports]);
  const space = model ? spaceOfModel(model, spaces.data) : undefined;
  const ofSpace = useMemo(
    () => (endpoints.data ?? []).filter((endpoint) => space !== undefined && spaceOf(endpoint) === space),
    [endpoints.data, space],
  );
  const served = useMemo(
    () => new Map(ofSpace.map((endpoint) => [endpoint.metadata.name, servedBy(endpoint, project, policies.data ?? [], spaces.data ?? [], parsed.nodes)])),
    [ofSpace, project, policies.data, spaces.data, parsed.nodes],
  );
  const counts = useMemo(() => servingCounts([...served.values()]), [served]);
  const writers = useMemo(() => writtenBy(pipelines.data ?? [], parsed.nodes), [pipelines.data, parsed.nodes]);
  const writerNames = useMemo(() => [...new Set([...writers.values()].flat())].sort(), [writers]);
  const subsets = useMemo(
    () => new Map(parsed.model.classes.flatMap((klass) => (klass.in_subset?.[0] ? [[klass.name, klass.in_subset[0]] as const] : []))),
    [parsed.model],
  );
  const areas = useMemo(() => areasOf(parsed.nodes, parsed.edges, subsets), [parsed.nodes, parsed.edges, subsets]);
  const tabs = areaTabs(areas);
  const shownArea = tabs.includes(area) ? area : MAIN_AREA;
  const only = useMemo(
    () => (tabs.length === 0 ? undefined : new Set([...areas].filter(([, one]) => one === shownArea).map(([box]) => box))),
    [tabs.length, areas, shownArea],
  );

  const highlight: Served | undefined = useMemo(() => {
    if (overlay.startsWith("endpoint:")) return served.get(overlay.slice("endpoint:".length)) ?? new Map();
    if (overlay.startsWith("pipeline:")) {
      const pipeline = overlay.slice("pipeline:".length);
      const rows = new Map(parsed.nodes.map((node) => [node.name, (node.rows ?? []).map((row) => row.name)]));
      return new Map([...writers].filter(([, names]) => names.includes(pipeline)).map(([klass]) => [klass, new Set(rows.get(klass) ?? [])]));
    }
    return undefined;
  }, [overlay, served, writers, parsed.nodes]);

  const move = (box: string, at: { x: number; y: number }) =>
    setPositions((current) => {
      const next = { ...current, [box]: { x: Math.round(at.x), y: Math.round(at.y) } };
      savePositions(key, next);
      return next;
    });
  const resetLayout = () => {
    savePositions(key, {});
    setPositions({});
  };

  const back = (
    <Link to="/projects/$project/models/$name" params={{ project, name }} className="focus-ring inline-flex items-center gap-1 rounded text-body text-primary-soft-fg">
      <Icon name="chevronLeft" className="size-4" />
      {t("models.canvas.back", { name })}
    </Link>
  );
  if (models.isError || source.isError) {
    return (
      <main className="p-4">
        <ResourcePageFailed
          title={t("models.canvas.title", { name })}
          description={t("models.canvas.lead")}
          error={models.error ?? source.error}
          onRetry={() => void (models.isError ? models.refetch() : source.refetch())}
          back={back}
        />
      </main>
    );
  }
  if (models.data && !model) {
    return (
      <main className="flex flex-col gap-4 p-4">
        <PageHeader title={t("models.canvas.title", { name })} description={t("models.canvas.lead")} />
        <EmptyState icon="models" title={t("models.page.notFound", { name })} description={t("models.page.notFoundLead")} action={back} />
      </main>
    );
  }
  const classes = parsed.model.classes.filter((klass) => parsed.nodes.some((node) => node.kind === "class" && node.name === klass.name));
  const picked = classes.find((klass) => klass.name === selected);
  const overlaySummary = highlight
    ? t("models.canvas.overlayStatus", { count: highlight.size, total: classes.length })
    : "";
  const mayPropose = can("DataModel", "propose");

  return (
    <main
      aria-labelledby="model-canvas-title"
      className="fixed inset-0 z-40 flex flex-col bg-surface pb-[env(safe-area-inset-bottom,0px)] pt-[env(safe-area-inset-top,0px)] max-sm:overflow-auto"
    >
      <header className="flex flex-wrap items-start gap-x-6 gap-y-2 border-b border-border px-4 py-2">
        <div className="flex min-w-0 flex-col">
          {back}
          <h1 id="model-canvas-title" className="text-lg font-semibold [overflow-wrap:anywhere]">
            {t("models.canvas.title", { name })}
          </h1>
        </div>
        <div className="flex min-w-0 flex-col gap-1 text-caption text-fg-muted max-sm:w-full">
          <div className="relative flex items-center gap-1">
            <label htmlFor="model-canvas-overlay">{t("models.canvas.overlay")}</label>
            {/* The how-to is a "?" beside the label, not a paragraph under the pick (T-6164): the header keeps
                one line. The text stays in the document for the select's aria-describedby. An empty pick
                keeps its next step in sight below. */}
            {ofSpace.length > 0 ? (
              <OverlayHelp id={hintId} label={t("models.canvas.overlayHelp")} hint={t("models.canvas.overlayHint")} />
            ) : null}
          </div>
          <div className="w-full sm:w-72">
          <Select
            id="model-canvas-overlay"
            value={overlay}
            aria-describedby={ofSpace.length === 0 ? "model-canvas-overlay-hint" : hintId}
            onChange={(event) => setOverlay(event.target.value as Overlay)}
          >
            <option value="">{t("models.canvas.overlayNone")}</option>
            <optgroup label={t("models.canvas.endpoints")}>
              {ofSpace.length === 0 ? (
                <option disabled value="-">
                  {t("models.canvas.noEndpoints")}
                </option>
              ) : (
                ofSpace.map((endpoint) => (
                  <option key={endpoint.metadata.name} value={`endpoint:${endpoint.metadata.name}`}>
                    {`${localized(endpoint.metadata.title, i18n.language, endpoint.metadata.name)} (${endpoint.metadata.name})`}
                  </option>
                ))
              )}
            </optgroup>
            {writerNames.length > 0 ? (
              <optgroup label={t("models.canvas.pipelines")}>
                {writerNames.map((pipeline) => (
                  <option key={pipeline} value={`pipeline:${pipeline}`}>
                    {pipeline}
                  </option>
                ))}
              </optgroup>
            ) : null}
          </Select>
          </div>
          {ofSpace.length === 0 ? (
            <span id="model-canvas-overlay-hint" className="max-w-prose">
              {t("models.canvas.noEndpointsHint")}
            </span>
          ) : null}
          {/* Under the pick it answers, so the header keeps one top line and no ragged row (T-6094 UI judge). */}
          <p role="status" aria-live="polite" className="min-h-5 font-medium text-fg">
            {overlaySummary}
          </p>
        </div>
        <div className="flex flex-wrap items-center gap-2 sm:pt-5">
          <Button
            size="sm"
            variant="secondary"
            className="max-sm:hidden"
            onClick={resetLayout}
            disabled={Object.keys(positions).length === 0}
            disabledReason={t("models.canvas.resetNothing")}
          >
            {t("models.canvas.reset")}
          </Button>
          <Button size="sm" variant="secondary" className="sm:hidden" aria-pressed={diagramOnPhone} onClick={() => setDiagramOnPhone(!diagramOnPhone)}>
            {diagramOnPhone ? t("models.canvas.showList") : t("models.canvas.showDiagram")}
          </Button>
        </div>
      </header>

      <div className="flex min-h-0 flex-1 flex-col max-sm:flex-none lg:flex-row">
        <section ref={diagram} aria-label={t("models.canvas.diagram")} className={`${diagramOnPhone ? "" : "max-sm:hidden"} flex min-h-0 min-w-0 flex-1 flex-col p-2 max-sm:min-h-160`}>
          {source.data === undefined ? (
            <p role="status" className="text-body text-fg-muted">
              {t("models.source.loading")}
            </p>
          ) : (
            <LinkmlGraphView
              canvas
              source={source.data}
              imports={imports}
              positions={positions}
              onMove={diagramOnPhone ? undefined : move}
              highlight={highlight}
              badges={counts}
              areas={areas}
              only={only}
              selected={selected}
              onOpenClass={open}
            />
          )}
          <p className="pt-1 text-caption text-fg-muted max-sm:hidden">{t("models.canvas.moveHint")}</p>
        </section>

        <ClassList
          classes={classes}
          areas={areas}
          counts={counts}
          highlight={highlight}
          pick={overlay.split(":")[1]}
          selected={selected}
          onSelect={open}
          hidden={diagramOnPhone}
        />

        <aside
          aria-labelledby="model-canvas-details"
          className="flex min-h-0 flex-col gap-3 overflow-auto border-border p-4 max-lg:border-t max-sm:overflow-visible lg:w-88 lg:border-l"
        >
          <h2 id="model-canvas-details" ref={details} tabIndex={-1} className="focus-ring scroll-mt-2 rounded-sm text-body font-semibold">
            {picked ? localized(picked.title, i18n.language, picked.name) : t("models.canvas.details")}
          </h2>
          {picked && diagramOnPhone ? (
            // The press scrolled the drawing away: one press takes the phone back to it (T-6164 UI judge).
            <Button size="sm" variant="secondary" className="self-start sm:hidden" onClick={backToDiagram}>
              {t("models.canvas.backToDiagram")}
            </Button>
          ) : null}
          {picked ? (
            <ClassDetails
              project={project}
              name={name}
              model={parsed.model}
              klass={picked}
              area={areas.get(picked.name)}
              highlight={highlight?.get(picked.name)}
              servedBy={[...served].filter(([, one]) => one.has(picked.name)).map(([endpoint]) => endpoint)}
              writers={writers.get(picked.name) ?? []}
              mayPropose={mayPropose}
            />
          ) : (
            <p className="text-body text-fg-muted">{t("models.canvas.pickHint")}</p>
          )}
        </aside>
      </div>

      {tabs.length > 0 ? (
        <footer className={`border-t border-border px-4 ${diagramOnPhone ? "" : "max-sm:hidden"}`}>
          <Tabs
            id="model-canvas-areas"
            label={t("models.canvas.areas")}
            tabs={tabs.map((one) => ({ value: one, label: areaLabel(one, t) }))}
            value={shownArea}
            onChange={setArea}
          />
        </footer>
      ) : null}
    </main>
  );
}

function areaLabel(area: string | undefined, t: (key: string) => string): string {
  return area === undefined || area === MAIN_AREA ? t("models.canvas.areaMain") : area === PERSONAL_AREA ? t("models.canvas.areaPersonal") : area;
}

/** On a phone the classes are a list (the canvas one press away), each with its area and counts. */
function ClassList({
  classes,
  areas,
  counts,
  highlight,
  pick,
  selected,
  onSelect,
  hidden,
}: {
  classes: LinkmlClass[];
  areas: ReadonlyMap<string, string>;
  counts: ReadonlyMap<string, number>;
  highlight?: Served;
  /** The picked Endpoint's or Pipeline's name, when one is. */
  pick?: string;
  selected?: string;
  onSelect: (name: string) => void;
  hidden: boolean;
}): JSX.Element | null {
  const { t } = useTranslation();
  if (hidden) return null;
  return (
    <section aria-labelledby="model-canvas-list" className="flex flex-col gap-2 p-4 sm:hidden">
      <h2 id="model-canvas-list" className="text-body font-semibold">
        {t("models.canvas.list")}
      </h2>
      <ul className="flex flex-col gap-2">
        {classes.map((klass) => (
          <li key={klass.name}>
            <Button
              variant="secondary"
              aria-pressed={klass.name === selected}
              onClick={() => onSelect(klass.name)}
              className={`h-auto! w-full flex-wrap justify-start p-3 text-left ${klass.name === selected ? "border-primary" : ""} ${highlight && !highlight.has(klass.name) ? "opacity-50" : ""}`}
            >
              <span className="w-full min-w-0 font-mono text-body font-medium [overflow-wrap:anywhere]">{klass.name}</span>
              <Badge tone={areas.get(klass.name) === PERSONAL_AREA ? "danger" : "neutral"}>{areaLabel(areas.get(klass.name), t)}</Badge>
              <span className="text-caption text-fg-muted">
                {/* A faded card says why, not how many serve it: "Served by 1" read as a contradiction (T-6094 UI judge). */}
                {pick && highlight && !highlight.has(klass.name)
                  ? t("models.canvas.notReached", { name: pick })
                  : t("models.canvas.badge", { count: counts.get(klass.name) ?? 0 })}
              </span>
            </Button>
          </li>
        ))}
      </ul>
    </section>
  );
}

function ClassDetails({
  project,
  name,
  model,
  klass,
  area,
  highlight,
  servedBy: endpoints,
  writers,
  mayPropose,
}: {
  project: string;
  name: string;
  model: LinkmlModel;
  klass: LinkmlClass;
  area?: string;
  highlight?: ReadonlySet<string>;
  servedBy: string[];
  writers: string[];
  mayPropose: boolean;
}): JSX.Element {
  const { t } = useTranslation();
  const rows = fieldsOf(model, klass);
  return (
    <>
      <div className="flex flex-wrap items-center gap-2">
        <span className="font-mono text-caption text-fg-muted">{klass.name}</span>
        <Badge tone={area === PERSONAL_AREA ? "danger" : "neutral"}>{areaLabel(area, t)}</Badge>
        {klass.from ? <Badge tone="neutral">{t("models.graph.from", { name: klass.from })}</Badge> : null}
      </div>
      {klass.description ? <p className="text-caption text-fg-muted">{klass.description}</p> : null}
      {mayPropose && !klass.from ? (
        <Link
          to="/projects/$project/models/$name"
          params={{ project, name }}
          search={{ class: klass.name }}
          className="focus-ring self-start rounded border border-border px-3 py-1 text-body font-medium"
        >
          {t("models.canvas.edit")}
        </Link>
      ) : (
        <Button size="sm" variant="secondary" className="self-start" disabled disabledReason={klass.from ? t("models.canvas.editImported", { name: klass.from }) : t("models.canvas.editRefused")}>
          {t("models.canvas.edit")}
        </Button>
      )}
      <div className="overflow-x-auto">
        <Table caption={t("models.canvas.fields", { name: klass.name })}>
          <TableHead>
            <TableHeaderCell>{t("models.canvas.field")}</TableHeaderCell>
            <TableHeaderCell>{t("models.canvas.type")}</TableHeaderCell>
          </TableHead>
          <TableBody>
            {rows.map(({ slot, inherited }) => (
              <TableRow key={slot.name} className={inherited || (highlight && !highlight.has(slot.name)) ? "text-fg-muted" : undefined}>
                <TableCell primary>
                  <span className="font-mono">{slot.name}</span>
                  {inherited ? <span className="block text-caption">{t("models.canvas.inherited", { name: inherited })}</span> : null}
                  {slot.personal || klass.personal ? <Badge tone="danger">{t("models.canvas.personalBadge")}</Badge> : null}
                  {highlight?.has(slot.name) ? <span className="block text-caption text-primary-soft-fg">{t("models.canvas.served")}</span> : null}
                  {slot.description ? <span className="block text-caption text-fg-muted">{slot.description}</span> : null}
                </TableCell>
                <TableCell>
                  <span className="font-mono">{slot.range ?? model.default_range ?? "string"}</span>
                  {/* Required and several values in words beside the type: two columns fit the panel. */}
                  {[slot.required ? t("models.canvas.required") : "", slot.multivalued ? t("models.canvas.several") : ""]
                    .filter(Boolean)
                    .map((word) => (
                      <span key={word} className="block text-caption text-fg-muted">
                        {word}
                      </span>
                    ))}
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </div>
      <section aria-labelledby="model-canvas-endpoints" className="flex flex-col gap-1">
        <h3 id="model-canvas-endpoints" className="text-caption font-semibold">
          {t("models.canvas.servedBy")}
        </h3>
        {endpoints.length === 0 ? (
          <p className="text-caption text-fg-muted">{t("models.canvas.servedByNone")}</p>
        ) : (
          <ul className="flex flex-wrap gap-2">
            {endpoints.map((endpoint) => (
              <li key={endpoint}>
                <RecordLink project={project} plural="endpoints" name={endpoint} className="font-mono text-caption" />
              </li>
            ))}
          </ul>
        )}
      </section>
      <section aria-labelledby="model-canvas-pipelines" className="flex flex-col gap-1">
        <h3 id="model-canvas-pipelines" className="text-caption font-semibold">
          {t("models.canvas.writtenBy")}
        </h3>
        {writers.length === 0 ? (
          <p className="text-caption text-fg-muted">{t("models.canvas.writtenByNone")}</p>
        ) : (
          <ul className="flex flex-wrap gap-2">
            {writers.map((pipeline) => (
              <li key={pipeline}>
                <RecordLink project={project} plural="pipelines" name={pipeline} className="font-mono text-caption" />
              </li>
            ))}
          </ul>
        )}
      </section>
    </>
  );
}

/**
 * The how-to of the overlay pick: a "?" beside the label that shows `hint` on hover, press or
 * keyboard focus, and Escape closes. The text stays in the document under `id`, so the select's
 * aria-describedby reads it while it is closed (T-6164).
 */
export function OverlayHelp({ id, label, hint }: { id: string; label: string; hint: string }): JSX.Element {
  const [open, setOpen] = useState(false);
  return (
    <>
      <Button
        variant="ghost"
        size="xs"
        data-testid="model-canvas-overlay-help"
        aria-label={label}
        title={label}
        aria-describedby={id}
        aria-expanded={open}
        className="px-1 text-fg-muted"
        onClick={() => setOpen(true)}
        onMouseEnter={() => setOpen(true)}
        onMouseLeave={() => setOpen(false)}
        onBlur={() => setOpen(false)}
        onKeyDown={(event) => {
          if (event.key === "Escape" && open) {
            event.stopPropagation();
            setOpen(false);
          }
        }}
      >
        <Icon name="help" className="size-4" aria-hidden="true" />
      </Button>
      <span
        id={id}
        role="tooltip"
        className={
          open
            ? "absolute left-0 top-full z-50 mt-1 w-72 rounded-md border border-border bg-surface p-2 text-caption font-normal text-fg shadow-2"
            : "sr-only"
        }
      >
        {hint}
      </span>
    </>
  );
}

/**
 * Where the details sit under the drawing or the list (below lg), a pressed class brings them into
 * view and takes the focus there: on the phone canvas a press changed a panel a screen's height away.
 * `backToDiagram` is the one press that takes the phone back to the drawing (T-6164).
 */
export function useDetailsInView(select: (klass: string) => void) {
  const details = useRef<HTMLHeadingElement>(null);
  const diagram = useRef<HTMLElement>(null);
  const open = (klass: string) => {
    select(klass);
    if (typeof window === "undefined" || window.matchMedia?.("(min-width: 1024px)").matches !== false) return;
    window.requestAnimationFrame(() => {
      details.current?.scrollIntoView?.({ block: "start" });
      details.current?.focus({ preventScroll: true });
    });
  };
  const backToDiagram = () => diagram.current?.scrollIntoView?.({ block: "start" });
  return { details, diagram, open, backToDiagram };
}
