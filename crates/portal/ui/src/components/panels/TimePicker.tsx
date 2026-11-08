/**
 * The dashboard's time picker, laid out as Grafana 12.4's: a button naming the range, and under it
 * an absolute range (From and To, in Grafana's words or as `2026-11-08 06:00:00`) beside the list
 * of Grafana's quick ranges. The refresh picker beside it offers Grafana's intervals.
 */
import { useState } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { clsx } from "clsx";
import { ToolbarPopover } from "../entities/GridToolbar";
import { Button, Field, Icon, Input } from "../ui";
import { absoluteWords, quickRange, QUICK_RANGES, REFRESHES, resolveTime, spanParts } from "./timeRange";

type T = (key: string, options?: Record<string, unknown>) => string;

/** A span (`1h`) as Grafana's words, `Last 1 hour`, from the quick range of that span when there is one. */
export function spanWords(span: string, t: T): string {
  return t(`dashboardView.quick.last${span}`, { defaultValue: `Last ${span}` });
}

/** The range's name on the picker: a quick range by its name, `now-4h` as `Last 4h`, else both ends. */
export function rangeWords(from: string, to: string, now: number, t: T): string {
  const named = quickRange(from, to);
  if (named) return t(`dashboardView.quick.${named.key}`);
  const span = /^now-(.+)$/.exec(from)?.[1];
  if (to === "now" && span && spanParts(span)) return spanWords(span, t);
  const start = resolveTime(from, now);
  const end = resolveTime(to, now, true);
  return start === null || end === null ? `${from} – ${to}` : t("dashboardView.timePicker.between", { from: absoluteWords(start), to: absoluteWords(end) });
}

/** The words a panel's own range shows beside its title, as Grafana writes them. */
export function overrideWords(own: { timeFrom?: string; timeShift?: string }, t: T): string | undefined {
  const range = own.timeFrom ? spanWords(own.timeFrom, t) : undefined;
  if (range && own.timeShift) return t("dashboardView.timePicker.rangeShifted", { range, shift: own.timeShift });
  if (own.timeShift) return t("dashboardView.timePicker.shifted", { shift: own.timeShift });
  return range;
}

/** The time words typed in, as an absolute time picker writes them: an instant as local time. */
function typed(text: string, now: number): string | null {
  const trimmed = text.trim();
  if (trimmed.startsWith("now") || /^\d+$/.test(trimmed)) return resolveTime(trimmed, now) === null ? null : trimmed;
  const at = resolveTime(trimmed, now);
  return at === null ? null : String(at);
}

export interface TimePickerProps {
  from: string;
  to: string;
  now: number;
  onChange: (from: string, to: string) => void;
}

