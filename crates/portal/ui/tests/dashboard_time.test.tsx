/**
 * The dashboard's time words and pickers against Grafana 12.4.14: its quick range and refresh
 * lists as the reference instance draws them (tests/fixtures/grafana-reference), its calendar
 * rounding (weeks from Sunday, a `to` rounded to the unit's last millisecond), a panel's own
 * range (timeFrom only under a relative range, then timeShift), and a multi-value pick.
 */
import { describe, expect, it } from "vitest";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import "../src/i18n";
import { panelQuery, toggled } from "../src/pages/grafana/DashboardPage";
import { overrideWords, pastedRange, RefreshPicker, rangeWords, TimePicker, TimeRangeControls, zoneWords } from "../src/components/panels/TimePicker";
import { absoluteWords, panelRange, QUICK_RANGES, REFRESHES, refreshMs, resolveTime, roundInterval, shiftedRange, zoomedOutRange } from "../src/components/panels/timeRange";
import timePicker from "./fixtures/grafana-reference/time-picker.json";
import refreshPicker from "./fixtures/grafana-reference/refresh-picker.json";
import i18n from "../src/i18n";

const t = i18n.t.bind(i18n) as (key: string, options?: Record<string, unknown>) => string;
// Sunday 8 November 2026, 14:30:15 local time, the day the reference was read.
const NOW = new Date(2026, 10, 8, 14, 30, 15).getTime();
const local = (...parts: [number, number, number, number?, number?, number?, number?]) => new Date(...(parts as [number, number, number, number, number, number, number])).getTime();

describe("Grafana's pickers", () => {
  it("offers Grafana's quick ranges, in its order and words", () => {
    expect(QUICK_RANGES.map((range) => t(`dashboardView.quick.${range.key}`))).toEqual(timePicker.quickRanges);
  });

  it("offers Grafana's refresh intervals", () => {
    render(<RefreshPicker value="" onChange={() => undefined} onRefresh={() => undefined} />);
    expect(screen.getAllByRole("menuitemradio").map((option) => option.textContent)).toEqual(refreshPicker.intervals);
    expect(REFRESHES).toHaveLength(refreshPicker.intervals.length);
  });

  it("keeps a saved refresh that is not on the list", () => {
    render(<RefreshPicker value="2m" onChange={() => undefined} onRefresh={() => undefined} />);
    expect(screen.getByRole("menuitemradio", { checked: true })).toHaveTextContent("2m");
    expect(screen.getByRole("group", { name: "Refresh" }).querySelector("summary")).toHaveTextContent("2m");
  });

  it("picks an interval from the menu, closes it, and refreshes now on its button, as Grafana's picker does", async () => {
    const user = userEvent.setup();
    const picked: string[] = [];
    let now = 0;
    render(<RefreshPicker value="" onChange={(value) => picked.push(value)} onRefresh={() => (now += 1)} />);
    const summary = screen.getByRole("group", { name: "Refresh" }).querySelector("summary")!;
    await user.click(summary);
    await user.click(screen.getByRole("menuitemradio", { name: "1m" }));
    expect(picked).toEqual(["1m"]);
    expect(summary.closest("details")).not.toHaveAttribute("open");
    await user.click(screen.getByRole("button", { name: "Refresh now" }));
    expect(now).toBe(1);
  });

  it("names a quick range by its name and an absolute one by both ends", () => {
    expect(rangeWords("now-6h", "now", NOW, t)).toBe("Last 6 hours");
    expect(rangeWords("now/w", "now/w", NOW, t)).toBe("This week");
    expect(rangeWords(String(local(2026, 10, 7, 6)), String(local(2026, 10, 8, 6)), NOW, t)).toBe("2026-11-07 06:00:00 to 2026-11-08 06:00:00");
  });

  it("narrows the quick ranges to the words searched, as Grafana's quick range search does", async () => {
    const user = userEvent.setup();
    render(<TimePicker from="now-6h" to="now" now={NOW} onChange={() => undefined} />);
    await user.type(screen.getByRole("searchbox", { name: "Search quick ranges" }), "year");
    const quick = screen.getByRole("navigation", { name: /quick/i });
    expect(within(quick).getAllByRole("button").map((button) => button.textContent)).toEqual(timePicker.quickRanges.filter((words: string) => words.toLowerCase().includes("year")));
    await user.type(screen.getByRole("searchbox", { name: "Search quick ranges" }), "zzz");
    expect(within(quick).getByText("No quick range matches.")).toBeInTheDocument();
  });

  it("applies a quick range and an absolute one typed in, and refuses a word Grafana does not read", async () => {
    const user = userEvent.setup();
    const picked: [string, string][] = [];
    render(<TimePicker from="now-6h" to="now" now={NOW} onChange={(from, to) => picked.push([from, to])} />);
    await user.click(screen.getAllByText("Last 6 hours")[0]);
    await user.click(screen.getByRole("button", { name: "Yesterday" }));
    expect(picked.at(-1)).toEqual(["now-1d/d", "now-1d/d"]);
    const from = screen.getByRole("textbox", { name: "From" });
    await user.clear(from);
    await user.type(from, "2026-11-07 06:00:00");
    const to = screen.getByRole("textbox", { name: "To" });
    await user.clear(to);
    await user.type(to, "yesterday");
    await user.click(screen.getByRole("button", { name: "Apply time range" }));
    expect(screen.getByRole("alert")).toHaveTextContent("Write a time such as now-6h");
    expect(picked).toHaveLength(1);
    await user.clear(to);
    await user.type(to, "now");
    await user.click(screen.getByRole("button", { name: "Apply time range" }));
    expect(picked.at(-1)).toEqual([String(local(2026, 10, 7, 6)), "now"]);
  });
});

