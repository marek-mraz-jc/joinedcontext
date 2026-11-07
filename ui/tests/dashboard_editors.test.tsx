/** T-0528: the Dashboard and Layer editors, the legend and the viewport query (UI-17, UI-18, UI-22). */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";
import { rememberPrefill } from "../src/assistant/state";
import { geojsonUrl } from "../src/routes/DashboardsPage";
import {
  dashboardFromManifest,
  dashboardToManifest,
  layerFromManifest,
  layerToManifest,
} from "../src/pages/dashboards/editors";
import { dashboardSchema } from "../src/schemas/kinds";
import type { JsonSchema } from "../src/components/forms/types";
import { answeringChecks, checksSoFar } from "./checks";
import { findFormPage } from "./formPage";

const calls = vi.hoisted(() => ({
  sources: [] as { id: string; source: Record<string, unknown> }[],
  removed: [] as string[],
  moveEnd: null as null | (() => void),
}));

vi.mock("maplibre-gl", () => {
  class FakeMap {
    addControl() {}
    addSource(id: string, source: Record<string, unknown>) {
      calls.sources.push({ id, source });
    }
    addLayer() {}
    on(event: string, second: unknown) {
      if (event === "load" && typeof second === "function") {
        (second as () => void)();
      }
      if (event === "moveend" && typeof second === "function") {
        calls.moveEnd = second as () => void;
      }
    }
    remove() {}
    getSource() {
      return undefined;
    }
    removeLayer(id: string) {
      calls.removed.push(id);
    }
    removeSource() {}
    off() {}
    fitBounds() {}
    getBounds() {
      return { getWest: () => 24.9, getSouth: () => 60.1, getEast: () => 25.0, getNorth: () => 60.2 };
    }
  }
  const Popup = class {
    setLngLat() {
      return this;
    }
    setHTML() {
      return this;
    }
    addTo() {
      return this;
    }
  };
  return { Map: FakeMap, setWorkerUrl: () => undefined, NavigationControl: class {}, Popup, default: { Map: FakeMap, NavigationControl: class {}, Popup } };
});

const IDENTITY = { subject: "b7c1e0f4", username: "jana.kovacova", name: "Jana Kováčová", roles: [] };
const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });

const MODEL = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "DataModel",
  metadata: { name: "bikes", namespace: "helsinki" },
  spec: {
    version: "1.0.0",
    linkml: [
      "id: https://hel.fi/models/bikes",
      "name: bikes",
      "classes:",
      "  BikeHireDockingStation:",
      "    slots: [id, availableBikeNumber, name]",
      "slots:",
      "  id: {}",
      "  availableBikeNumber: { range: integer, minimum_value: 0 }",
      "  name: { range: string }",
      "",
    ].join("\n"),
  },
};
const SPACE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ContextSpace",
  metadata: { name: "mobility", namespace: "helsinki" },
  spec: { dataModelRef: "bikes" },
};
const ENDPOINT = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Endpoint",
  metadata: { name: "helsinki-bikes", namespace: "helsinki" },
  spec: { contextSpaceRef: "mobility", slug: "k7m2qz4tv6xh3n5jb2ryd3wcfa", audience: "public", enabledRepresentations: ["ngsi-ld"] },
};
const LAYER = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Layer",
  metadata: { name: "bikes", namespace: "helsinki" },
  spec: {
    sourceEndpointRef: "helsinki-bikes",
    entityType: "BikeHireDockingStation",
    style: "circle",
    colorBy: { property: "availableBikeNumber", domain: [0, 20] },
    popupProperties: ["name", "availableBikeNumber"],
  },
};
const DASHBOARD = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Dashboard",
  metadata: { name: "bikes", namespace: "helsinki" },
  spec: { title: { en: "Bikes" }, visibility: "project", pages: [{ layout: "full-map", layers: ["bikes"] }] },
};
const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-77aa11bb", namespace: "helsinki" },
  status: { lane: "green", phase: "PendingApproval", plan: { update: 1 } },
};
const FEATURES = {
  type: "FeatureCollection",
  features: [{ type: "Feature", geometry: { type: "Point", coordinates: [24.95, 60.17] }, properties: { name: "Kamppi", availableBikeNumber: 7 } }],
};

