/**
 * T-3827: the roles. A steward marks a finding resolved with a note, or corrects the attribute,
 * which the App writes through its Endpoint, and opens a finding again; everyone sees what was
 * resolved; the history shows a viewer their own changes and a steward everyone's. Anyone else
 * finds the steward's buttons in sight, closed, with the reason.
 */
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { JcProvider } from "@joinedcontext/sdk";
import type { Schema } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";
import App from "./App";
import { valueOf } from "./components/FindingAction";
import { ENTITIES, NOW, SCHEMA_WITH_VEHICLE } from "./fixtures/quality";
import { InspectorContext, parseAnswer, type Inspector } from "./inspect";
import { problemText } from "./problems";
import { Problem, ServerContext } from "./server";
import type { Server } from "./server";
import { roles, STEWARD, VIEWER } from "./testing/roles";
import type { Roles } from "./testing/roles";
import { sweep } from "./testing/sweep";
import wasm from "../wasm/pkg/data_quality_inspector_bg.wasm?url&inline";
import { initSync, inspect } from "../wasm/pkg/data_quality_inspector.js";

// A sweep presses every control: a slow CI runner takes several times a laptop's 4 s.
const SWEEP = 30_000;

initSync({ module: Uint8Array.from(atob(wasm.slice(wasm.indexOf(",") + 1)), (c) => c.charCodeAt(0)) });
const runInspector: Inspector = async (input) => parseAnswer(inspect(JSON.stringify(input)));

vi.mock("echarts", () => ({ init: vi.fn(() => ({ setOption: vi.fn(), resize: vi.fn(), dispose: vi.fn(), on: vi.fn() })) }));

const ACCESS = {
  permissions: ["BikeHireDockingStation", "AdministrativeArea", "Event", "Vehicle"].map((type) => ({ resource: { type }, actions: ["queryEntity", "retrieveEntity"], attributes: "*" as const })),
  prohibitions: [],
};
const NEG = "urn:ngsi-ld:BikeHireDockingStation:fail-neg";

let server: Server & Roles;
/** The App with a fresh server for `who`; `before` sets the server up before the page reads it. */
function show(language = "en", who: Roles["who"] = null, before: (server: Server & Roles) => void = () => undefined) {
  window.history.replaceState(null, "", `/?lang=${language}#quality?type=BikeHireDockingStation`);
  server = Object.assign(roles(who), {
    runs: async () => ({ runs: [], stale: false }),
    reportUrl: async () => "https://store.test/r.json",
  });
  before(server);
  const client = stubClient({ entities: ENTITIES, schema: SCHEMA_WITH_VEHICLE as unknown as Schema, access: ACCESS }, { appName: "data-quality-inspector" });
  return render(
    <JcProvider client={client}>
      <ServerContext.Provider value={server}>
        <InspectorContext.Provider value={runInspector}>
          <App />
        </InspectorContext.Provider>
      </ServerContext.Provider>
    </JcProvider>,
  );
}
const failing = () => screen.findByRole("table", { name: "Failing entities" });
const statuses = () => screen.getAllByRole("status").map((s) => s.textContent);

beforeEach(() => {
  vi.useFakeTimers({ toFake: ["Date"] });
  vi.setSystemTime(new Date(NOW * 1000));
});
afterEach(() => {
  vi.useRealTimers();
  window.history.replaceState(null, "", "/");
});

describe("the parts", () => {
  it("reads a corrected value by its kind, and refuses what is none", () => {
    expect(valueOf("text", "  Kallio ")).toBe("Kallio");
    expect(valueOf("text", "  ")).toBeNull();
    expect(valueOf("text", "x".repeat(501))).toBeNull();
    expect(valueOf("number", "12,5")).toBe(12.5);
    expect(valueOf("number", "-3")).toBe(-3);
    expect(valueOf("number", "")).toBeNull();
    expect(valueOf("number", "ten")).toBeNull();
    expect(valueOf("boolean", "true")).toBe(true);
    expect(valueOf("boolean", "false")).toBe(false);
    expect(valueOf("boolean", "maybe")).toBeNull();
  });

  it("says what each refusal means", () => {
    expect(problemText("en", new Problem(0, ""))).toMatch(/could not be reached/);
    expect(problemText("en", new Problem(401, ""))).toMatch(/Sign in again/);
    expect(problemText("en", new Problem(403, ""))).toBe("Your account may not do this.");
    expect(problemText("en", new Problem(404, ""))).toMatch(/no longer there/);
    expect(problemText("en", new Problem(409, "this finding is resolved already"))).toBe("The server refused: this finding is resolved already");
    expect(problemText("en", new Problem(502, ""))).toMatch(/did not answer/);
    expect(problemText("en", "odd")).toMatch(/did not answer/);
  });
});