describe("Grafana's time words", () => {
  it("rounds to the unit's start, and a `to` to its last millisecond", () => {
    expect(resolveTime("now/d", NOW)).toBe(local(2026, 10, 8));
    expect(resolveTime("now/d", NOW, true)).toBe(local(2026, 10, 9) - 1);
    // Grafana reads weeks from Sunday: on Sunday 8 November this week is 8 to 14 November.
    expect([resolveTime("now/w", NOW), resolveTime("now/w", NOW, true)]).toEqual([local(2026, 10, 8), local(2026, 10, 15) - 1]);
    expect([resolveTime("now-1M/M", NOW), resolveTime("now-1M/M", NOW, true)]).toEqual([local(2026, 9, 1), local(2026, 10, 1) - 1]);
    expect([resolveTime("now/fQ", NOW), resolveTime("now/fQ", NOW, true)]).toEqual([local(2026, 9, 1), local(2027, 0, 1) - 1]);
    expect(resolveTime("now-1y/fy", NOW)).toBe(local(2025, 0, 1));
  });

  it("steps days and months on the calendar, a month keeping its day as far as it can", () => {
    expect(resolveTime("now-6M", NOW)).toBe(local(2026, 4, 8, 14, 30, 15));
    expect(resolveTime("now-1M", local(2026, 2, 31, 12))).toBe(local(2026, 1, 28, 12));
    expect(resolveTime("now-1d", local(2026, 2, 29, 12))).toBe(local(2026, 2, 28, 12));
    expect(resolveTime("now-90m", NOW)).toBe(NOW - 90 * 60_000);
  });

  it("reads epochs, ISO times and the picker's local words, and nothing else", () => {
    expect(resolveTime("1700000000000", NOW)).toBe(1_700_000_000_000);
    expect(resolveTime("2026-11-07T06:00:00Z", NOW)).toBe(Date.UTC(2026, 10, 7, 6));
    expect(resolveTime("2026-11-07 06:00:00", NOW)).toBe(local(2026, 10, 7, 6));
    expect(absoluteWords(local(2026, 10, 7, 6))).toBe("2026-11-07 06:00:00");
    for (const wrong of ["yesterday", "now-", "now-6x", "now/d/x", "now-6h junk", ""]) expect(resolveTime(wrong, NOW), wrong).toBeNull();
  });

  it("sets Auto refresh as Grafana does: the range over the width, rounded, at least 5 s", () => {
    expect(roundInterval(11_000)).toBe(10_000);
    expect(refreshMs("auto", 6 * 3_600_000, 1440)).toBe(15_000);
    expect(refreshMs("auto", 7 * 86_400_000, 1440)).toBe(300_000);
    expect(refreshMs("auto", 5 * 60_000, 1440)).toBe(5000);
    expect([refreshMs("1d"), refreshMs("30s"), refreshMs(""), refreshMs("soon")]).toEqual([86_400_000, 30_000, 0, 0]);
  });
});

describe("Grafana's time picker footer", () => {
  it("names the browser's zone, its short name and its UTC offset", () => {
    expect(zoneWords(Date.UTC(2026, 10, 8), "Europe/Bratislava")).toBe("Europe/Bratislava, CET UTC+01:00");
    expect(zoneWords(Date.UTC(2026, 6, 8), "Europe/Bratislava")).toBe("Europe/Bratislava, CEST UTC+02:00");
    expect(zoneWords(Date.UTC(2026, 6, 8), "UTC")).toBe("UTC, UTC UTC+00:00");
  });
});

