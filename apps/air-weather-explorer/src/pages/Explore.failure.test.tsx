/**
 * The analysis refused: the page says why in its own words instead of computing forever, whether
 * the module threw an error or something that is not one.
 */
import { render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { JcProvider } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";
import { AIR_STATIONS, NOW, WEATHER_STATIONS } from "../fixtures/stations";
import { Explore } from "./Explore";

// The App's server, in memory (T-3348).
vi.mock("../server", async (original) => {
  const { server } = await import("../testing/server");
  return { ...(await original<typeof import("../server")>()), airApi: () => server };
});
vi.mock("maplibre-gl", () => import("../testing/maplibre"));
vi.mock("echarts/core", () => ({ use: () => undefined, init: () => ({ setOption: () => undefined, resize: () => undefined, dispose: () => undefined, on: () => undefined }) }));
const refusal = vi.hoisted(() => ({ value: null as unknown }));
vi.mock("../analysis", async (original) => ({
  ...(await original<typeof import("../analysis")>()),
  computeAnalysis: () => Promise.reject(refusal.value),
}));

function show() {
  const client = stubClient({ entities: [...AIR_STATIONS, ...WEATHER_STATIONS] }, { appName: "air-weather-explorer", language: "en" });
  render(
    <JcProvider client={client}>
      <Explore />
    </JcProvider>,
  );
}

beforeEach(() => {
  vi.useFakeTimers({ toFake: ["Date"] });
  vi.setSystemTime(new Date(NOW));
});
afterEach(() => {
  vi.useRealTimers();
});

describe("a refused analysis", () => {
  it("says the module's own words", async () => {
    refusal.value = new Error("the readings could not be read: not a number");
    show();
    expect(await screen.findByRole("alert")).toHaveTextContent("The comparison could not be computed: the readings could not be read: not a number");
  });

  it("says what came back when it is not an error", async () => {
    refusal.value = "out of memory";
    show();
    expect(await screen.findByRole("alert")).toHaveTextContent("The comparison could not be computed: out of memory");
  });
});
