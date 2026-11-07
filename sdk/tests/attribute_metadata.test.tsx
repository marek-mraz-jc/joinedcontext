// T-5529: the Metadata tab of one attribute says who changed it, when and through what, and names
// why a field is hidden instead of leaving it empty.
import { describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within, fireEvent } from "@testing-library/react";
import { AttributeMetadata, newest, when } from "../src/grid/AttributeMetadata";
import { EntityGrid } from "../src/grid/EntityGrid";
import { parseGridConfig } from "../src/grid/config";
import { fixtureSource, SourceError } from "../src/grid/source";
import type { EntitySource, HistoryPoint, HistoryWindow } from "../src/grid/source";

const ID = "urn:ngsi-ld:KeyPerformanceIndicator:kpi-1";
const NOW = new Date("2026-10-24T12:00:00Z");
const cell = { kind: "property" as const, value: 41.5, createdAt: "2026-10-20T08:00:00Z", modifiedAt: "2026-10-24T09:00:00Z" };

function sourceOf(history: (window: HistoryWindow) => Promise<HistoryPoint[]>) {
  const calls: HistoryWindow[] = [];
  const source: Pick<EntitySource, "history"> = {
    history: (_id, _attr, window) => {
      calls.push(window);
      return history(window);
    },
  };
  return { source, calls };
}

function value(term: string): HTMLElement {
  const dt = screen.getByText(term, { selector: "dt" });
  return dt.nextElementSibling as HTMLElement;
}

describe("when (T-5529)", () => {
  it("writes a time relative and absolute in English and Slovak, in the person's time zone", () => {
    const en = when("2026-10-24T09:00:00Z", "en", NOW, "Europe/Bratislava")!;
    expect(en.relative).toBe("3 hours ago");
    expect(en.absolute).toContain("11:00");
    const sk = when("2026-10-20T08:00:00Z", "sk", NOW, "Europe/Bratislava")!;
    expect(sk.relative).toBe("pred 4 dňami");
    expect(sk.absolute).toContain("2026");
    expect(when("not a date", "en", NOW)).toBeNull();
  });
});

describe("newest (T-5529)", () => {
  it("takes the version written last and says {} for a version without provenance", () => {
    expect(
      newest([
        { at: "2026-10-24T09:00:00Z", provenance: { reason: "old", writtenAt: "2026-10-24T09:00:00Z" } },
        { at: "2026-10-24T09:00:00Z", provenance: { reason: "fix", writtenAt: "2026-10-24T10:00:00Z" } },
      ]),
    ).toEqual({ reason: "fix", writtenAt: "2026-10-24T10:00:00Z" });
    expect(newest([{ at: "2026-10-24T09:00:00Z" }])).toEqual({});
    expect(newest([])).toBeUndefined();
  });
});