export function TimePicker({ from, to, now, onChange }: TimePickerProps): JSX.Element {
  const { t } = useTranslation();
  const shown = (word: string, roundUp: boolean) => (/^\d+$/.test(word) ? absoluteWords(resolveTime(word, now, roundUp) ?? now) : word);
  const [start, setStart] = useState(() => shown(from, false));
  const [end, setEnd] = useState(() => shown(to, true));
  const [wrong, setWrong] = useState<{ from?: boolean; to?: boolean }>({});
  const [search, setSearch] = useState("");
  const quick = QUICK_RANGES.filter((range) => t(`dashboardView.quick.${range.key}`).toLowerCase().includes(search.trim().toLowerCase()));
  return (
    // At the toolbar's right end, as Grafana's: the panel opens leftwards and stays on the screen.
    <ToolbarPopover label={rangeWords(from, to, now, t)} outlined wide end tall closeLabel={t("dashboardView.timePicker.close")}>
      <div className="flex flex-col gap-4 sm:flex-row">
        <form
          className="flex min-w-56 flex-col gap-3"
          aria-label={t("dashboardView.timePicker.absolute")}
          onSubmit={(event) => {
            event.preventDefault();
            const nextFrom = typed(start, now);
            const nextTo = typed(end, now);
            setWrong({ from: nextFrom === null, to: nextTo === null });
            if (nextFrom === null || nextTo === null) return;
            onChange(nextFrom, nextTo);
            close(event.currentTarget);
          }}
        >
          <p className="text-body font-semibold text-fg">{t("dashboardView.timePicker.absolute")}</p>
          <Field id="dashboard-time-from" label={t("dashboardView.timePicker.from")} help={t("dashboardView.timePicker.hint")} errors={wrong.from ? [t("dashboardView.timePicker.invalid")] : undefined}>
            <Input id="dashboard-time-from" value={start} onChange={(event) => setStart(event.target.value)} />
          </Field>
          <Field id="dashboard-time-to" label={t("dashboardView.timePicker.to")} help={t("dashboardView.timePicker.hintTo")} errors={wrong.to ? [t("dashboardView.timePicker.invalid")] : undefined}>
            <Input id="dashboard-time-to" value={end} onChange={(event) => setEnd(event.target.value)} />
          </Field>
          <Button type="submit" size="sm">
            {t("dashboardView.timePicker.apply")}
          </Button>
        </form>
        <nav aria-label={t("dashboardView.timePicker.quick")} className="order-first flex min-w-48 flex-col gap-2 sm:order-none sm:border-l sm:border-border sm:pl-4">
          <Input type="search" aria-label={t("dashboardView.timePicker.search")} placeholder={t("dashboardView.timePicker.search")} value={search} onChange={(event) => setSearch(event.target.value)} />
          {quick.length === 0 ? <p className="text-caption text-fg-muted">{t("dashboardView.timePicker.noQuick")}</p> : null}
          <ul className="flex max-h-60 flex-col overflow-y-auto sm:max-h-80">
            {quick.map((range) => {
              const current = range.from === from && range.to === to;
              return (
                <li key={range.key}>
                  <Button
                    variant="ghost"
                    size="sm"
                    aria-pressed={current}
                    className={clsx("w-full justify-start", current ? "bg-primary-soft text-primary-soft-fg" : undefined)}
                    onClick={(event) => {
                      setStart(range.from);
                      setEnd(range.to);
                      onChange(range.from, range.to);
                      close(event.currentTarget);
                    }}
                  >
                    {t(`dashboardView.quick.${range.key}`)}
                  </Button>
                </li>
              );
            })}
          </ul>
        </nav>
      </div>
    </ToolbarPopover>
  );
}

/** The popover around `target` closes, as a picked menu item closes Grafana's. */
function close(target: EventTarget): void {
  const details = (target as HTMLElement).closest("details");
  if (details) {
    details.open = false;
    details.querySelector("summary")?.focus();
  }
}

/**
 * Grafana's refresh picker: a Refresh now button joined to a menu of its intervals (Off, Auto,
 * then the intervals as Grafana writes them), the picked interval named on the menu's button.
 */
export function RefreshPicker({ value, onChange, onRefresh }: { value: string; onChange: (value: string) => void; onRefresh: () => void }): JSX.Element {
  const { t } = useTranslation();
  const choices: string[] = (REFRESHES as readonly string[]).includes(value) ? [...REFRESHES] : [...REFRESHES, value];
  const words = (every: string) => (every === "" ? t("dashboardView.refreshOff") : every === "auto" ? t("dashboardView.refreshAuto") : every);
  return (
    <div role="group" aria-label={t("dashboardView.refreshLabel")} className="flex items-start gap-1">
      <Button variant="secondary" onClick={onRefresh}>
        <Icon name="refresh" className="size-4" />
        {t("dashboardView.refreshNow")}
      </Button>
      <ToolbarPopover label={words(value)} outlined end closeLabel={t("dashboardView.timePicker.close")}>
        <ul role="menu" aria-label={t("dashboardView.refreshEvery")} className="flex flex-col">
          {choices.map((every) => (
            <li key={every} role="none">
              <Button
                role="menuitemradio"
                aria-checked={every === value}
                variant="ghost"
                size="sm"
                className={clsx("w-full justify-start", every === value ? "bg-primary-soft text-primary-soft-fg" : undefined)}
                onClick={(event) => {
                  onChange(every);
                  close(event.currentTarget);
                }}
              >
                {words(every)}
              </Button>
            </li>
          ))}
        </ul>
      </ToolbarPopover>
    </div>
  );
}
