/**
 * The dashboard's time picker, laid out as Grafana 12.4's: a button naming the range, and under it
 * an absolute range (From and To, in Grafana's words or as `2026-11-08 06:00:00`) beside the list
 * of Grafana's quick ranges. The refresh picker beside it offers Grafana's intervals.
 */
import { useRef, useState } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { clsx } from "clsx";
import { ToolbarPopover } from "../entities/GridToolbar";
import { Button, Field, fieldIds, Icon, Input } from "../ui";
import { absoluteWords, quickRange, QUICK_RANGES, REFRESHES, resolveTime, shiftedRange, spanParts, zoomedOutRange } from "./timeRange";

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

/**
 * A range Grafana's copy button wrote (`{"from": ..., "to": ...}`), when both ends read as times;
 * anything else on the clipboard is refused.
 */
export function pastedRange(text: string, now: number): { from: string; to: string } | null {
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch {
    return null;
  }
  const { from, to } = (typeof value === "object" && value !== null ? value : {}) as Record<string, unknown>;
  if (typeof from !== "string" || typeof to !== "string") return null;
  const [start, end] = [typed(from, now), typed(to, now)];
  return start === null || end === null ? null : { from: start, to: end };
}

/** A calendar button beside a time box: the browser's own date and time picker, its pick written as Grafana's words. */
function CalendarPick({ label, value, onPick }: { label: string; value: string; onPick: (words: string) => void }): JSX.Element {
  const picker = useRef<HTMLInputElement>(null);
  const local = /^\d{4}-\d{2}-\d{2} \d{2}:\d{2}(:\d{2})?$/.test(value) ? value.replace(" ", "T") : "";
  return (
    <span className="relative flex">
      <Button
        variant="secondary"
        aria-label={label}
        title={label}
        onClick={() => {
          try {
            picker.current?.showPicker();
          } catch {
            picker.current?.focus();
          }
        }}
      >
        <Icon name="calendar" className="size-4" />
      </Button>
      <Input
        ref={picker}
        type="datetime-local"
        step={1}
        tabIndex={-1}
        aria-hidden="true"
        className="pointer-events-none absolute inset-0 h-full w-full opacity-0"
        value={local}
        onChange={(event) => {
          const picked = event.target.value;
          if (picked) onPick(picked.replace("T", " ") + (picked.length === 16 ? ":00" : ""));
        }}
      />
    </span>
  );
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
  const [pasted, setPasted] = useState("");
  const quick = QUICK_RANGES.filter((range) => t(`dashboardView.quick.${range.key}`).toLowerCase().includes(search.trim().toLowerCase()));
  return (
    // At the toolbar's right end, as Grafana's: the panel opens leftwards and stays on the screen.
    <ToolbarPopover label={rangeWords(from, to, now, t)} icon="clock" chevron outlined wide end tall closeLabel={t("dashboardView.timePicker.close")}>
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
            <div className="flex gap-1">
              <Input id="dashboard-time-from" aria-describedby={fieldIds("dashboard-time-from").help} value={start} onChange={(event) => setStart(event.target.value)} />
              <CalendarPick label={t("dashboardView.timePicker.calendarFrom")} value={start} onPick={setStart} />
            </div>
          </Field>
          <Field id="dashboard-time-to" label={t("dashboardView.timePicker.to")} help={t("dashboardView.timePicker.hintTo")} errors={wrong.to ? [t("dashboardView.timePicker.invalid")] : undefined}>
            <div className="flex gap-1">
              <Input id="dashboard-time-to" aria-describedby={fieldIds("dashboard-time-to").help} value={end} onChange={(event) => setEnd(event.target.value)} />
              <CalendarPick label={t("dashboardView.timePicker.calendarTo")} value={end} onPick={setEnd} />
            </div>
          </Field>
          <div className="flex gap-1">
            <Button variant="secondary" size="sm" aria-label={t("dashboardView.timePicker.copy")} title={t("dashboardView.timePicker.copy")} onClick={() => void navigator.clipboard?.writeText(JSON.stringify({ from: start, to: end }))}>
              <Icon name="copy" className="size-4" />
            </Button>
            <Button
              variant="secondary"
              size="sm"
              aria-label={t("dashboardView.timePicker.paste")}
              title={t("dashboardView.timePicker.paste")}
              onClick={async () => {
                const range = pastedRange((await navigator.clipboard?.readText().catch(() => "")) ?? "", now);
                setPasted(range ? "" : t("dashboardView.timePicker.pasteInvalid"));
                if (!range) return;
                setStart(shown(range.from, false));
                setEnd(shown(range.to, true));
              }}
            >
              <Icon name="paste" className="size-4" />
            </Button>
            <Button type="submit" variant="primary" size="sm" className="flex-1">
              {t("dashboardView.timePicker.apply")}
            </Button>
          </div>
          {pasted ? (
            <p role="status" className="text-caption text-danger">
              {pasted}
            </p>
          ) : null}
        </form>
        <nav aria-label={t("dashboardView.timePicker.quick")} className="order-first flex min-w-48 flex-col gap-2 sm:order-none sm:border-l sm:border-border sm:pl-4">
          <Input type="search" aria-label={t("dashboardView.timePicker.search")} placeholder={t("dashboardView.timePicker.search")} value={search} onChange={(event) => setSearch(event.target.value)} />
          {quick.length === 0 ? <p className="text-caption text-fg-muted">{t("dashboardView.timePicker.noQuick")}</p> : null}
          {/* A height that cuts a row in half, so the list reads as one that scrolls, as Grafana's. */}
          <ul className="flex max-h-60 flex-col overflow-y-auto sm:max-h-75">
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
      <p className="mt-3 flex flex-wrap gap-x-2 border-t border-border pt-2 text-caption text-fg-muted">
        <span className="font-semibold text-fg">{t("dashboardView.timePicker.browserTime")}</span>
        <span>{zoneWords(now)}</span>
      </p>
    </ToolbarPopover>
  );
}

