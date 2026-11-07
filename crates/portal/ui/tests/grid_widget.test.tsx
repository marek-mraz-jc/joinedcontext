/**
 * UI-15, UI-16 (T-2137): the `grid` widget of a Dashboard.
 *
 * Two things are worth a test here and the file is small because neither is about drawing a
 * table: the widget reads through its own `endpointRef` and `entityType` whatever the manifest's
 * `grid` object says, so a dashboard committed by one team cannot point a grid at another team's
 * endpoint; and a `grid` an older platform accepted says on the page why it cannot be drawn
 * rather than disappearing.
 */
import { screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { GridWidget } from "../src/components/dashboards/GridWidget";
import { expectNoAxeViolations, renderPart } from "./page_contract";

const TYPE = "BikeHireDockingStation";

const ENTITIES = [
  {
    id: "urn:ngsi-ld:BikeHireDockingStation:hel:helsinki:001",
    type: TYPE,
    availableBikeNumber: { type: "Property", value: 5, unitCode: "C62" },
  },
];

/** Every NGSI-LD read the grid makes, in the order it made them. */
function reads(): string[] {
  const mock = globalThis.fetch as unknown as { mock: { calls: unknown[][] } };
  return mock.mock.calls.map((call) => String(call[0]));
}

function show(config?: Record<string, unknown>) {
  return renderPart(
    <GridWidget
      project="helsinki"
      slug="helsinki-bikes"
      type={TYPE}
      config={config}
      title="Docking stations"
    />,
    {
      answer: (url) =>
        url.pathname.includes("/ngsi-ld/v1/entities")
          ? new Response(JSON.stringify(ENTITIES), {
              status: 200,
              headers: { "Content-Type": "application/json", "NGSILD-Results-Count": "1" },
            })
          : undefined,
    },
  );
}

afterEach(() => {
  window.localStorage.clear();
  vi.restoreAllMocks();
});

describe("a dashboard's grid widget", () => {
  it("shows the widget's title and the entities its endpoint answers with", async () => {
    show({ columns: [{ attr: "availableBikeNumber", label: "Bikes" }] });
    expect(screen.getByRole("heading", { name: "Docking stations" })).toBeInTheDocument();
    await waitFor(() => expect(screen.getByText("5")).toBeInTheDocument());
    expect(reads().some((url) => url.startsWith("/api/endpoint/helsinki-bikes/ngsi-ld/v1/entities"))).toBe(true);
  });

  it("reads through its own endpoint and type, whatever the manifest's grid names", async () => {
    show({
      // A dashboard author typed another team's endpoint and another type into the widget's
      // config; neither may decide what is read.
      source: { kind: "endpoint", slug: "payroll" },
      type: "Salary",
      columns: [{ attr: "availableBikeNumber" }],
    });
    await waitFor(() => expect(reads().length).toBeGreaterThan(0));
    const entities = reads().filter((url) => url.includes("/ngsi-ld/v1/entities"));
    expect(entities.every((url) => url.startsWith("/api/endpoint/helsinki-bikes/"))).toBe(true);
    expect(entities.some((url) => url.includes(`type=${TYPE}`))).toBe(true);
    expect(reads().some((url) => url.includes("payroll") || url.includes("Salary"))).toBe(false);
  });

  it("says why a grid it cannot draw is not drawn, and draws no table", async () => {
    show({ columns: [{ attr: "" }] });
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("columns/0/attr");
    expect(alert).toHaveTextContent("must be a non-empty string");
    expect(alert.textContent).toContain(en.dashboards.widget.badGrid.split("{")[0].trim());
    expect(screen.queryByRole("grid")).not.toBeInTheDocument();
  });

  it("has no axe violation, drawn and refused", async () => {
    const drawn = show({ columns: [{ attr: "availableBikeNumber", label: "Bikes" }] });
    await waitFor(() => expect(screen.getByText("5")).toBeInTheDocument());
    await expectNoAxeViolations(drawn.container);
    drawn.unmount();

    const refused = show({ columns: [{ attr: "" }] });
    await screen.findByRole("alert");
    await expectNoAxeViolations(refused.container);
  });
});

it("keeps its strings in the Portal's catalogue", async () => {
  await i18n.changeLanguage("sk");
  expect(i18n.t("dashboards.widget.badGrid", { reason: "x" })).not.toMatch(/^dashboards\./);
  await i18n.changeLanguage("en");
});
