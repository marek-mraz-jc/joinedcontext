/**
 * The indicator card (T-0583, UI-17, PF-55): the value large with its unit, the formula and the
 * count, one button that writes the entity through the indicator space's endpoint with the
 * person's session, the refusal shown in place, and the link into the space once written.
 */
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { RouterProvider, createRootRoute, createRoute, createRouter } from "@tanstack/react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { KpiCard, formatValue, keepMessage, kpiOf } from "../src/pages/apps/KpiCard";
import type { Kpi } from "../src/pages/apps/KpiCard";
import { expectNoAxeViolations } from "./page_contract";

const SLUG = "kpislug0000000000000000000";
const ENTITY = {
  id: "urn:ngsi-ld:KeyPerformanceIndicator:hel.fi:helsinki-kpi:average-pm10",
  type: "KeyPerformanceIndicator",
  currentValue: { type: "Property", value: 18.4, unitCode: "GQ" },
  "@context": ["https://uri.etsi.org/ngsi-ld/v1/ngsi-ld-core-context-v1.8.jsonld"],
};
const KPI: Kpi = {
  name: "average-pm10",
  title: "Average PM10 <b>now</b>",
  value: 18.4,
  unit: "GQ",
  formula: "avg(pm10) over AirQualityObserved",
  count: 12,
  space: "helsinki-kpi",
  endpointSlug: SLUG,
  endpointName: "helsinki-kpi-all",
  entity: ENTITY,
};

function renderCard(kpi: Kpi = KPI, onSend?: (text: string) => void) {
  const rootRoute = createRootRoute();
  const home = createRoute({ getParentRoute: () => rootRoute, path: "/", component: () => <KpiCard project="helsinki" kpi={kpi} onSend={onSend} /> });
  const explore = createRoute({ getParentRoute: () => rootRoute, path: "/projects/$project/explore", component: () => <p>explore</p> });
  const plural = createRoute({ getParentRoute: () => rootRoute, path: "/projects/$project/$plural", component: () => <p>list</p> });
  const router = createRouter({ routeTree: rootRoute.addChildren([home, explore, plural]) });
  render(
    <I18nextProvider i18n={i18n}>
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <RouterProvider router={router} />
      </QueryClientProvider>
    </I18nextProvider>,
  );
  return router;
}

function stubFetch(status: number, body: unknown = null) {
  const fetchMock = vi.fn(async () => new Response(body === null ? null : JSON.stringify(body), { status, headers: { "content-type": "application/json" } }));
  vi.stubGlobal("fetch", fetchMock);
  return fetchMock;
}

