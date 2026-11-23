// The time picker's calendar buttons, the copy and paste of a range and the shift forward button.
import { afterEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import "../src/i18n";
import { TimePicker, TimeRangeControls } from "../src/components/panels/TimePicker";

const NOW = Date.UTC(2026, 10, 8, 12, 0, 0);
const H = 3_600_000;

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

function calendars(): HTMLInputElement[] {
  return Array.from(document.querySelectorAll<HTMLInputElement>('input[type="datetime-local"]'));
}

describe("calendar pick", () => {
  it("opens the browser's picker, falls back to focus where showPicker throws, and writes the pick as words", () => {
    render(<TimePicker from="2026-11-07 06:00:00" to="now" now={NOW} onChange={() => undefined} />);
    const [fromPicker, toPicker] = calendars();
    expect(fromPicker.value).toMatch(/^2026-11-07T06:00/);
    expect(toPicker.value).toBe("");
    const shown = vi.fn();
    fromPicker.showPicker = shown;
    fireEvent.click(screen.getByRole("button", { name: "Pick the start on a calendar", hidden: true }));
    expect(shown).toHaveBeenCalledOnce();
    toPicker.showPicker = () => {
      throw new Error("NotAllowedError");
    };
    const focus = vi.spyOn(toPicker, "focus");
    fireEvent.click(screen.getByRole("button", { name: "Pick the end on a calendar", hidden: true }));
    expect(focus).toHaveBeenCalled();
    fireEvent.change(toPicker, { target: { value: "2026-11-08T09:30" } });
    expect(screen.getByRole("textbox", { name: "To", hidden: true })).toHaveValue("2026-11-08 09:30:00");
    fireEvent.change(fromPicker, { target: { value: "2026-11-07T05:10" } });
    expect(screen.getByRole("textbox", { name: "From", hidden: true })).toHaveValue("2026-11-07 05:10:00");
    fireEvent.change(fromPicker, { target: { value: "" } });
    expect(screen.getByRole("textbox", { name: "From", hidden: true })).toHaveValue("2026-11-07 05:10:00");
  });
});

describe("copy and paste", () => {
  it("copies the typed range as JSON and pastes a valid one into both boxes", async () => {
    const written: string[] = [];
    let clip = JSON.stringify({ from: "now-2d", to: "now-1d" });
    vi.stubGlobal("navigator", { ...navigator, clipboard: { writeText: (text: string) => Promise.resolve(void written.push(text)), readText: () => Promise.resolve(clip) } });
    render(<TimePicker from="now-6h" to="now" now={NOW} onChange={() => undefined} />);
    fireEvent.click(screen.getByRole("button", { name: "Copy time range", hidden: true }));
    expect(JSON.parse(written[0])).toEqual({ from: "now-6h", to: "now" });
    fireEvent.click(screen.getByRole("button", { name: "Paste time range", hidden: true }));
    expect(await screen.findByDisplayValue("now-2d")).toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "To", hidden: true })).toHaveValue("now-1d");
    clip = "not a range";
    fireEvent.click(screen.getByRole("button", { name: "Paste time range", hidden: true }));
    expect(await screen.findByText(/The clipboard holds no time range/)).toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "From", hidden: true })).toHaveValue("now-2d");
  });

  it("reads a refused clipboard as an invalid paste", async () => {
    vi.stubGlobal("navigator", { ...navigator, clipboard: { writeText: () => Promise.resolve(), readText: () => Promise.reject(new Error("denied")) } });
    render(<TimePicker from="now-6h" to="now" now={NOW} onChange={() => undefined} />);
    fireEvent.click(screen.getByRole("button", { name: "Paste time range", hidden: true }));
    expect(await screen.findByText(/The clipboard holds no time range/)).toBeInTheDocument();
  });
});

describe("shift forward", () => {
  it("moves the range one span later", async () => {
    const changes: [string, string][] = [];
    render(<TimeRangeControls from={String(10 * H)} to={String(16 * H)} now={100 * H} onChange={(from, to) => changes.push([from, to])} />);
    await userEvent.click(screen.getByRole("button", { name: "Move time range forwards" }));
    expect(changes).toEqual([[String(13 * H), String(19 * H)]]);
  });
});