describe("signed out and a viewer", () => {
  it("see what was resolved, and the steward's buttons closed with the reason", async () => {
    show("en", VIEWER, (s) =>
      s.resolved.push({ id: 1, entity: NEG, attribute: "availableBikeNumber", note: "Sensor reset", corrected: false, resolverName: "Sami Steward", resolvedAt: "2026-10-08T12:00:00Z" }),
    );
    expect(await screen.findByLabelText("Your role: Viewer")).toBeInTheDocument();
    const table = await failing();
    expect(await within(table).findByText("Resolved by Sami Steward: Sensor reset")).toBeInTheDocument();
    const reopen = within(table).getByRole("button", { name: "Open again: availableBikeNumber fail-neg" });
    expect(reopen).toBeDisabled();
    expect(reopen).toHaveAttribute("title", "Only a steward resolves or corrects findings.");
    for (const resolve of within(table).getAllByRole("button", { name: /^Resolve: / })) {
      expect(resolve).toBeDisabled();
      expect(resolve).toHaveAttribute("title", "Only a steward resolves or corrects findings.");
    }
    expect(statuses()).toContain("Only a steward resolves or corrects findings.");
    fireEvent.click(screen.getByRole("button", { name: "History" }));
    expect(await screen.findByText("Your own changes; a steward sees everyone's.")).toBeInTheDocument();
    expect(await screen.findByText("No change yet.")).toBeInTheDocument();
  });

  it("signed out, are told so and to sign in for the history", async () => {
    show();
    expect(await screen.findByLabelText("Your role: Not signed in")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "History" }));
    expect(await screen.findByText("Sign in to see the history.")).toBeInTheDocument();
    expect(server.history).not.toHaveBeenCalled();
  });

  it("take the server's silence about who calls as signed out, and its failure to list resolutions in words", async () => {
    show("en", null, (s) => {
      vi.mocked(s.me).mockRejectedValueOnce(new Problem(502, ""));
      vi.mocked(s.resolutions).mockRejectedValueOnce(new Problem(502, ""));
    });
    expect((await screen.findAllByLabelText("Your role: Not signed in")).length).toBeGreaterThan(0);
    await waitFor(() => expect(statuses()).toContain("The server did not answer. Try again in a moment."));
  });
});