describe("the indicator card", () => {
  beforeEach(async () => {
    window.history.pushState({}, "", "/");
    document.cookie = "__Host-jc_csrf=csrf-xyz; Secure; path=/";
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("shows the value with its unit, the formula, the count and the id, as text", async () => {
    renderCard();
    const card = await screen.findByRole("region", { name: en.agentRun.kpi.title });
    expect(within(card).getByTestId("kpi-value")).toHaveTextContent("18.4");
    expect(within(card).getByText("µg/m³").closest("[title]")).toHaveAttribute("title", "microgram per cubic metre (GQ)");
    expect(within(card).getByText("avg(pm10) over AirQualityObserved")).toBeInTheDocument();
    expect(within(card).getByText("12 entities")).toBeInTheDocument();
    expect(within(card).getByText(ENTITY.id)).toBeInTheDocument();
    expect(within(card).getByText("Average PM10 <b>now</b>")).toBeInTheDocument();
    expect(document.querySelector("b")).toBeNull();
  });

  it("writes the entity through the indicator space's endpoint with the session, then links into the space", async () => {
    const fetchMock = stubFetch(201);
    const router = renderCard();
    await userEvent.click(await screen.findByRole("button", { name: en.agentRun.kpi.write }));
    expect(fetchMock).toHaveBeenCalledTimes(1);
    const [url, init] = fetchMock.mock.calls[0] as unknown as [string, RequestInit];
    expect(url).toBe(`/api/endpoint/${SLUG}/ngsi-ld/v1/entities`);
    expect(init.method).toBe("POST");
    expect(init.credentials).toBe("same-origin");
    expect((init.headers as Record<string, string>)["x-csrf-token"]).toBe("csrf-xyz");
    // DM-113: the indicator's write names why it was written.
    expect((init.headers as Record<string, string>)["JC-Reason"]).toBe(encodeURIComponent(en.agentRun.kpi.reason));
    expect((init.headers as Record<string, string>)["content-type"]).toBe("application/ld+json");
    expect(JSON.parse(init.body as string)).toEqual(ENTITY);
    expect(await screen.findByText(en.agentRun.kpi.written)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("link", { name: en.agentRun.kpi.view }));
    await vi.waitFor(() => {
      expect(router.state.location.pathname).toBe("/projects/helsinki/explore");
      expect(router.state.location.search).toEqual({ space: "helsinki-kpi", endpoint: "helsinki-kpi-all" });
    });
  });

  it("keeps a refusal on the card with the endpoint's reason", async () => {
    stubFetch(403, { type: "about:blank", title: "Forbidden", status: 403, detail: "no policy grants createEntity in helsinki-kpi" });
    renderCard();
    await userEvent.click(await screen.findByRole("button", { name: en.agentRun.kpi.write }));
    expect(await screen.findByRole("alert")).toHaveTextContent("no policy grants createEntity in helsinki-kpi");
    expect(screen.getByRole("button", { name: en.agentRun.kpi.write })).toBeInTheDocument();
    expect(screen.queryByText(en.agentRun.kpi.written)).toBeNull();
  });

  it("says when the indicator space has no endpoint and offers the endpoints page instead", async () => {
    const fetchMock = stubFetch(201);
    renderCard({ ...KPI, endpointSlug: undefined, endpointName: undefined });
    expect(await screen.findByText(/has no endpoint yet/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: en.agentRun.kpi.write })).toBeNull();
    expect(screen.getByRole("link", { name: en.agentRun.kpi.openEndpoints })).toBeInTheDocument();
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("asks the assistant to keep the indicator updated on a period or on every change, into the space the person names", async () => {
    const onSend = vi.fn();
    const kpi = kpiOf(
      { ...KPI },
      { name: "average-pm10", type: "AirQualityObserved", attribute: "pm10", agg: "avg", q: "pm10>0" },
    );
    expect(kpi?.query).toEqual({ type: "AirQualityObserved", attribute: "pm10", agg: "avg", q: "pm10>0" });
    renderCard(kpi ?? KPI, onSend);
    await userEvent.click(await screen.findByRole("button", { name: en.agentRun.kpi.keep }));
    const form = screen.getByRole("form", { name: en.agentRun.kpi.keep });
    const minutes = within(form).getByRole("spinbutton", { name: en.agentRun.kpi.keepMinutes });
    await userEvent.clear(minutes);
    await userEvent.type(minutes, "30");
    const space = within(form).getByRole("combobox", { name: /Into the space/ });
    expect(space).toHaveValue("helsinki-kpi");
    await userEvent.clear(space);
    await userEvent.type(space, "air-kpi");
    await userEvent.click(within(form).getByRole("button", { name: en.agentRun.kpi.keepSend }));
    expect(onSend).toHaveBeenCalledWith(
      "Keep the indicator average-pm10 updated: avg of pm10 over AirQualityObserved where pm10>0, every 30m, into the space air-kpi.",
    );

    // Reopened, the form keeps the space the person named.
    await userEvent.click(screen.getByRole("button", { name: en.agentRun.kpi.keep }));
    await userEvent.click(screen.getByRole("radio", { name: en.agentRun.kpi.keepOnChange }));
    await userEvent.click(screen.getByRole("button", { name: en.agentRun.kpi.keepSend }));
    expect(onSend).toHaveBeenLastCalledWith(
      "Keep the indicator average-pm10 updated: avg of pm10 over AirQualityObserved where pm10>0, on every change, into the space air-kpi.",
    );
  });

  it("offers no pipeline when the conversation takes no message, and a count names no attribute", () => {
    renderCard();
    expect(screen.queryByRole("button", { name: en.agentRun.kpi.keep })).toBeNull();
    const t = i18n.getFixedT("en");
    const count = { ...KPI, query: { type: "BikeHireDockingStation", attribute: "", agg: "count" } };
    expect(keepMessage(count, { onChange: false, minutes: 15, space: "transportation-kpi" }, t)).toBe(
      "Keep the indicator average-pm10 updated: count of BikeHireDockingStation, every 15m, into the space transportation-kpi.",
    );
  });

  it("takes only what the Portal wrote, and formats a value to read", () => {
    expect(kpiOf(KPI)?.name).toBe("average-pm10");
    expect(kpiOf({ name: "x", value: "18" })).toBeNull();
    expect(kpiOf(null)).toBeNull();
    expect(formatValue(1200, "en")).toBe("1,200");
    expect(formatValue(18.4444, "en")).toBe("18.44");
    expect(formatValue(123.456, "en")).toBe("123.5");
    expect(formatValue(0.00314, "en")).toBe("0.0031");
  });

  // T-3986: the app builder draws this card only in a model run, which the live walk cannot start (spend guard).
  it("has_no_serious_axe_violation_as_a_model_run_draws_it", async () => {
    renderCard();
    await screen.findByRole("region", { name: en.agentRun.kpi.title });
    await expectNoAxeViolations(document.body);
  });
});
