/**
 * T-5516: the one view of a data record. An NGSI-LD entity reads as label/value rows, never as
 * JSON text; a nested object folds; "Copy as JSON" keeps the raw form one click away; a URL value
 * links only through safeHref; and axe finds nothing to fault.
 */
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { afterEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { RecordView } from "../src/components/ui";
import { Nested } from "../src/components/ui/RecordView";
import { cellOf, geometryText } from "../src/components/ui/recordView";
import { pageFindings } from "./pageChecks";
import { expectNoAxeViolations } from "./page_contract";

const ENTITY = {
  id: "urn:ngsi-ld:AirQualityObserved:ba-01",
  type: "AirQualityObserved",
  pm10: { type: "Property", value: 12, unitCode: "GQ", observedAt: "2026-10-24T08:00:00Z" },
  refDevice: { type: "Relationship", object: "urn:ngsi-ld:Device:ba-01" },
  location: { type: "GeoProperty", value: { type: "Point", coordinates: [17.1077, 48.1486] } },
  address: { type: "Property", value: { streetAddress: "Hlavná 1", addressLocality: "Bratislava" } },
  source: { type: "Property", value: "https://data.example.sk/air" },
  bad: "javascript:alert(1)",
};

function show(value: unknown) {
  return render(
    <I18nextProvider i18n={i18n}>
      <RecordView value={value} label="Sample record" />
    </I18nextProvider>,
  );
}

afterEach(() => vi.unstubAllGlobals());

describe("RecordView", () => {
  it("shows an entity as rows, each attribute unwrapped, and no JSON text", async () => {
    const { container } = show(ENTITY);
    const view = screen.getByRole("group", { name: "Sample record" });
    expect(within(view).getByText("pm10")).toBeInTheDocument();
    expect(within(view).getByText("12")).toBeInTheDocument();
    expect(within(view).getByText("GQ · 2026-10-24T08:00:00Z")).toBeInTheDocument();
    expect(within(view).getByText("urn:ngsi-ld:Device:ba-01")).toBeInTheDocument();
    expect(within(view).getByText("Point 17.1077, 48.1486")).toBeInTheDocument();
    expect(within(view).getByText("Bratislava")).toBeInTheDocument();
    expect(view.textContent).not.toContain('{"type":');
    expect(view.textContent).not.toContain('"value"');
    expect(within(view).getByRole("link", { name: /data\.example\.sk/ })).toHaveAttribute("href", "https://data.example.sk/air");
    // A javascript: value stays text.
    expect(within(view).getByText("javascript:alert(1)").closest("a")).toBeNull();
    expect(pageFindings({ namespaces: [], layout: false }).filter((f) => f.startsWith("raw-json"))).toEqual([]);
    await expectNoAxeViolations(container);
  });

  it("folds a nested object and opens it again", async () => {
    show({ outer: { inner: { deep: "x" } } });
    const toggles = screen.getAllByRole("button", { name: en.recordView.fields.replace("{count}", "1") });
    expect(toggles[0]).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByText("inner")).toBeVisible();
    await userEvent.click(toggles[0]);
    expect(toggles[0]).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByText("inner")).toBeNull();
    await userEvent.click(toggles[0]);
    expect(screen.getByText("inner")).toBeVisible();
  });

  it("starts a nested list folded below the first level", async () => {
    render(
      <I18nextProvider i18n={i18n}>
        <Nested value={["a", "b"]} depth={2} />
      </I18nextProvider>,
    );
    const toggle = screen.getByRole("button", { name: en.recordView.items.replace("{count}", "2") });
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    await userEvent.click(toggle);
    expect(screen.getByText("a")).toBeVisible();
  });

  it("copies the record as JSON", async () => {
    const writeText = vi.fn(async () => undefined);
    vi.stubGlobal("navigator", { ...navigator, clipboard: { writeText } });
    show(ENTITY);
    await userEvent.click(screen.getByRole("button", { name: en.recordView.copy }));
    expect(writeText).toHaveBeenCalledWith(JSON.stringify(ENTITY, null, 2));
    expect(await screen.findByRole("button", { name: en.recordView.copied })).toBeInTheDocument();
  });

  it("says when the browser refuses the copy", async () => {
    vi.stubGlobal("navigator", { ...navigator, clipboard: { writeText: vi.fn(async () => Promise.reject(new Error("no"))) } });
    show({ a: 1 });
    await userEvent.click(screen.getByRole("button", { name: en.recordView.copy }));
    expect(await screen.findByText(en.recordView.copyFailed)).toBeInTheDocument();
  });

  it("cuts long text short with a Show all", async () => {
    const long = "a".repeat(400);
    show({ note: long });
    expect(screen.queryByText(long)).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: en.recordView.showAll }));
    expect(screen.getByText(long, { exact: false })).toBeInTheDocument();
  });

  it("lists records, says when a list or record is empty, and shows text as text", () => {
    show([{ a: 1 }, { b: 2 }]);
    expect(screen.getByText(en.recordView.item.replace("{n}", "2"))).toBeInTheDocument();
    show([]);
    expect(screen.getByText(en.recordView.noItems)).toBeInTheDocument();
    show({});
    expect(screen.getByText(en.recordView.noFields)).toBeInTheDocument();
    show("id;name\n1;Stanica");
    expect(screen.getByText(/1;Stanica/)).toBeInTheDocument();
  });
});

describe("cellOf", () => {
  it("unwraps every attribute kind and names a geometry's size", () => {
    expect(cellOf({ type: "LanguageProperty", languageMap: { sk: "Bratislava" } })).toMatchObject({ kind: "nested" });
    expect(cellOf({ type: "ListProperty", valueList: [1, 2] })).toEqual({ kind: "text", text: "1, 2" });
    expect(cellOf({ type: "Property", value: null })).toEqual({ kind: "text", text: "—" });
    expect(cellOf([])).toEqual({ kind: "text", text: "—" });
    // A plain object with a type member is not an attribute.
    expect(cellOf({ type: "Station", name: "x" })).toMatchObject({ kind: "nested" });
    expect(geometryText({ type: "Polygon", coordinates: [[[0, 0], [1, 0], [1, 1], [0, 0]]] })).toBe("Polygon (4)");
    expect(geometryText({ type: "Station" })).toBeUndefined();
  });
});
