/**
 * T-1809, T-1814: a part that could not ask must not look like one still asking (UI-01, UI-44).
 *
 * The same defect twice, in two widgets nobody had written a test for. The temporal chart drew
 * "No history to draw." whether the endpoint had none or had refused the question, so a reader
 * concluded a sensor was silent when their own permissions were the answer. The access panel
 * left its decision chips on "checking…" for good when the gateway refused, which reads as "any
 * moment now" and is the opposite of what happened.
 */
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { AccessPanel, useAccess } from "../src/components/entities/AccessPanel";
import { TemporalChart } from "../src/components/dashboards/TemporalChart";

const HISTORY = {
  id: "urn:ngsi-ld:AirQualityObserved:hel.fi:air:kamppi",
  type: "AirQualityObserved",
  pm10: { type: "Property", values: [[12, "2026-09-15T08:00:00Z"], [18, "2026-09-15T09:00:00Z"]] },
};

function wrap(node: React.ReactNode) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>{node}</I18nextProvider>
    </QueryClientProvider>,
  );
}

const chart = () => (
  <TemporalChart
    slug="k7m2qz4tv6xh3n5jb2ryd3wcfa"
    entityId="urn:ngsi-ld:AirQualityObserved:hel.fi:air:kamppi"
    property="pm10"
    title="pm10 over time"
  />
);

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("the temporal chart", () => {
  it("draws_the_readings_it_was_given", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() =>
        Promise.resolve(
          new Response(JSON.stringify(HISTORY), {
            status: 200,
            headers: { "Content-Type": "application/ld+json" },
          }),
        ),
      ),
    );
    const { container } = wrap(chart());
    await waitFor(() => expect(container.querySelector("polyline")).not.toBeNull());
    expect(screen.queryByText(en.dashboards.widget.noHistory)).toBeNull();
  });

  it("keeps_the_shape_of_the_chart_while_the_readings_are_on_their_way", () => {
    vi.stubGlobal("fetch", vi.fn(() => new Promise<Response>(() => {})));
    wrap(chart());
    const waiting = screen.getByRole("status", { name: en.app.loading });
    expect(waiting).toHaveAttribute("aria-busy", "true");
    expect(waiting.querySelector("span[aria-hidden='true']")).not.toBeNull();
  });

  it("says_the_history_could_not_be_read_rather_than_that_there_is_none", async () => {
    const fetchMock = vi.fn(() =>
      Promise.resolve(new Response("nope", { status: 403, statusText: "Forbidden" })),
    );
    vi.stubGlobal("fetch", fetchMock);
    wrap(chart());

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("HTTP 403");
    // The sentence a reader would otherwise have taken for "the sensor is silent".
    expect(screen.queryByText(en.dashboards.widget.noHistory)).toBeNull();

    const before = fetchMock.mock.calls.length;
    await userEvent.click(screen.getByRole("button", { name: en.app.error.retry }));
    await waitFor(() => expect(fetchMock.mock.calls.length).toBeGreaterThan(before));
  });

  it("an_endpoint_with_nothing_to_show_still_says_there_is_no_history", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() =>
        Promise.resolve(
          new Response(JSON.stringify({ id: "x", type: "AirQualityObserved" }), { status: 200 }),
        ),
      ),
    );
    wrap(chart());
    expect(await screen.findByText(en.dashboards.widget.noHistory)).toBeInTheDocument();
    expect(screen.queryByRole("alert")).toBeNull();
  });
});

const GRANTS = {
  subject: { type: "user", id: "jana" },
  permissions: [
    { resource: { type: "AirQualityObserved" }, actions: ["queryEntity"], attributes: ["pm10"] },
  ],
};

/** The panel as a page mounts it: its own `useAccess`, so the wiring under test is the real one. */
function Panel(): React.JSX.Element {
  const slug = "k7m2qz4tv6xh3n5jb2ryd3wcfa";
  return <AccessPanel slug={slug} type="AirQualityObserved" access={useAccess(slug)} />;
}

describe("the access panel", () => {
  it("says_the_permissions_could_not_be_checked_rather_than_checking_for_good", async () => {
    // `/access` answers, `/access/v1/evaluations` does not: the grants are on screen and the decisions
    // are what could not be made, which is the case the chips used to swallow.
    vi.stubGlobal(
      "fetch",
      vi.fn((input: RequestInfo | URL) => {
        const url = input instanceof Request ? input.url : String(input);
        return url.includes("/access/v1/evaluations")
          ? Promise.reject(new Error("The gateway is not answering."))
          : Promise.resolve(
              new Response(JSON.stringify(GRANTS), {
                status: 200,
                headers: { "Content-Type": "application/json" },
              }),
            );
      }),
    );
    wrap(<Panel />);

    expect(await screen.findByRole("alert")).toHaveTextContent("The gateway is not answering.");
    // "checking…" on a chip that will never resolve reads as "any moment now".
    expect(
      screen.queryByText(en.access.panel.checking.replace("{action}", "queryEntity")),
    ).toBeNull();
  });

  it("shows_the_decision_for_each_action_when_the_gateway_answers", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn((input: RequestInfo | URL) => {
        const url = input instanceof Request ? input.url : String(input);
        const body = url.includes("/access/v1/evaluations") ? { evaluations: [{ decision: true }, { decision: true }] } : GRANTS;
        return Promise.resolve(
          new Response(JSON.stringify(body), {
            status: 200,
            headers: { "Content-Type": "application/json" },
          }),
        );
      }),
    );
    wrap(<Panel />);

    const checks = await screen.findByRole("list", { name: en.access.panel.checks });
    await waitFor(() =>
      expect(checks.textContent).toContain(
        en.access.panel.may.replace("{action}", en.access.panel.actions.queryEntity).replace("{type}", "AirQualityObserved"),
      ),
    );
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("says_the_permissions_could_not_be_checked_when_the_batch_answers_fewer_decisions", async () => {
    // A decision missing from the answer is not a "no": the panel says it could not check.
    vi.stubGlobal(
      "fetch",
      vi.fn((input: RequestInfo | URL) => {
        const url = input instanceof Request ? input.url : String(input);
        const body = url.includes("/access/v1/evaluations") ? { evaluations: [{ decision: true }] } : GRANTS;
        return Promise.resolve(
          new Response(JSON.stringify(body), { status: 200, headers: { "Content-Type": "application/json" } }),
        );
      }),
    );
    wrap(<Panel />);

    expect(await screen.findByRole("alert")).toHaveTextContent(en.app.error.generic);
    expect(screen.queryByRole("list", { name: en.access.panel.checks })).toBeNull();
  });
});
