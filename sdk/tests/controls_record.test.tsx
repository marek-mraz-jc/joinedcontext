/**
 * T-3373: the record the Apps' coverage gate reads, which controls a test file rendered and which
 * its tests clicked or typed into. A control no test touched shows as rendered and not exercised.
 */
import { fireEvent, render, screen } from "@testing-library/react";
import { mkdtempSync, readFileSync, readdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { controlId, recordControls } from "../src/sdk/testing";

describe("the controls record (T-3373)", () => {
  it("names a control by its role and the name a person reads", () => {
    render(
      <div>
        <button type="button">Save the change</button>
        <label>
          Station <input name="station" />
        </label>
        <select aria-label="Measure">
          <option>events</option>
        </select>
        <a href="https://example.org">Open in the Portal</a>
      </div>,
    );
    expect(controlId(screen.getByRole("button"))).toBe("button: Save the change");
    expect(controlId(screen.getByRole("textbox"))).toBe("textbox: Station");
    expect(controlId(screen.getByRole("combobox"))).toBe("combobox: Measure");
    expect(controlId(screen.getByRole("link"))).toBe("link: Open in the Portal");
  });

  it("names a control the same whatever counts its name carries", () => {
    render(
      <div>
        <label>
          <input type="checkbox" /> Road work (5)
        </label>
        <button type="button">Page 1 of 1 234,5</button>
      </div>,
    );
    expect(controlId(screen.getByRole("checkbox"))).toBe("checkbox: Road work (#)");
    expect(controlId(screen.getByRole("button"))).toBe("button: Page # of #");
  });

  it("leaves what is hidden from a screen reader out of a control's name", () => {
    render(
      <button type="button">
        Valid to<span aria-hidden="true"> ▼</span>
      </button>,
    );
    expect(controlId(screen.getByRole("button"))).toBe("button: Valid to");
  });

  it("names a list inside its label by the label alone, never by the options it holds", () => {
    render(
      <div>
        <label>
          <span>Weather station</span>
          <select>
            <option>Kaisaniemi (1,4 km)</option>
            <option>Kumpula</option>
          </select>
        </label>
        <select aria-describedby="x">
          <option>Unnamed</option>
        </select>
      </div>,
    );
    const [named, unnamed] = screen.getAllByRole("combobox");
    expect(controlId(named)).toBe("combobox: Weather station");
    expect(controlId(unnamed)).toBe("combobox: ");
  });

  it("writes what was rendered and what a test exercised, leaving out a disabled control", async () => {
    const dir = mkdtempSync(join(tmpdir(), "controls-"));
    let done: (() => Promise<void>) | undefined;
    recordControls((callback) => {
      done = callback;
    }, dir);
    render(
      <div>
        <button type="button">Clicked</button>
        <button type="button">Never touched</button>
        <button type="button" disabled>
          Disabled
        </button>
        <input aria-label="Typed into" />
        <div data-jc-sdk="grid">
          <button type="button">Sort by name</button>
        </div>
      </div>,
    );
    fireEvent.click(screen.getByRole("button", { name: "Clicked" }));
    fireEvent.change(screen.getByRole("textbox", { name: "Typed into" }), { target: { value: "x" } });
    await done?.();
    const [file] = readdirSync(dir);
    const record = JSON.parse(readFileSync(join(dir, file), "utf8")) as { rendered: string[]; exercised: string[] };
    expect(record.rendered).toEqual(expect.arrayContaining(["button: Clicked", "button: Never touched", "textbox: Typed into"]));
    expect(record.rendered).not.toContain("button: Disabled");
    // The SDK's own components are tested by the SDK's suite, not by each App.
    expect(record.rendered).not.toContain("button: Sort by name");
    expect(record.exercised).toEqual(["button: Clicked", "textbox: Typed into"]);
  });

  it("leaves out a control that was gone before anyone could see it, and so has no name", async () => {
    const dir = mkdtempSync(join(tmpdir(), "controls-"));
    let done: (() => Promise<void>) | undefined;
    recordControls((callback) => {
      done = callback;
    }, dir);
    // Added and removed in one task: the observer reports it detached, its label no longer found.
    const label = document.createElement("label");
    label.htmlFor = "brief";
    label.textContent = "Brief";
    const input = document.createElement("input");
    input.id = "brief";
    document.body.append(label, input);
    label.remove();
    input.remove();
    await done?.();
    const [file] = readdirSync(dir);
    const record = JSON.parse(readFileSync(join(dir, file), "utf8")) as { rendered: string[] };
    expect(record.rendered).not.toContain("textbox: ");
  });

  it("records a control under the name it takes after a later change", async () => {
    const dir = mkdtempSync(join(tmpdir(), "controls-"));
    let done: (() => Promise<void>) | undefined;
    recordControls((callback) => {
      done = callback;
    }, dir);
    const button = document.createElement("button");
    button.textContent = "Loading";
    document.body.append(button);
    await Promise.resolve();
    button.firstChild!.textContent = "Kallio stations";
    await done?.();
    const [file] = readdirSync(dir);
    const record = JSON.parse(readFileSync(join(dir, file), "utf8")) as { rendered: string[] };
    expect(record.rendered).toEqual(expect.arrayContaining(["button: Loading", "button: Kallio stations"]));
    button.remove();
  });

  it("does nothing without a directory to write to", () => {
    let registered = false;
    recordControls(() => {
      registered = true;
    }, undefined);
    expect(registered).toBe(false);
  });
});