function renderDashboards(options: { writeStatus?: number; writeBody?: unknown; drafts?: Record<string, unknown> } = {}) {
  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    const request = input as Request;
    const url = typeof input === "string" ? new URL(input) : new URL(request.url);
    const json = (body: unknown, status = 200) =>
      Promise.resolve(new Response(JSON.stringify(body), { status, headers: { "Content-Type": status >= 400 ? "application/problem+json" : "application/json" } }));
    if (url.pathname.includes("/api/endpoint/")) {
      return json(FEATURES);
    }
    if (url.pathname.endsWith("/auth/me")) {
      return json(IDENTITY);
    }
    // A draft save is taken, as the server takes one; the write under test is the proposal.
    if (request.method === "PUT" && url.pathname.includes("/drafts/")) {
      return json({ version: 1, manifest: null, verdict: null, touchedBy: "anna", touchedKind: "person" });
    }
    if (request.method !== "GET") {
      return json(options.writeBody ?? CHANGE, options.writeStatus ?? 202);
    }
    const draft = options.drafts?.[url.pathname];
    if (url.pathname.includes("/drafts/")) {
      return draft ? json(draft) : json({ title: "Not Found", status: 404 }, 404);
    }
    if (url.pathname.endsWith("/dashboards")) {
      return json(list([DASHBOARD]));
    }
    if (url.pathname.endsWith("/layers")) {
      return json(list([LAYER]));
    }
    if (url.pathname.endsWith("/endpoints")) {
      return json(list([ENDPOINT]));
    }
    if (url.pathname.endsWith("/spaces")) {
      return json(list([SPACE]));
    }
    if (url.pathname.endsWith("/datamodels")) {
      return json(list([MODEL]));
    }
    return json(list([]));
  });
  vi.stubGlobal("fetch", fetchMock);
  vi.stubGlobal("fetch", answeringChecks(globalThis.fetch));
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <App />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return fetchMock;
}

function writes(fetchMock: ReturnType<typeof vi.fn>): Request[] {
  return fetchMock.mock.calls
    .map((call) => call[0] as Request)
    .filter(
      (request) =>
        request instanceof Request &&
        request.method !== "GET" &&
        // The form's own draft, shared with the other windows; it writes nothing to Git (AG-61).
        !request.url.includes("/drafts/"),
    );
}

/**
 * Check, then Propose once the verdict is green: a form that holds a draft proposes only what its
 * check passed (PF-57, T-0779), and the dashboard and layer forms have their Check since T-2731.
 */
async function checkAndPropose(dialog: HTMLElement): Promise<void> {
  await userEvent.click(within(dialog).getByRole("button", { name: en.form.check }));
  const propose = within(dialog).getByRole("button", { name: en.dashboards.propose });
  await waitFor(() => expect(propose).not.toHaveAttribute("aria-disabled", "true"));
  await userEvent.click(propose);
}

async function openLayerEditor() {
  await userEvent.click(await screen.findByRole("button", { name: `${en.dashboards.editLayer}: bikes` }));
  return findFormPage();
}

/**
 * The dashboard's own Edit, which lives in the header's actions menu since T-2288: the header
 * keeps the chooser and the two "new" buttons in the open, and everything that acts on the
 * dashboard on screen is one menu behind the ⋯ button.
 */
async function openDashboardEditor(): Promise<void> {
  await userEvent.click(await screen.findByRole("button", { name: /More actions for/ }));
  await userEvent.click(await screen.findByRole("menuitem", { name: en.resourceEdit.button }));
}