describe("Grafana's copy and paste of a range", () => {
  const NOW = Date.UTC(2026, 10, 8, 6);
  it("takes the JSON Grafana's copy writes, and refuses anything else", () => {
    expect(pastedRange(JSON.stringify({ from: "now-6h", to: "now" }), NOW)).toEqual({ from: "now-6h", to: "now" });
    expect(pastedRange(JSON.stringify({ from: "1794117600000", to: "1794139200000" }), NOW)).toEqual({ from: "1794117600000", to: "1794139200000" });
    for (const text of ["", "now-6h", "{}", JSON.stringify({ from: "now-6h", to: 5 }), JSON.stringify({ from: "yesterday-ish", to: "now" }), "null"]) {
      expect(pastedRange(text, NOW)).toBeNull();
    }
  });

  it("offers a calendar beside each end and Apply as the primary action", () => {
    render(<TimePicker from="now-6h" to="now" now={NOW} onChange={() => undefined} />);
    expect(screen.getByRole("button", { name: "Pick the start on a calendar", hidden: true })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Pick the end on a calendar", hidden: true })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Copy time range", hidden: true })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Paste time range", hidden: true })).toBeInTheDocument();
  });
});

describe("Grafana's range steps", () => {
  const H = 3_600_000;
  // Grafana 12.4.14 getShiftedTimeRange and getZoomedTimeRange (public/build 7457, 2674).
  it("moves a half span either way, stops a later step at now, and zooms out to twice the span", () => {
    expect(shiftedRange(-1, 10 * H, 16 * H, 100 * H)).toEqual([7 * H, 13 * H]);
    expect(shiftedRange(1, 10 * H, 16 * H, 100 * H)).toEqual([13 * H, 19 * H]);
    // A step past now ends at now and keeps the start, as Grafana's does.
    expect(shiftedRange(1, 10 * H, 16 * H, 17 * H)).toEqual([10 * H, 17 * H]);
    expect(zoomedOutRange(10 * H, 16 * H)).toEqual([7 * H, 19 * H]);
    expect(zoomedOutRange(5, 5)).toEqual([5 - 15_000, 5 + 15_000]);
  });

  it("sets an absolute range from each button", async () => {
    const user = userEvent.setup();
    const changes: [string, string][] = [];
    render(<TimeRangeControls from={String(10 * H)} to={String(16 * H)} now={100 * H} onChange={(from, to) => changes.push([from, to])} />);
    await user.click(screen.getByRole("button", { name: "Move time range backwards" }));
    await user.click(screen.getByRole("button", { name: "Zoom out time range" }));
    expect(changes).toEqual([
      [String(7 * H), String(13 * H)],
      [String(7 * H), String(19 * H)],
    ]);
    expect(within(screen.getByRole("group", { name: "Time range" })).getByRole("button", { name: "Move time range forwards" })).toBeInTheDocument();
  });
});

describe("a panel's own range", () => {
  const relative = { from: "now-6h", to: "now" };
  const absolute = { from: String(local(2026, 10, 7, 6)), to: String(local(2026, 10, 8, 6)) };

  it("reads timeFrom under a relative range and shifts back by timeShift, in Grafana's words", () => {
    const own = panelRange({ timeFrom: "1h", timeShift: "1d" }, relative, NOW);
    expect([own.from, own.to]).toEqual([local(2026, 10, 7, 13, 30, 15), local(2026, 10, 7, 14, 30, 15)]);
    expect(overrideWords(own, t)).toBe("Last 1 hour + timeshift -1d");
    expect(overrideWords(panelRange({ timeFrom: "1h" }, relative, NOW), t)).toBe("Last 1 hour");
  });

  it("keeps the dashboard's absolute range and applies only the shift, as Grafana 12.4 does", () => {
    const own = panelRange({ timeFrom: "1h", timeShift: "1d" }, absolute, NOW);
    expect([own.from, own.to]).toEqual([local(2026, 10, 6, 6), local(2026, 10, 7, 6)]);
    expect(overrideWords(own, t)).toBe("Timeshift -1d");
    expect(overrideWords(panelRange({ timeFrom: "1h" }, absolute, NOW), t)).toBeUndefined();
  });

  it("asks the panel's datasource with the panel's own range as epochs", () => {
    const own = panelRange({ timeShift: "1d" }, absolute, NOW);
    expect(panelQuery(own, { testid: ["k6-a", "k6-b"] }, 100)).toBe(`from=${local(2026, 10, 6, 6)}&to=${local(2026, 10, 7, 6)}&maxDataPoints=100&var-testid=k6-a&var-testid=k6-b`);
  });
});

describe("a multi-value pick", () => {
  it("keeps All alone, replaces it by a value and falls back to it when nothing is left", () => {
    expect(toggled(["$__all"], "a", true)).toEqual(["a"]);
    expect(toggled(["a"], "b", true)).toEqual(["a", "b"]);
    expect(toggled(["a", "b"], "$__all", true)).toEqual(["$__all"]);
    expect(toggled(["a"], "a", true)).toEqual(["$__all"]);
    expect(toggled(["a"], "a", false)).toEqual([]);
  });
});
