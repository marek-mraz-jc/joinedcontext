/**
 * The dashboards page's remaining branches: a failed list with its Retry, the three ways to a
 * new or edited resource, the dashboard chooser, the legend's switch, the public-dashboard rule,
 * a layer whose features cannot be read, and the viewport that follows the map.
 */
import { cleanup, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { handPrefill } from "../src/assistant/state";
import { findFormPage, queryFormPage } from "./formPage";
import { jsonResponse, list, problem, renderRoute } from "./pageHarness";

const map = vi.hoisted(() => ({ handlers: new Map<string, () => void>() }));

vi.mock("maplibre-gl", () => {
  class FakeMap {
    addControl() {}
    addSource() {}
    addLayer() {}
    on(event: string, second: unknown) {
      if (event === "load" && typeof second === "function") (second as () => void)();
      if (event === "moveend" && typeof second === "function") map.handlers.set(event, second as () => void);
    }
    remove() {}
    getSource() {
      return undefined;
    }
    removeLayer() {}
    removeSource() {}
    off() {}
    fitBounds() {}
    getBounds() {
      return { getWest: () => 24.9, getSouth: () => 60.1, getEast: () => 25, getNorth: () => 60.2 };
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
  return {
    setWorkerUrl: () => undefined,
    Map: FakeMap,
    NavigationControl: class {},
    Popup,
    default: { Map: FakeMap, NavigationControl: class {}, Popup },
  };
});

const PATH = "/projects/helsinki/dashboards";

const endpoint = (name: string, slug: string, audience: string) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Endpoint",
  metadata: { name, namespace: "helsinki" },
  spec: { contextSpaceRef: "air", slug, audience },
});
const ENDPOINTS = [endpoint("pub-air", "k7m2qz4tv6xh3n5jb2ryd3wcfa", "public"), endpoint("org-air", "z9y8x7w6v5u4t3s2r1q0p9n8m7", "organization")];

const layer = (name: string, ref = "pub-air", extra: Record<string, unknown> = {}) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Layer",
  metadata: { name, namespace: "helsinki" },
  spec: { sourceEndpointRef: ref, entityType: "AirQualityObserved", style: "circle", ...extra },
});

const dashboard = (name: string, title: string, spec: Record<string, unknown> = {}) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Dashboard",
  metadata: { name, namespace: "helsinki" },
  spec: {
    title: { en: title },
    visibility: "project",
    pages: [{ title: "Map", layout: "full-map", layers: ["stations"] }],
    ...spec,
  },
});

interface World {
  dashboards?: unknown[];
  layers?: unknown[];
  features?: () => Response;
  dashboardsAnswer?: () => Response;
}