describe("a steward", () => {
  it("marks a finding resolved, opens it again, and finds both in everyone's history", async () => {
    show("en", STEWARD);
    expect(await screen.findByLabelText("Your role: Steward")).toBeInTheDocument();
    const table = await failing();
    fireEvent.click(within(table).getByRole("button", { name: "Resolve: availableBikeNumber fail-neg" }));
    const form = screen.getByRole("form", { name: "Resolve: availableBikeNumber of fail-neg" });
    const mark = within(form).getByRole("button", { name: "Mark resolved" });
    expect(mark).toBeDisabled();
    fireEvent.change(within(form).getByRole("textbox", { name: "Note" }), { target: { value: "  Sensor reset " } });
    fireEvent.click(mark);
    expect(await within(table).findByText("Resolved by Sami Steward: Sensor reset")).toBeInTheDocument();
    expect(server.resolve).toHaveBeenCalledWith(NEG, "availableBikeNumber", "Sensor reset");
    expect(statuses()).toContain("The finding is resolved.");
    expect(screen.queryByRole("form", { name: /^Resolve: / })).toBeNull();

    // Open again: at once, back when the server refuses; then for good.
    vi.mocked(server.reopen).mockRejectedValueOnce(new Problem(403, ""));
    fireEvent.click(within(table).getByRole("button", { name: "Open again: availableBikeNumber fail-neg" }));
    await waitFor(() => expect(statuses()).toContain("Your account may not do this."));
    expect(within(table).getByText("Resolved by Sami Steward: Sensor reset")).toBeInTheDocument();
    fireEvent.click(within(table).getByRole("button", { name: "Open again: availableBikeNumber fail-neg" }));
    await waitFor(() => expect(statuses()).toContain("The finding is open again."));
    expect(within(table).queryByText("Resolved by Sami Steward: Sensor reset")).toBeNull();

    server.log.push({ id: 1, at: "2026-10-07T12:00:00Z", actorName: "Other Steward", action: "insert", target: "resolutions/9", summary: "resolved name of x", owner: "u-other" });
    fireEvent.click(screen.getByRole("button", { name: "History" }));
    expect(await screen.findByText("Everyone's changes, newest first.")).toBeInTheDocument();
    expect(await screen.findByText(/removed: reopened a finding/)).toBeInTheDocument();
    expect(screen.getByText("Other Steward")).toBeInTheDocument();
  });

  it("corrects the value by its kind through the server, which resolves the finding", async () => {
    show("en", STEWARD);
    const table = await failing();
    fireEvent.click(await within(table).findByRole("button", { name: "Resolve: availableBikeNumber fail-neg" }));
    const form = screen.getByRole("form", { name: "Resolve: availableBikeNumber of fail-neg" });
    const correct = within(form).getByRole("button", { name: "Correct the value" });
    fireEvent.change(within(form).getByRole("textbox", { name: "Note" }), { target: { value: "Counted on site" } });
    fireEvent.change(within(form).getByRole("combobox", { name: "Kind of value" }), { target: { value: "number" } });
    fireEvent.change(within(form).getByRole("textbox", { name: "Correct value" }), { target: { value: "four" } });
    expect(correct).toBeDisabled();
    fireEvent.change(within(form).getByRole("textbox", { name: "Correct value" }), { target: { value: "4" } });

    vi.mocked(server.correct).mockRejectedValueOnce(new Problem(403, ""));
    fireEvent.click(correct);
    await waitFor(() => expect(within(form).getByRole("status")).toHaveTextContent("Your account may not do this."));
    expect(server.corrected).toEqual([]);
    fireEvent.click(correct);
    expect(await within(table).findByText("Corrected by Sami Steward: Counted on site")).toBeInTheDocument();
    expect(server.corrected).toEqual([{ entity: NEG, attribute: "availableBikeNumber", value: 4 }]);
    expect(statuses()).toContain("The value is corrected and the finding resolved.");

    // Yes or no; a correction of a finding already resolved keeps its resolution.
    fireEvent.click(within(table).getAllByRole("button", { name: /^Resolve: / })[0]);
    const next = screen.getByRole("form", { name: /^Resolve: / });
    fireEvent.change(within(next).getByRole("textbox", { name: "Note" }), { target: { value: "x" } });
    fireEvent.change(within(next).getByRole("combobox", { name: "Kind of value" }), { target: { value: "boolean" } });
    fireEvent.change(within(next).getByRole("combobox", { name: "Correct value" }), { target: { value: "false" } });
    fireEvent.click(within(next).getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("form", { name: /^Resolve: / })).toBeNull();
  });

  for (const [language, pages] of [
    ["en", ["History", "Helsinki data quality"]],
    ["fi", ["Historia", "Helsingin datan laatu"]],
    ["sv", ["Historik", "Helsingfors datakvalitet"]],
  ] as const) {
    it(`uses every control of every page once, in ${language}`, async () => {
      const { container } = show(language, STEWARD, (s) =>
        s.resolved.push({ id: 1, entity: NEG, attribute: "availableBikeNumber", note: "Sensor reset", corrected: true, resolverName: "Sami Steward", resolvedAt: "2026-10-08T12:00:00Z" }),
      );
      await waitFor(() => expect(container.querySelector("table.jc-table button.app-open")).not.toBeNull());
      const main = () => container.querySelector<HTMLElement>("main") ?? container;
      // Each type's findings answered in turn: each one's form opened and every control of it used,
      // once as it opens and once with a yes-or-no value; a resolved one opened again.
      for (const type of ["BikeHireDockingStation", "AdministrativeArea", "Event", "Vehicle"]) {
        await act(async () => {
          window.location.hash = `#quality?type=${type}`;
          window.dispatchEvent(new HashChangeEvent("hashchange"));
        });
        await waitFor(() => expect(main().querySelector(".jc-table")).not.toBeNull());
        const answered = new Set<string>();
        for (;;) {
          const label = (b: HTMLButtonElement) => b.getAttribute("aria-label") ?? "";
          const next = [...main().querySelectorAll<HTMLButtonElement>("table.jc-table button:not(.app-open)")].find((b) => !answered.has(label(b)) && !b.disabled);
          if (!next) break;
          answered.add(label(next));
          for (const kind of ["text", "boolean"]) {
            await act(async () => fireEvent.click(next));
            const form = main().querySelector<HTMLElement>("form.app-action");
            if (!form) break;
            await act(async () => fireEvent.change(form.querySelector("select")!, { target: { value: kind } }));
            const [, choice] = form.querySelectorAll("select");
            if (choice) for (const value of ["false", "true"]) await act(async () => fireEvent.change(choice, { target: { value } }));
            await act(async () => sweep(form));
          }
        }
      }
      // Each type's tile on the overview, which opens the type.
      for (let i = 0; ; i++) {
        await act(async () => {
          window.location.hash = "#quality";
          window.dispatchEvent(new HashChangeEvent("hashchange"));
        });
        const tiles = await waitFor(() => {
          const found = main().querySelectorAll<HTMLElement>(".app-tile-clickable");
          expect(found.length).toBeGreaterThan(0);
          return found;
        });
        if (i >= tiles.length) break;
        await act(async () => fireEvent.click(tiles[i]));
      }
      await act(async () => sweep(main()));
      for (const page of pages) {
        fireEvent.click(await screen.findByRole("button", { name: page }));
        await act(async () => new Promise((done) => setTimeout(done, 0)));
        await act(async () => sweep(main()));
      }
    }, SWEEP);
  }
});

describe("in Swedish", () => {
  it("speaks Swedish in the header and the failing table", async () => {
    show("sv", VIEWER);
    expect(await screen.findByLabelText("Din roll: Läsare")).toBeInTheDocument();
    expect(await screen.findByRole("table", { name: "Felaktiga objekt" })).toBeInTheDocument();
    expect(screen.getAllByText("Öppen").length).toBeGreaterThan(0);
  });
});