describe("dashboard editors", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/projects/helsinki/dashboards");
    calls.sources.length = 0;
    calls.removed.length = 0;
    calls.moveEnd = null;
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  // T-2758: the layer form showed no hint under any field; its arrangement is found by its kind.
  it("says under the layer's fields what each one is for", async () => {
    renderDashboards();
    const dialog = await openLayerEditor();
    expect(within(dialog).getByText(/The short name of this layer/)).toBeInTheDocument();
    expect(within(dialog).getByLabelText(/^Name/)).toHaveAccessibleDescription(/The short name of this layer/);
  });

  it("writes spec.filter.q from a filter row of the layer editor (UI-18, UI-33)", async () => {
    const fetchMock = renderDashboards();
    const dialog = await openLayerEditor();

    // The rows come from the endpoint's space model: the type is already chosen, the
    // attribute list knows availableBikeNumber is an integer.
    expect(within(dialog).getByLabelText(en.entities.type)).toHaveValue("BikeHireDockingStation");
    await userEvent.click(within(dialog).getByRole("button", { name: en.entities.addFilter }));
    await userEvent.selectOptions(within(dialog).getByRole("combobox", { name: en.entities.attribute }), "availableBikeNumber");
    await userEvent.selectOptions(within(dialog).getByRole("combobox", { name: en.entities.operator }), ">");
    await userEvent.type(within(dialog).getByRole("spinbutton", { name: en.entities.value }), "0");
    await waitFor(() => expect(within(dialog).getByLabelText(en.entities.q)).toHaveValue("availableBikeNumber>0"));

    await checkAndPropose(dialog);
    await waitFor(() => expect(writes(fetchMock)).toHaveLength(1));
    const request = writes(fetchMock)[0];
    expect(request.method).toBe("PUT");
    expect(new URL(request.url).pathname).toBe("/api/v1/projects/helsinki/layers/bikes");
    const body = (await request.clone().json()) as { kind: string; spec: Record<string, unknown> };
    expect(body.kind).toBe("Layer");
    expect(body.spec.entityType).toBe("BikeHireDockingStation");
    expect(body.spec.filter).toEqual({ q: "availableBikeNumber>0" });
    expect(body.spec.colorBy).toEqual({ property: "availableBikeNumber", domain: [0, 20] });
    expect(body.spec.visible).toBeUndefined();
    // Checked before it was proposed (PF-57, T-0956).
    expect(checksSoFar().some((check) => check.includes("/layers"))).toBe(true);
  });

  it("opens a layer's editor on the change the assistant made, and sends nothing until proposed (AG-77)", async () => {
    rememberPrefill("/projects/helsinki/dashboards?edit=bikes", { ...LAYER, spec: { ...LAYER.spec, style: "heatmap" } });
    window.history.pushState({}, "", "/projects/helsinki/dashboards?edit=bikes&draft=bikes");
    const fetchMock = renderDashboards();

    const dialog = await findFormPage();
    expect(writes(fetchMock)).toHaveLength(0);
    await checkAndPropose(dialog);
    await waitFor(() => expect(writes(fetchMock)).toHaveLength(1));
    const request = writes(fetchMock)[0];
    expect(new URL(request.url).pathname).toBe("/api/v1/projects/helsinki/layers/bikes");
    expect(((await request.clone().json()) as { spec: { style: string } }).spec.style).toBe("heatmap");
  });

  it("opens a new layer the assistant drafted, by the draft's name alone (`?draft=`, AG-73)", async () => {
    window.history.pushState({}, "", "/projects/helsinki/dashboards?draft=night-bikes");
    const fetchMock = renderDashboards({
      drafts: {
        "/api/v1/projects/helsinki/drafts/Layer/night-bikes": {
          project: "helsinki",
          kind: "Layer",
          name: "night-bikes",
          manifest: { ...LAYER, metadata: { name: "night-bikes", namespace: "helsinki" } },
          verdict: null,
          touchedBy: "jana.kovacova",
          touchedKind: "assistant",
          version: 1,
          updatedAt: "2026-09-22T08:00:00Z",
        },
      },
    });

    const form = await findFormPage(en.dashboards.addLayer);
    await waitFor(() => expect(within(form).getByDisplayValue("night-bikes")).toBeInTheDocument());
    expect(writes(fetchMock)).toHaveLength(0);
  });

  it("opens the dashboard the assistant drafted as new, and proposes it with its drafted layer as one import (T-0739)", async () => {
    const drafted = {
      ...DASHBOARD,
      metadata: { name: "stations-map", namespace: "helsinki" },
      spec: { title: "Stations", visibility: "project", pages: [{ layout: "full-map", layers: ["bikes", "stations"] }] },
    };
    const layer = { ...LAYER, metadata: { name: "stations", namespace: "helsinki" } };
    rememberPrefill("/projects/helsinki/dashboards?edit=stations-map", drafted);
    window.history.pushState({}, "", "/projects/helsinki/dashboards?edit=stations-map&draft=stations-map");
    const fetchMock = renderDashboards({
      drafts: {
        "/api/v1/projects/helsinki/drafts/Layer/stations": {
          project: "helsinki",
          kind: "Layer",
          name: "stations",
          manifest: layer,
          verdict: null,
          touchedBy: "jana.kovacova",
          touchedKind: "assistant",
          version: 1,
          updatedAt: "2026-09-15T08:00:00Z",
        },
      },
    });

    const dialog = await findFormPage(en.dashboards.add);
    expect(writes(fetchMock)).toHaveLength(0);
    await waitFor(() =>
      expect(
        fetchMock.mock.calls.some((call) => {
          // The layer's GeoJSON is read with a plain URL string; the draft goes through the client.
          const input = call[0] as RequestInfo | URL;
          const url = input instanceof Request ? input.url : String(input);
          return new URL(url, window.location.origin).pathname.endsWith("/drafts/Layer/stations");
        }),
      ).toBe(true),
    );
    await checkAndPropose(dialog);
    await waitFor(() => expect(writes(fetchMock)).toHaveLength(1));
    const request = writes(fetchMock)[0];
    expect(request.method).toBe("POST");
    expect(new URL(request.url).pathname).toBe("/api/v1/projects/helsinki/import");
    const body = (await request.clone().json()) as { manifests: { kind: string; metadata: { name: string } }[] };
    // The existing layer stays out of the bundle; the drafted one goes with the dashboard.
    expect(body.manifests.map((m) => `${m.kind}/${m.metadata.name}`)).toEqual(["Layer/stations", "Dashboard/stations-map"]);
  });

  it("shows the server's reason when a public dashboard would read a private endpoint (UI-19)", async () => {
    renderDashboards({
      writeStatus: 400,
      writeBody: {
        type: "about:blank",
        title: "Bad Request",
        status: 400,
        detail: "a public dashboard may only read public endpoints: layer bikes reads helsinki-bikes whose audience is organization (UI-19)",
      },
    });
    await openDashboardEditor();
    const dialog = await findFormPage();
    await userEvent.selectOptions(within(dialog).getByLabelText(new RegExp(`^${en.dashboards.field.visibility}`)), en.choice.visibility.public);
    await checkAndPropose(dialog);

    expect(await within(dialog).findByText(/whose audience is organization/)).toBeInTheDocument();
  });

  it("switches a layer off from the legend and asks the endpoint for the viewport after a move (UI-22)", async () => {
    renderDashboards();
    await waitFor(() => expect(calls.sources).toHaveLength(1));
    expect(new URL(String(calls.sources[0].source.data ?? "http://x/"), window.location.origin).searchParams.get("georel")).toBeNull();

    await userEvent.click(await screen.findByRole("checkbox", { name: `${en.dashboards.show}: bikes` }));
    await waitFor(() => expect(calls.removed).toContain("bikes"));
  });

  it("spells the viewport as the gateway's georel, geometry and coordinates", () => {
    const url = new URL(geojsonUrl("slug", { entityType: "T" }, [24.9, 60.1, 25, 60.2]), "http://x");
    expect(url.searchParams.get("georel")).toBe("within");
    expect(url.searchParams.get("geometry")).toBe("Polygon");
    expect(JSON.parse(url.searchParams.get("coordinates") ?? "")).toEqual([[[24.9, 60.1], [25, 60.1], [25, 60.2], [24.9, 60.2], [24.9, 60.1]]]);
    // A manifest's own geoQ wins over the viewport.
    const fixed = new URL(geojsonUrl("slug", { filter: { geoQ: "georel=near;maxDistance==2000;geometry=Point;coordinates=[25,60]" } }, [0, 0, 1, 1]), "http://x");
    expect(fixed.searchParams.get("georel")).toBe("near;maxDistance==2000");
    expect(fixed.searchParams.get("coordinates")).toBe("[25,60]");
  });

  it("reads a layer from the endpoint's NGSI-LD API with plain values, never a file (T-4600)", () => {
    const url = new URL(geojsonUrl("k7m2/../x", { entityType: "T" }), "http://x");
    expect(url.pathname).toBe("/api/endpoint/k7m2%2F..%2Fx/ngsi-ld/v1/entities");
    expect(url.searchParams.get("options")).toBe("keyValues");
    expect(url.searchParams.get("type")).toBe("T");
    expect(url.searchParams.get("georel")).toBeNull();
  });

  it("round-trips a layer through its form without writing the visible default", () => {
    const form = layerFromManifest(LAYER);
    expect(form.visible).toBe(true);
    const manifest = layerToManifest("helsinki", form) as { spec: Record<string, unknown> };
    expect(manifest.spec).toEqual(LAYER.spec);
    const off = layerToManifest("helsinki", { ...form, visible: false }) as { spec: Record<string, unknown> };
    expect(off.spec.visible).toBe(false);
  });

  it("writes the dashboard form to a draft and still proposes the manifest itself (T-0791, AG-61)", async () => {
    const fetchMock = renderDashboards();
    await openDashboardEditor();
    const dialog = await findFormPage();
    await userEvent.selectOptions(
      within(dialog).getByLabelText(new RegExp(`^${en.dashboards.field.visibility}`)),
      en.choice.visibility.organization,
    );

    await waitFor(() => {
      const drafted = fetchMock.mock.calls
        .map((call) => call[0] as Request)
        .find((request) => request.method === "PUT" && request.url.includes("/drafts/Dashboard/bikes"));
      expect(drafted).toBeDefined();
    });

    await checkAndPropose(dialog);

    // `propose_draft` knows no dashboards, so the proposal is the manifest and not a draft
    // reference: a `draft` in the body would be a 400 (src/api/mutate.rs).
    await waitFor(() => expect(writes(fetchMock)).toHaveLength(1));
    const body = (await writes(fetchMock)[0].clone().json()) as Record<string, unknown>;
    expect(body.kind).toBe("Dashboard");
    expect(body.draft).toBeUndefined();
    // Checked before it was proposed (PF-57, T-0956).
    expect(checksSoFar().some((check) => check.includes("/dashboards"))).toBe(true);
  });
});