describe("AttributeMetadata (T-5529)", () => {
  it("shows the last writer's organization, kind, channel and reason from one versions read", async () => {
    const { source, calls } = sourceOf(async () => [
      {
        at: "2026-10-24T09:00:00Z",
        value: 41.5,
        provenance: { actor: "eino", actorKind: "person", organization: "helsinki", channel: "ui", reason: "typo fixed" },
      },
    ]);
    render(<AttributeMetadata source={source} id={ID} attr="kpiValue" cell={cell} locale="en" timeZone="UTC" now={() => NOW} />);
    await waitFor(() => expect(value("Organization").textContent).toBe("helsinki"));
    expect(value("Changed by").textContent).toBe("eino (a person)");
    expect(value("Through").textContent).toBe("Portal");
    expect(value("Reason").textContent).toBe("typo fixed");
    expect(value("Last changed").textContent).toContain("3 hours ago");
    expect(value("Created").querySelector("time")?.getAttribute("dateTime")).toBe(cell.createdAt);
    expect(calls).toEqual([{ versions: true, lastN: 1 }]);
  });

  it("names a pipeline write by its kind and says a missing reason was not given", async () => {
    const { source } = sourceOf(async () => [{ at: "2026-10-24T09:00:00Z", value: 1, provenance: { actorKind: "pipeline", channel: "pipeline" } }]);
    render(<AttributeMetadata source={source} id={ID} attr="kpiValue" cell={cell} locale="en" now={() => NOW} />);
    await waitFor(() => expect(value("Changed by").textContent).toBe("a pipeline"));
    expect(value("Reason").textContent).toBe("No reason was given");
    expect(value("Organization").textContent).toBe("hidden: no access (this view does not pass it)");
  });

  it("shows a 403 on the versions read as history not granted, once, without asking again", async () => {
    const { source, calls } = sourceOf(async () => {
      throw new SourceError(403, "forbidden");
    });
    render(<AttributeMetadata source={source} id={ID} attr="kpiValue" cell={cell} locale="en" now={() => NOW} />);
    // One refusal is said once, under "Changed by", not four times (owner 2026-11-01).
    await waitFor(() => expect(value("Changed by").textContent).toBe("hidden: no access (history not granted)"));
    for (const term of ["Organization", "Through", "Reason"]) {
      expect(screen.queryByText(term)).toBeNull();
    }
    expect(value("Last changed").querySelector("time")).not.toBeNull();
    expect(calls).toHaveLength(1);
  });

  it("names a member the Endpoint did not pass with sysAttrs", async () => {
    const { source } = sourceOf(async () => []);
    render(<AttributeMetadata source={source} id={ID} attr="kpiValue" cell={{ kind: "property", value: 1 }} locale="en" now={() => NOW} />);
    expect(value("Created").textContent).toBe("hidden: no access (this view does not pass it)");
    // Nothing recorded is said once, under "Changed by", not per field.
    await waitFor(() => expect(value("Changed by").textContent).toBe("No change was recorded"));
    for (const term of ["Organization", "Through", "Reason"]) {
      expect(screen.queryByText(term)).toBeNull();
    }
  });

  it("asks a public Endpoint for nothing and hides who wrote it", () => {
    const history = vi.fn();
    render(<AttributeMetadata source={{ history }} id={ID} attr="kpiValue" cell={{ kind: "property", value: 1 }} publicView locale="en" />);
    for (const term of ["Created", "Changed by"]) {
      expect(value(term).textContent).toBe("hidden: no access (public view)");
    }
    expect(screen.queryByText("Organization")).toBeNull();
    expect(history).not.toHaveBeenCalled();
  });

  it("says a failed read in the server's words", async () => {
    const { source } = sourceOf(async () => {
      throw new SourceError(502, "the broker did not answer");
    });
    render(<AttributeMetadata source={source} id={ID} attr="kpiValue" cell={cell} locale="en" now={() => NOW} />);
    await waitFor(() => expect(value("Reason").textContent).toBe("the broker did not answer"));
  });
});

describe("the row detail's Metadata tab (T-5529)", () => {
  it("opens beside History and reads the picked attribute", async () => {
    const base = fixtureSource([
      {
        id: ID,
        type: "KeyPerformanceIndicator",
        name: { type: "Property", value: "Recycling rate" },
        kpiValue: { type: "Property", value: 41.5, createdAt: "2026-10-20T08:00:00Z", modifiedAt: "2026-10-24T09:00:00Z" },
      },
    ]);
    const history = vi.fn(async () => [{ at: "2026-10-24T09:00:00Z", value: 41.5, provenance: { organization: "helsinki", actorKind: "person" } }]);
    const config = parseGridConfig({
      source: { kind: "fixture", name: "test" },
      type: "KeyPerformanceIndicator",
      columns: [
        { attr: "name", label: "Name", pinned: true },
        { attr: "kpiValue", label: "Value" },
      ],
      history: { enabled: true },
    }).config!;
    render(<EntityGrid config={config} source={{ ...base, history }} />);
    fireEvent.click(await screen.findByRole("button", { name: "Open: Recycling rate" }));
    const dialog = await screen.findByRole("dialog");
    fireEvent.click(within(dialog).getByRole("tab", { name: "Metadata" }));
    fireEvent.change(within(dialog).getByRole("combobox"), { target: { value: "kpiValue" } });
    await waitFor(() => expect(within(dialog).getByText("helsinki")).toBeTruthy());
    expect(history).toHaveBeenLastCalledWith(ID, "kpiValue", { versions: true, lastN: 1 });
  });
});
