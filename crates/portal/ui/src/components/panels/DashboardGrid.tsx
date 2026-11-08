/**
 * The panels of a dashboard where their `gridPos` puts them: 24 columns of 30 px rows from `lg`
 * up, as Grafana lays them out; one column in reading order below it. A `row` panel is a heading
 * across the grid that folds the panels under it.
 */
import { useState, useSyncExternalStore } from "react";
import type { JSX, ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { Button, Icon } from "../ui";
import type { PanelDefinition } from "./PanelView";

/** Tailwind's `lg`. */
const WIDE = "(min-width: 64rem)";
/** One grid row and the gap under it, in px: `lg:auto-rows-[minmax(24px,auto)]` and `gap-3`. A panel's `gridPos.h` counts rows. */
export const ROW_PX = 36;
/** The panel's title and padding above its body, and the last row's missing gap, in px. */
const CHROME_PX = 68;

function onWideChange(changed: () => void): () => void {
  const query = typeof window.matchMedia === "function" ? window.matchMedia(WIDE) : null;
  query?.addEventListener("change", changed);
  return () => query?.removeEventListener("change", changed);
}

function isWide(): boolean {
  return typeof window.matchMedia === "function" && window.matchMedia(WIDE).matches;
}

/** The body height a panel gets: its rows less the card's own chrome, never under 120 px. */
export function bodyHeight(panel: PanelDefinition): number {
  // A description is a line or two under the title.
  const described = panel.description ? 32 : 0;
  return Math.max(120, (panel.gridPos?.h ?? 8) * ROW_PX - CHROME_PX - described);
}

/** The panels in reading order: by row, then by column, a folded row's own panels after it. */
export function ordered(panels: PanelDefinition[]): PanelDefinition[] {
  return [...panels].sort((a, b) => (a.gridPos?.y ?? 0) - (b.gridPos?.y ?? 0) || (a.gridPos?.x ?? 0) - (b.gridPos?.x ?? 0));
}

/** The groups a dashboard's `row` panels make: the panels before the first row, then each row's own. */
export function sections(panels: PanelDefinition[]): { row?: PanelDefinition; panels: PanelDefinition[] }[] {
  const out: { row?: PanelDefinition; panels: PanelDefinition[] }[] = [{ panels: [] }];
  for (const panel of ordered(panels)) {
    if (panel.type === "row") out.push({ row: panel, panels: [...(panel.panels ?? [])] });
    else out[out.length - 1].panels.push(panel);
  }
  return out.filter((section) => section.row || section.panels.length > 0);
}

export interface DashboardGridProps {
  panels: PanelDefinition[];
  render: (panel: PanelDefinition) => ReactNode;
}

function Cells({ panels, render, wide }: { panels: PanelDefinition[]; render: DashboardGridProps["render"]; wide: boolean }): JSX.Element {
  const top = Math.min(...panels.map((panel) => panel.gridPos?.y ?? 0));
  return (
    <div className="grid grid-cols-1 gap-3 lg:auto-rows-[minmax(24px,auto)] lg:grid-cols-24">
      {ordered(panels).map((panel) => {
        const pos = panel.gridPos ?? {};
        const h = pos.h ?? 8;
        // A computed place on the 24-column grid; below `lg` the panel takes its content's height,
        // so an empty or failed panel stays short on a phone.
        const place = wide ? { gridColumn: `${(pos.x ?? 0) + 1} / span ${Math.min(24, pos.w ?? 12)}`, gridRow: `${(pos.y ?? 0) - top + 1} / span ${h}` } : undefined;
        return (
          <div key={panel.id} className="min-w-0" style={place}>
            {render(panel)}
          </div>
        );
      })}
    </div>
  );
}

export function DashboardGrid({ panels, render }: DashboardGridProps): JSX.Element {
  const { t } = useTranslation();
  const wide = useSyncExternalStore(onWideChange, isWide, () => true);
  const [open, setOpen] = useState<Record<number, boolean>>({});
  return (
    <div className="flex flex-col gap-4">
      {sections(panels).map((section, index) => {
        if (!section.row) return <Cells key={`top-${index}`} panels={section.panels} render={render} wide={wide} />;
        const row = section.row;
        const shown = open[row.id] ?? !row.collapsed;
        const title = row.title || t("dashboardView.untitled");
        return (
          <section key={row.id} aria-label={title} className="flex flex-col gap-3">
            <h2 className="text-body font-semibold text-fg">
              <Button variant="ghost" size="sm" aria-expanded={shown} onClick={() => setOpen({ ...open, [row.id]: !shown })}>
                <Icon name={shown ? "chevronDown" : "chevronRight"} className="size-4" />
                {title}
              </Button>
            </h2>
            {shown && section.panels.length > 0 ? <Cells panels={section.panels} render={render} wide={wide} /> : null}
          </section>
        );
      })}
    </div>
  );
}
