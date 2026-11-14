/**
 * One chart drawn by Vega from a Vega-Lite spec, the way the viz renderer of components/viz draws
 * (ADR-N-053): the expression interpreter instead of `eval` (the Portal's CSP has no
 * `unsafe-eval`), a loader that refuses every load (the data is inline), and a canvas, so no
 * inline style attribute is written into the page. Vega is loaded only when a chart is drawn.
 */
import { useEffect, useRef, useState } from "react";
import type { JSX, ReactNode } from "react";
import { clsx } from "clsx";
import type { TopLevelSpec } from "vega-lite";
import type { ChartTheme, Spec } from "./specs";
import { formatWith } from "./specs";
import type { ChartMode } from "./palette";
import { THEME, chartMode } from "./palette";

/** The chart theme (T-6886) for the page's scheme, read when a chart is drawn. */
export function chartTheme(mode: ChartMode = chartMode()): ChartTheme {
  const config = THEME[mode];
  return {
    text: config.axis.titleColor,
    muted: config.axis.labelColor,
    grid: config.axis.gridColor,
    surface: config.background,
    font: config.font,
    config,
  };
}

type VegaModules = [typeof import("vega"), typeof import("vega-lite"), typeof import("vega-interpreter")];

let modules: Promise<VegaModules> | null = null;

/** Vega, Vega-Lite and the interpreter, loaded once, with `jcFormat` registered for axis labels. */
export function loadVega(): Promise<VegaModules> {
  modules ??= Promise.all([import("vega"), import("vega-lite"), import("vega-interpreter")]).then((loaded) => {
    loaded[0].expressionFunction("jcFormat", (value: unknown, key: unknown) => formatWith(value, String(key)));
    return loaded;
  });
  return modules;
}

export interface HoverAt {
  x: number;
  y: number;
  /** The chart's width when the pointer moved, so the tooltip opens away from the nearer edge. */
  width: number;
  datum: Record<string, unknown>;
}

export interface VegaChartProps {
  spec: Spec;
  /** What the chart shows, for a screen reader: the canvas itself says nothing. */
  label: string;
  height: number;
  /** The tooltip's content for the datum under the pointer; nothing draws none. */
  tooltip?: (datum: Record<string, unknown>) => ReactNode;
  className?: string;
  /** The line shown when Vega cannot draw, in place of its own error text. */
  failure?: string;
}

export function VegaChart({ spec, label, height, tooltip, className, failure }: VegaChartProps): JSX.Element {
  const host = useRef<HTMLDivElement>(null);
  const [hover, setHover] = useState<HoverAt | null>(null);
  const [failed, setFailed] = useState<string | null>(null);

  useEffect(() => {
    let view: { finalize: () => void } | null = null;
    let cancelled = false;
    const element = host.current;
    if (!element) return undefined;
    loadVega()
      .then(([vega, vegaLite, interpreter]) => {
        if (cancelled) return undefined;
        const compiled = vegaLite.compile(spec as unknown as TopLevelSpec).spec;
        const loader = vega.loader();
        const refuse = () => Promise.reject(new Error("a dashboard chart loads nothing"));
        loader.load = refuse;
        loader.sanitize = refuse;
        const made = new vega.View(vega.parse(compiled, undefined, { ast: true }), {
          expr: interpreter.expressionInterpreter,
          renderer: "canvas",
          container: element,
          hover: true,
          loader,
        });
        made.tooltip((_handler: unknown, event: MouseEvent, item: { datum?: Record<string, unknown> } | null | undefined, value: unknown) => {
          if (!item || value === null || value === undefined || !item.datum) {
            setHover(null);
            return;
          }
          const box = element.getBoundingClientRect();
          setHover({ x: event.clientX - box.left, y: event.clientY - box.top, width: box.width, datum: item.datum });
        });
        view = made;
        return made.runAsync();
      })
      .then(() => setFailed(null))
      .catch((error: unknown) => {
        if (!cancelled) setFailed(error instanceof Error ? error.message : String(error));
      });
    return () => {
      cancelled = true;
      view?.finalize();
      setHover(null);
    };
  }, [spec]);

  return (
    <div className={clsx("relative w-full", className)}>
      {/* Vega writes its own role and label onto its container, so the chart's label sits a level up. */}
      <div role="img" aria-label={label}>
        {/* A chart that could not be drawn leaves no empty box: the line below says so. */}
        <div ref={host} className="w-full" style={{ height: failed ? 0 : height }} onPointerLeave={() => setHover(null)} />
      </div>
      {/* With `failure` the page holds the values elsewhere: a note, not an error. */}
      {failed ? <p className={clsx("text-caption", failure ? "text-fg-muted" : "text-danger")}>{failure ?? failed}</p> : null}
      {hover && tooltip ? (
        <div
          role="tooltip"
          className="pointer-events-none absolute z-20 max-w-72 rounded-md border border-border bg-surface p-2 text-caption text-fg shadow-2"
          style={{
            top: Math.max(0, hover.y + 12),
            ...(hover.x > hover.width / 2 ? { right: Math.max(0, hover.width - hover.x + 12) } : { left: hover.x + 12 }),
          }}
        >
          {tooltip(hover.datum)}
        </div>
      ) : null}
    </div>
  );
}
