/**
 * T-1790: the dashboards page meets the UI contract (UI-01, UI-11, UI-15, UI-16, UI-44, PF-50).
 *
 * Its page switcher was a row of hand-made `role="tab"` buttons: every one a tab stop, no arrow
 * keys, and no panel named by the tab that selects it. Its legend ticked layers with a raw
 * checkbox. `dashboard_pages.test.tsx` holds what the pages and the widgets do; these cases are
 * the contract around them.
 */
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import {
  expectAxeClean,
  expectOneH1,
  jsonResponse,
  list,
  LOCALES,
  OTHER_BRAND,
  problem,
  renderRoute,
} from "./pageHarness";

vi.mock("maplibre-gl", () => {
  class FakeMap {
    addControl() {}
    addSource() {}
    addLayer() {}
    on(event: string, second: unknown) {
      if (event === "load" && typeof second === "function") {
        (second as () => void)();
      }
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

const ENDPOINT = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Endpoint",
  metadata: { name: "helsinki-air", namespace: "helsinki" },
  spec: { contextSpaceRef: "air", slug: "k7m2qz4tv6xh3n5jb2ryd3wcfa", audience: "public" },
};

const layer = (name: string) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Layer",
  metadata: { name, namespace: "helsinki" },
  spec: { sourceEndpointRef: "helsinki-air", entityType: "AirQualityObserved", style: "circle" },
});

const dashboard = (over: Record<string, unknown> = {}) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Dashboard",
  metadata: { name: "air", namespace: "helsinki" },
  spec: {
    title: { en: "Air", sk: "Ovzdušie", cs: "Ovzduší", de: "Luft" },
    visibility: "project",
    pages: [
      { title: "Map", layout: "full-map", layers: ["stations"] },
      { title: "Second", layout: "full-map", layers: [] },
    ],
    ...over,
  },
});

const answering =
  (dashboards: unknown[], layers: unknown[] = [layer("stations")]) =>
  (path: string) => {
    if (path.endsWith("/dashboards")) return jsonResponse(list(dashboards));
    if (path.endsWith("/layers")) return jsonResponse(list(layers));
    if (path.endsWith("/endpoints")) return jsonResponse(list([ENDPOINT]));
    if (path.startsWith("/api/endpoint/")) {
      return jsonResponse({ type: "FeatureCollection", features: [] });
    }
    return undefined;
  };

