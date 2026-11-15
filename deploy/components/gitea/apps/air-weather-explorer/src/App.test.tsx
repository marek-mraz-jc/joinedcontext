/**
 * T-3374/T-3375, SDK-39, SDK-40, AP-140: the App in the SDK's shell, its language switch, and a
 * station opened in the SDK's entity panel from its button or from the map. A public App: the
 * panel reads the station and links to it in the Portal; it offers no Edit.
 */
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { JcProvider } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";
import App, { chooseLanguage } from "./App";
import { AIR_STATIONS, NOW, WEATHER_STATIONS } from "./fixtures/stations";
import { server } from "./testing/server";
import { Map as FakeMap, Popup } from "./testing/maplibre";

// The App's server, in memory (T-3348).
vi.mock("./server", async (original) => {
  const { server } = await import("./testing/server");
  return { ...(await original<typeof import("./server")>()), airApi: () => server };
});
vi.mock("maplibre-gl", () => import("./testing/maplibre"));
vi.mock("echarts/core", () => ({ use: vi.fn(), init: vi.fn(() => ({ setOption: vi.fn(), resize: vi.fn(), dispose: vi.fn(), on: vi.fn() })) }));

const READ = {
  permissions: [
    { resource: { type: "AirQualityObserved" }, actions: ["queryEntity", "retrieveEntity", "retrieveTemporal"], attributes: "*" as const },
    { resource: { type: "WeatherObserved" }, actions: ["queryEntity", "retrieveEntity", "retrieveTemporal"], attributes: "*" as const },
  ],
  prohibitions: [],
};
const PORTAL = "https://portal.dev.joinedcontext.com/projects/helsinki";

function show(language = "en") {
  const client = stubClient(
    { entities: [...AIR_STATIONS, ...WEATHER_STATIONS], access: READ },
    { appName: "air-weather-explorer", language, portal: PORTAL, space: "helsinki" },
  );
  render(
    <JcProvider client={client}>
      <App />
    </JcProvider>,
  );
  return client;
}

beforeEach(() => {
  server.reset();
  vi.useFakeTimers({ toFake: ["Date"] });
  vi.setSystemTime(new Date(NOW));
  window.history.replaceState(null, "", "/");
});
afterEach(() => {
  vi.useRealTimers();
  window.history.replaceState(null, "", "/");
  FakeMap.built.length = 0;
  Popup.opened.length = 0;
});

describe("the App in the SDK's shell", () => {
  it("has the shell's title", () => {
    show();
    expect(screen.getByRole("heading", { level: 1, name: "Air quality and weather" })).toBeInTheDocument();
  });

  // Every language's own list is used once: the role sweeps leave the shell header alone,
  // because a change of language reloads the page (T-4058).
  it.each([
    ["en", "Language", "fi"],
    ["fi", "Kieli", "sv"],
    ["sv", "Språk", "en"],
  ])("in %s, the language switch %s reloads the page in %s", (current, label, next) => {
    show(current);
    const language = screen.getByRole("combobox", { name: label });
    expect(language).toHaveValue(current);
    const reload = vi.fn();
    const original = window.location;
    Object.defineProperty(window, "location", { configurable: true, value: { ...original, search: "", hash: "", pathname: "/", reload } });
    try {
      fireEvent.change(language, { target: { value: next } });
      expect(reload).toHaveBeenCalledTimes(1);
    } finally {
      Object.defineProperty(window, "location", { configurable: true, value: original });
    }
  });

  it("writes the chosen language into the address and reloads only for a new one it speaks", () => {
    window.history.replaceState(null, "", "/?station=kallio#/compare");
    const reload = vi.fn();
    chooseLanguage("en", "en", reload);
    chooseLanguage("de", "en", reload);
    expect(reload).not.toHaveBeenCalled();
    chooseLanguage("fi", "en", reload);
    expect(reload).toHaveBeenCalledTimes(1);
    expect(window.location.search).toBe("?station=kallio&lang=fi");
    expect(window.location.hash).toBe("#/compare");
  });

  it("opens the chosen stations in the panel, which reads them and links to the Portal, with no Edit", async () => {
    show();
    fireEvent.click(await screen.findByRole("button", { name: "Details of Kallio 2" }));
    const panel = await screen.findByRole("dialog", { name: "Kallio 2" });
    const link = await within(panel).findByRole("link", { name: "Open in the Portal" });
    expect(link).toHaveAttribute("href", `${PORTAL}/explore?space=helsinki&entityId=${encodeURIComponent(AIR_STATIONS[0].id)}`);
    // Following it leaves the App; jsdom does not navigate, so the click is only seen to reach it.
    const followed = vi.fn((event: Event) => event.preventDefault());
    link.addEventListener("click", followed);
    fireEvent.click(link);
    expect(followed).toHaveBeenCalledTimes(1);
    expect(within(panel).queryByRole("button", { name: "Edit" })).toBeNull();
    fireEvent.click(within(panel).getByRole("button", { name: "Close" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());

    fireEvent.click(await screen.findByRole("button", { name: "Details of Helsinki Kaisaniemi" }));
    expect(await screen.findByRole("dialog", { name: "Helsinki Kaisaniemi" })).toBeInTheDocument();
  });

  it("picks a station on the map for the comparison and opens it, its popup in text only", async () => {
    show();
    const map = await waitFor(() => {
      const [built] = FakeMap.built;
      expect(built?.sources["app-points"]).toBeDefined();
      return built!;
    });
    await waitFor(() => expect((map.sources["app-points"].data as { features: unknown[] }).features).toHaveLength(6));
    expect(map.fitted).toHaveLength(1);

    act(() => map.fire("click", { features: [{ properties: { id: "weather:road-1002" } }] }, "app-points"));
    expect(await screen.findByRole("dialog", { name: "kt51_Hki_Lapinlahti" })).toBeInTheDocument();
    expect(screen.getByRole("combobox", { name: "Weather station" })).toHaveDisplayValue("kt51_Hki_Lapinlahti");
    expect(new URLSearchParams(window.location.search).get("weather")).toBe("road-1002");
    const [popup] = Popup.opened;
    expect(popup.content?.querySelector("strong")?.textContent).toBe("kt51_Hki_Lapinlahti");
    expect(popup.content?.innerHTML).not.toContain("<script");

    act(() => map.fire("click", { features: [{ properties: { id: "air:makelankatu" } }] }, "app-points"));
    expect(await screen.findByRole("dialog", { name: "Mäkelänkatu" })).toBeInTheDocument();
    expect(screen.getByRole("combobox", { name: "Air quality station" })).toHaveDisplayValue("Mäkelänkatu");

    // A click beside every point, or on a point the page no longer lists, picks nothing.
    act(() => map.fire("click", { features: [] }, "app-points"));
    act(() => map.fire("click", { features: [{ properties: { id: "air:gone" } }] }, "app-points"));
    expect(Popup.opened).toHaveLength(2);
    expect(screen.getByRole("dialog", { name: "Mäkelänkatu" })).toBeInTheDocument();
  });
});