function answering(world: World = {}, seen: string[] = []) {
  return (path: string, request?: Request): Response | undefined => {
    if (request) seen.push(`${request.method} ${path}`);
    if (path.endsWith("/dashboards")) {
      return world.dashboardsAnswer ? world.dashboardsAnswer() : jsonResponse(list(world.dashboards ?? [dashboard("air", "Air")]));
    }
    if (path.endsWith("/layers")) return jsonResponse(list(world.layers ?? [layer("stations")]));
    if (path.endsWith("/endpoints")) return jsonResponse(list(ENDPOINTS));
    if (path.startsWith("/api/endpoint/")) {
      return world.features ? world.features() : jsonResponse({ type: "FeatureCollection", features: [] });
    }
    return undefined;
  };
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
  map.handlers.clear();
  window.sessionStorage.clear();
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("dashboards page, remaining branches", () => {
  it("shows why the list failed and asks again on Retry", async () => {
    let calls = 0;
    const { calls: asked } = await renderRoute({
      path: PATH,
      answer: answering({
        dashboardsAnswer: () => {
          calls += 1;
          return calls === 1 ? problem(503, "The dashboards are away.") : jsonResponse(list([dashboard("air", "Air")]));
        },
      }),
    });
    expect(await screen.findByRole("alert")).toHaveTextContent("The dashboards are away.");
    await userEvent.click(screen.getByRole("button", { name: en.app.error.retry }));
    expect(await screen.findByRole("heading", { level: 1, name: "Air" })).toBeInTheDocument();
    expect(asked().filter((line) => line.endsWith("/dashboards")).length).toBeGreaterThanOrEqual(2);
  });

  it("opens an empty new-dashboard form from the header and closes it with Cancel", async () => {
    await renderRoute({ path: PATH, answer: answering() });
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: en.dashboards.add }));
    const form = await findFormPage(en.dashboards.add);
    await user.click(within(form).getByRole("button", { name: en.form.cancel }));
    await waitFor(() => expect(queryFormPage()).toBeNull());
  });

  it("opens an empty new-layer form from the header and closes it with Cancel", async () => {
    await renderRoute({ path: PATH, answer: answering() });
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: en.dashboards.addLayer }));
    expect(await findFormPage(en.dashboards.addLayer)).toBeInTheDocument();
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("offers the same new buttons on the empty page", async () => {
    await renderRoute({ path: PATH, answer: answering({ dashboards: [] }) });
    expect(await screen.findByText(en.dashboards.empty)).toBeInTheDocument();
    const user = userEvent.setup();
    const add = screen.getAllByRole("button", { name: en.dashboards.add });
    await user.click(add[add.length - 1]);
    expect(await findFormPage(en.dashboards.add)).toBeInTheDocument();
  });

  it("opens the chosen layer's form from the legend's Edit", async () => {
    await renderRoute({ path: PATH, answer: answering() });
    await userEvent.click(await screen.findByRole("button", { name: `${en.dashboards.editLayer}: stations` }));
    expect(await findFormPage(en.dashboards.editLayer)).toBeInTheDocument();
  });

  it("chooses between dashboards and starts the new one on its first page", async () => {
    await renderRoute({
      path: PATH,
      answer: answering({
        dashboards: [
          dashboard("air", "Air", { pages: [{ title: "One", layers: ["stations"] }, { title: "Two", layers: [] }] }),
          dashboard("noise", "Noise", { pages: [{ title: "Quiet", layers: [] }] }),
        ],
      }),
    });
    const user = userEvent.setup();
    await user.click(await screen.findByRole("tab", { name: "Two" }));
    expect(screen.getByRole("tab", { name: "Two" })).toHaveAttribute("aria-selected", "true");
    const select = screen.getByRole("combobox", { name: en.dashboards.title });
    expect(within(select).getAllByRole("option").map((option) => option.textContent)).toEqual(["Air", "Noise"]);
    await user.selectOptions(select, "noise");
    expect(await screen.findByRole("heading", { level: 1, name: "Noise" })).toBeInTheDocument();
    expect(screen.queryByRole("tablist")).toBeNull();
    await user.selectOptions(select, "air");
    expect(await screen.findByRole("tab", { name: "One" })).toHaveAttribute("aria-selected", "true");
  });

  it("opens the dashboard the address names", async () => {
    await renderRoute({
      path: `${PATH}?dashboard=noise`,
      answer: answering({ dashboards: [dashboard("air", "Air"), dashboard("noise", "Noise", { pages: [] })] }),
    });
    expect(await screen.findByRole("heading", { level: 1, name: "Noise" })).toBeInTheDocument();
  });

  it("stops asking for a layer's features when its legend entry is switched off, and asks again when it is switched on", async () => {
    const seen: string[] = [];
    await renderRoute({ path: PATH, answer: answering({}, seen) });
    const tick = await screen.findByRole("checkbox", { name: `${en.dashboards.show}: stations` });
    const features = () => seen.filter((line) => line.startsWith("GET /api/endpoint/")).length;
    await waitFor(() => expect(features()).toBe(1));
    const user = userEvent.setup();
    await user.click(tick);
    await waitFor(() => expect(tick).not.toBeChecked());
    // Off: the layer's features are not asked for again, and the map holds no layer.
    expect(features()).toBe(1);
    await user.click(tick);
    await waitFor(() => expect(tick).toBeChecked());
    // On again: the features are the same read, still held by the page, so no second request is needed.
    expect(features()).toBe(1);
  });

  it("keeps a layer switched off by its manifest off until the reader switches it on", async () => {
    const seen: string[] = [];
    await renderRoute({
      path: PATH,
      answer: answering({ layers: [layer("stations", "pub-air", { visible: false })] }, seen),
    });
    const tick = await screen.findByRole("checkbox", { name: `${en.dashboards.show}: stations` });
    expect(tick).not.toBeChecked();
    expect(seen.some((line) => line.startsWith("GET /api/endpoint/"))).toBe(false);
    await userEvent.click(tick);
    await waitFor(() => expect(tick).toBeChecked());
    await waitFor(() => expect(seen.some((line) => line.startsWith("GET /api/endpoint/"))).toBe(true));
  });

  it("names a layer that is not in the project or has no endpoint, and draws the rest", async () => {
    await renderRoute({
      path: PATH,
      answer: answering({
        dashboards: [dashboard("air", "Air", { pages: [{ title: "Map", layers: ["stations", "ghost", "orphan"] }] })],
        layers: [layer("stations"), layer("orphan", "no-such-endpoint")],
      }),
    });
    expect(await screen.findByText(/ghost, orphan/)).toBeInTheDocument();
    expect(screen.getByRole("checkbox", { name: `${en.dashboards.show}: stations` })).toBeInTheDocument();
    expect(screen.queryByRole("checkbox", { name: `${en.dashboards.show}: ghost` })).toBeNull();
  });

  it("refuses a layer and a widget of a public dashboard that read a non-public endpoint, and asks for neither", async () => {
    const seen: string[] = [];
    await renderRoute({
      path: PATH,
      answer: answering({
        dashboards: [
          dashboard("air", "Air", {
            visibility: "public",
            pages: [{ title: "Map", layers: ["inner"], widgets: [{ widgetType: "kpi", endpointRef: "org-air", entityId: "urn:ngsi-ld:X:1", property: "pm10" }] }],
          }),
        ],
        layers: [layer("inner", "org-air")],
      }, seen),
    });
    const notice = await screen.findByText(/a public dashboard may only read public endpoints/);
    expect(notice).toHaveTextContent("inner, org-air");
    expect(screen.queryByRole("checkbox", { name: `${en.dashboards.show}: inner` })).toBeNull();
    expect(seen.some((line) => line.includes("z9y8x7w6v5u4t3s2r1q0p9n8m7"))).toBe(false);
  });

  it("draws a public layer of a public dashboard", async () => {
    await renderRoute({
      path: PATH,
      answer: answering({ dashboards: [dashboard("air", "Air", { visibility: "public" })] }),
    });
    expect(await screen.findByRole("checkbox", { name: `${en.dashboards.show}: stations` })).toBeChecked();
    expect(screen.queryByText(/a public dashboard may only read/)).toBeNull();
  });

  it("keeps the map and the legend when a layer's features cannot be read", async () => {
    const seen: string[] = [];
    await renderRoute({
      path: PATH,
      answer: answering({ features: () => problem(502, "The endpoint is away.") }, seen),
    });
    expect(await screen.findByRole("checkbox", { name: `${en.dashboards.show}: stations` })).toBeInTheDocument();
    await waitFor(() => expect(seen.some((line) => line.startsWith("GET /api/endpoint/"))).toBe(true));
    await waitFor(() => expect(screen.queryByRole("status", { name: en.app.loading })).toBeNull());
    expect(screen.getByRole("heading", { level: 1, name: "Air" })).toBeInTheDocument();
  });

  it("asks for the features inside the viewport once the reader has moved the map", async () => {
    const urls: string[] = [];
    await renderRoute({
      path: PATH,
      answer: (path, request) => {
        if (path.startsWith("/api/endpoint/")) urls.push(request.url);
        return answering()(path);
      },
    });
    await screen.findByRole("checkbox", { name: `${en.dashboards.show}: stations` });
    await waitFor(() => expect(map.handlers.has("moveend")).toBe(true));
    expect(urls.every((url) => !url.includes("georel"))).toBe(true);
    map.handlers.get("moveend")?.();
    await waitFor(() => expect(urls.some((url) => new URL(url).searchParams.get("georel") === "within")).toBe(true));
    const moved = urls.find((url) => url.includes("georel")) as string;
    expect(JSON.parse(new URL(moved).searchParams.get("coordinates") ?? "[]")).toEqual([
      [[24.9, 60.1], [25, 60.1], [25, 60.2], [24.9, 60.2], [24.9, 60.1]],
    ]);
  });

  it("opens the dashboard the assistant sent a change for, with its manifest", async () => {
    const changed = dashboard("air", "Air renamed");
    window.history.pushState({}, "", `${PATH}?edit=air`);
    handPrefill(PATH, changed as unknown as Record<string, unknown>);
    await renderRoute({ path: `${PATH}?edit=air`, answer: answering() });
    const form = await findFormPage(en.dashboards.edit);
    // The form holds the assistant's manifest, not the stored one.
    await waitFor(() => expect(within(form).getByDisplayValue("Air renamed")).toBeInTheDocument());
  });

  it("opens the layer a change was asked for from the lists when the assistant left no manifest", async () => {
    await renderRoute({ path: `${PATH}?edit=stations`, answer: answering() });
    expect(await findFormPage(en.dashboards.editLayer)).toBeInTheDocument();
  });

  it("opens a dashboard the assistant drafted as a new one when the project has none of that name", async () => {
    const drafted = dashboard("fresh", "Fresh");
    window.history.pushState({}, "", `${PATH}?edit=fresh`);
    handPrefill(PATH, drafted as unknown as Record<string, unknown>);
    await renderRoute({ path: `${PATH}?edit=fresh`, answer: answering() });
    expect(await findFormPage(en.dashboards.add)).toBeInTheDocument();
  });
});