/**
 * The browser's zone as Grafana's time picker footer names it: the zone, its short name and its
 * offset from UTC at `at` (`Europe/Bratislava, CET UTC+01:00`).
 */
export function zoneWords(at: number, zone?: string): string {
  const format = new Intl.DateTimeFormat("en-GB", { timeZone: zone, timeZoneName: "short" });
  const name = format.resolvedOptions().timeZone;
  const short = format.formatToParts(at).find((part) => part.type === "timeZoneName")?.value ?? "";
  const local = new Date(new Date(at).toLocaleString("en-US", { timeZone: name })).getTime();
  const utc = new Date(new Date(at).toLocaleString("en-US", { timeZone: "UTC" })).getTime();
  const minutes = Math.round((local - utc) / 60_000);
  const offset = `UTC${minutes < 0 ? "-" : "+"}${String(Math.floor(Math.abs(minutes) / 60)).padStart(2, "0")}:${String(Math.abs(minutes) % 60).padStart(2, "0")}`;
  return `${name}, ${short === offset ? "" : `${short} `}${offset}`;
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
      <ToolbarPopover label={words(value)} chevron outlined end tall closeLabel={t("dashboardView.timePicker.close")}>
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

/**
 * Grafana's range controls: « a half span earlier, the picker, » a half span later, and zoom out
 * to twice the span. Each step sets an absolute range, as Grafana's buttons do.
 */
export function TimeRangeControls({ from, to, now, onChange }: TimePickerProps): JSX.Element {
  const { t } = useTranslation();
  const start = resolveTime(from, now) ?? now;
  const end = resolveTime(to, now, true) ?? now;
  const go = ([nextFrom, nextTo]: [number, number]) => onChange(String(Math.round(nextFrom)), String(Math.round(nextTo)));
  return (
    <div role="group" aria-label={t("dashboardView.rangeLabel")} className="flex flex-wrap items-start gap-1 sm:ml-auto">
      <Button variant="secondary" aria-label={t("dashboardView.shiftBack")} title={t("dashboardView.shiftBack")} onClick={() => go(shiftedRange(-1, start, end, Date.now()))}>
        <Icon name="chevronsLeft" className="size-4" />
      </Button>
      <TimePicker key={`${from}|${to}`} from={from} to={to} now={now} onChange={onChange} />
      <Button variant="secondary" aria-label={t("dashboardView.shiftForward")} title={t("dashboardView.shiftForward")} onClick={() => go(shiftedRange(1, start, end, Date.now()))}>
        <Icon name="chevronsRight" className="size-4" />
      </Button>
      <Button variant="secondary" aria-label={t("dashboardView.zoomOut")} title={t("dashboardView.zoomOut")} onClick={() => go(zoomedOutRange(start, end))}>
        <Icon name="zoomOut" className="size-4" />
      </Button>
    </div>
  );
}