/**
 * T-1440, UI-71, SDK-30: a Dashboard may place the entity grid as a widget, configured by the same
 * object the explorer and a generated application are configured by. The form offers it, and what
 * the person configured reaches the manifest unchanged.
 */
describe("the grid widget of a dashboard", () => {
  it("is one of the widget types the form offers, with the published configuration under it", () => {
    const schema = dashboardSchema((key) => key, ["air-quality-stations"]);
    const pages = schema.properties?.pages as { items?: { properties?: Record<string, JsonSchema> } };
    const widget = pages.items?.properties?.widgets as { items?: { properties?: Record<string, JsonSchema> } };
    const properties = widget.items?.properties ?? {};
    expect((properties.widgetType as { oneOf?: { const: string }[] }).oneOf?.map((option) => option.const)).toEqual(["temporal-chart", "bar-chart", "histogram", "grid"]);
    expect(properties.entityType).toBeDefined();
    const grid = properties.grid as { properties?: Record<string, unknown> };
    // The grid's own configuration, and never the two fields the widget decides for it.
    expect(Object.keys(grid.properties ?? {})).toContain("columns");
    expect(Object.keys(grid.properties ?? {})).toContain("pageSize");
    expect(Object.keys(grid.properties ?? {})).not.toContain("source");
    expect(Object.keys(grid.properties ?? {})).not.toContain("type");
    // Nor the second source of the explorer's comparison: a dashboard manifest names one endpoint,
    // and jc_core's GridConfig refuses the field outright, so the form would draw a manifest the
    // platform rejects.
    expect(Object.keys(grid.properties ?? {})).not.toContain("compareWith");
    // An App's computed column (T-6384) is no field of jc_core's GridColumn either.
    const columns = (grid.properties ?? {}).columns as { items?: { properties?: Record<string, unknown> } };
    expect(Object.keys(columns.items?.properties ?? {})).toContain("attr");
    expect(Object.keys(columns.items?.properties ?? {})).not.toContain("formula");
  });

  it("round-trips a configured grid through the manifest unchanged", () => {
    const grid = {
      columns: [{ attr: "pm10", format: "number" }],
      pageSize: 25,
      history: { enabled: true },
      mode: "edit",
      editableAttrs: ["pm10"],
    };
    const form = {
      name: "air-quality-overview",
      title: "Air quality",
      visibility: "project",
      pages: [
        {
          title: "Data",
          layout: "grid-2x2",
          widgets: [
            {
              widgetType: "grid",
              endpointRef: "ep-air-quality",
              entityType: "AirQualityObserved",
              grid,
            },
          ],
        },
      ],
    };
    const manifest = dashboardToManifest("helsinki", form);
    const spec = manifest.spec as { pages: { widgets: { grid: unknown; entityType: string }[] }[] };
    expect(spec.pages[0].widgets[0].entityType).toBe("AirQualityObserved");
    expect(spec.pages[0].widgets[0].grid).toEqual(grid);
    expect(dashboardFromManifest(manifest).pages).toEqual(form.pages);
  });
});