describe("the dashboards page", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it("opens_under_one_h1_and_is_axe_clean", async () => {
    const { container } = await renderRoute({ path: PATH, answer: answering([dashboard()]) });
    await expectOneH1(en.dashboards.title);
    await screen.findByRole("tablist", { name: en.dashboards.pages });
    await expectAxeClean(container);
  });

  it("the_page_switcher_is_one_tab_stop_and_the_arrows_move_between_its_pages", async () => {
    await renderRoute({ path: PATH, answer: answering([dashboard()]) });
    const tablist = await screen.findByRole("tablist", { name: en.dashboards.pages });
    const [first, second] = within(tablist).getAllByRole("tab");

    // One tab stop for the whole list: the unselected tabs are reached with the arrows, not Tab.
    expect(first).toHaveAttribute("tabindex", "0");
    expect(second).toHaveAttribute("tabindex", "-1");

    first.focus();
    await userEvent.keyboard("{ArrowRight}");
    await waitFor(() => expect(second).toHaveAttribute("aria-selected", "true"));
    expect(second).toHaveFocus();
    await userEvent.keyboard("{Home}");
    await waitFor(() => expect(first).toHaveAttribute("aria-selected", "true"));
  });

  it("each_tab_names_the_panel_it_shows", async () => {
    await renderRoute({ path: PATH, answer: answering([dashboard()]) });
    const tablist = await screen.findByRole("tablist", { name: en.dashboards.pages });
    const selected = within(tablist).getAllByRole("tab")[0];
    const panel = screen.getByRole("tabpanel");
    // The pair a screen reader follows: the tab controls the panel, the panel is labelled by it.
    expect(selected).toHaveAttribute("aria-controls", panel.id);
    expect(panel).toHaveAttribute("aria-labelledby", selected.id);
  });

  it("a_single_page_dashboard_shows_no_switcher_and_no_panel_to_name", async () => {
    await renderRoute({
      path: PATH,
      answer: answering([dashboard({ pages: [{ title: "Map", layout: "full-map", layers: ["stations"] }] })]),
    });
    await expectOneH1(en.dashboards.title);
    expect(screen.queryByRole("tablist")).toBeNull();
    expect(screen.queryByRole("tabpanel")).toBeNull();
  });

  it("the_legend_ticks_a_layer_with_the_shared_checkbox_from_the_keyboard", async () => {
    await renderRoute({ path: PATH, answer: answering([dashboard()]) });
    const tick = await screen.findByRole("checkbox", { name: `${en.dashboards.show}: stations` });
    expect(tick).toHaveClass("size-[1rem]");
    expect(tick).toBeChecked();

    tick.focus();
    await userEvent.keyboard(" ");
    await waitFor(() => expect(tick).not.toBeChecked());
  });

  it("holds_its_layout_while_the_dashboards_are_on_their_way", async () => {
    await renderRoute({ path: PATH, pending: true });
    const waiting = await screen.findAllByRole("status");
    expect(waiting.length).toBeGreaterThan(0);
    expect(screen.queryByText(en.dashboards.empty)).toBeNull();
  });

  // UI-16, T-1503: the dashboard is read and its features are not: the map's frame holds its
  // place and says it is loading, while the page around it is already there.
  it("holds_the_maps_frame_while_its_features_are_on_their_way", async () => {
    const answer = answering([dashboard()]);
    await renderRoute({
      path: PATH,
      answer: (path) =>
        path.startsWith("/api/endpoint/") ? (new Promise<Response>(() => {}) as never) : answer(path),
    });
    // The dashboard itself has arrived: its pages are named.
    expect(await screen.findByRole("tab", { name: "Map" })).toBeInTheDocument();
    const panel = screen.getByRole("tabpanel");
    expect(within(panel).getByRole("status", { name: en.app.loading })).toBeInTheDocument();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("says_why_the_dashboards_could_not_be_read", async () => {
    await renderRoute({
      path: PATH,
      answer: (path) =>
        path.endsWith("/dashboards") ? problem(403, "You may not read this project's dashboards.") : undefined,
    });
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "You may not read this project's dashboards.",
    );
    expect(screen.queryByText(en.dashboards.empty)).toBeNull();
  });

  it("no_dashboard_yet_says_what_would_make_one", async () => {
    await renderRoute({ path: PATH, answer: answering([]) });
    expect(await screen.findByText(en.dashboards.empty)).toBeInTheDocument();
  });

  it("survives_a_dashboard_of_500_layers", async () => {
    const layers = Array.from({ length: 500 }, (_, index) => layer(`layer-${index}`));
    await renderRoute({
      path: PATH,
      answer: answering(
        [dashboard({ pages: [{ title: "Map", layout: "full-map", layers: layers.map((l) => l.metadata.name) }] })],
        layers,
      ),
    });
    const legend = await screen.findByRole("list", { name: en.dashboards.legend });
    await waitFor(() => expect(within(legend).getAllByRole("checkbox").length).toBe(500));
  });

  it("says_everything_it_says_in_all_four_languages", async () => {
    for (const locale of LOCALES) {
      const { unmount } = await renderRoute({ path: PATH, locale, answer: answering([]) });
      expect(await screen.findByText(i18n.t("dashboards.empty"))).toBeInTheDocument();
      if (locale !== "en") {
        expect(screen.queryByText(en.dashboards.empty)).toBeNull();
      }
      unmount();
      vi.restoreAllMocks();
    }
  });

  it("the_colour_ramp_is_the_layers_own_scale_and_nothing_else_carries_a_literal", async () => {
    const { container } = await renderRoute({
      path: PATH,
      brand: OTHER_BRAND,
      answer: answering([dashboard()]),
    });
    await screen.findByRole("tablist", { name: en.dashboards.pages });
    const literal = [...container.querySelectorAll<HTMLElement>("[style]")].filter((element) =>
      /#[0-9a-f]{3,8}|\brgba?\(/i.test(element.getAttribute("style") ?? ""),
    );
    // Only the swatches of a layer's own colour scale, and each is decorative.
    for (const element of literal) {
      expect(element.getAttribute("aria-hidden"), element.outerHTML.slice(0, 120)).toBe("true");
    }
  });

  it("a_dashboard_title_out_of_a_manifest_is_rendered_as_text", async () => {
    const { container } = await renderRoute({
      path: PATH,
      answer: answering([dashboard({ title: { en: "<img src=x onerror=alert(1)>" } })]),
    });
    expect(await screen.findByText("<img src=x onerror=alert(1)>")).toBeInTheDocument();
    expect(container.querySelector("img")).toBeNull();
  });
});
