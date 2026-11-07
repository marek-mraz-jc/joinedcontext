/**
 * T-5118: an Endpoint's Usage section and the project's Usage table. A reader sees calls per day
 * and surface, an empty window says what to do next, a refusal is said in words, and a month
 * downloads as the CSV the Portal wrote.
 */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ReactNode } from "react";
import i18n from "../src/i18n";
import { EndpointUsageSection, ProjectUsageSection } from "../src/components/endpoints/UsageSection";
import { thisMonth } from "../src/api/endpointUsage";
import { InRouter, expectAxeClean, jsonResponse, problem } from "./pageHarness";

const PROJECT = "mesto";

function days(totals: number[]) {
  return totals.map((total, i) => ({
    day: `2026-10-${String(i + 1).padStart(2, "0")}`,
    calls: total ? { "ngsi-ld": total - 1, download: 1 } : {},
    total,
  }));
}

function usage(totals: number[]) {
  const total = totals.reduce((a, b) => a + b, 0);
  return {
    name: "air",
    from: "2026-10-01",
    to: `2026-10-${String(totals.length).padStart(2, "0")}`,
    days: days(totals),
    totals: total ? { "ngsi-ld": total - totals.filter(Boolean).length, download: totals.filter(Boolean).length } : {},
    total,
  };
}

function answer(route: (url: URL) => Response) {
  const calls: URL[] = [];
  vi.stubGlobal("fetch", (input: RequestInfo | URL, init?: RequestInit) => {
    const request = input instanceof Request ? input : new Request(String(input), init);
    const url = new URL(request.url, "http://localhost");
    calls.push(url);
    return Promise.resolve(route(url));
  });
  return calls;
}

function mount(children: ReactNode) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <InRouter>{children}</InRouter>
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("EndpointUsageSection (T-5118)", () => {
  it("shows the window's calls, the totals per surface and a table of the busy days", async () => {
    const user = userEvent.setup();
    const calls = answer((url) => jsonResponse(usage(url.searchParams.get("days") === "7" ? [0, 0, 0, 0, 0, 2, 3] : [4, 0, 6])));
    const { container } = mount(<EndpointUsageSection project={PROJECT} name="air" />);
    expect(await screen.findByText(i18n.t("endpointUsage.total", { calls: "10", from: "2026-10-01", to: "2026-10-03" }))).toBeInTheDocument();
    expect(screen.getByRole("img", { name: i18n.t("endpointUsage.chart", { calls: "10", days: 30 }) })).toBeInTheDocument();
    expect(screen.getByText(i18n.t("endpointUsage.surface.ngsi-ld"))).toBeInTheDocument();
    expect(screen.getByText(i18n.t("endpointUsage.surface.download"))).toBeInTheDocument();
    expect(screen.queryByText(i18n.t("endpointUsage.surface.ogc"))).toBeNull();
    const table = screen.getByRole("table", { hidden: true });
    expect(within(table).getAllByRole("row", { hidden: true })).toHaveLength(3);
    expect(screen.getByText(i18n.t("endpointUsage.hint.window"))).toBeInTheDocument();
    expect(screen.getByText(i18n.t("endpointUsage.privacy"))).toBeInTheDocument();
    await expectAxeClean(container);

    await user.selectOptions(screen.getByLabelText(i18n.t("endpointUsage.window")), "7");
    expect(await screen.findByRole("img", { name: i18n.t("endpointUsage.chart", { calls: "5", days: 7 }) })).toBeInTheDocument();
    expect(calls.map((url) => `${url.pathname}?${url.searchParams.toString()}`)).toEqual([
      `/api/v1/projects/${PROJECT}/endpoints/air/usage?days=30`,
      `/api/v1/projects/${PROJECT}/endpoints/air/usage?days=7`,
    ]);
  });

  it("says what to do when nobody called, and a refusal in the server's words", async () => {
    answer(() => jsonResponse(usage([0, 0])));
    const first = mount(<EndpointUsageSection project={PROJECT} name="air" />);
    expect(await screen.findByText(i18n.t("endpointUsage.noCalls"))).toBeInTheDocument();
    expect(screen.queryByRole("img")).toBeNull();
    first.unmount();

    answer(() => problem(503, "the usage database did not answer; try again in a minute"));
    mount(<EndpointUsageSection project={PROJECT} name="air" />);
    expect(await screen.findByRole("alert")).toHaveTextContent("the usage database did not answer; try again in a minute");
  });
});

describe("ProjectUsageSection (T-5118)", () => {
  const PROJECT_USAGE = {
    from: "2026-09-23",
    to: "2026-10-22",
    endpoints: [
      { name: "air", slug: "air-slug", total: 12, totals: { "ngsi-ld": 12 }, daily: [5, 7] },
      { name: "quiet", slug: "quiet-slug", total: 0, totals: {}, daily: [0, 0] },
    ],
  };

  it("lists every readable Endpoint with its calls and downloads a month as CSV", async () => {
    const user = userEvent.setup();
    const saved: Blob[] = [];
    vi.spyOn(URL, "createObjectURL").mockImplementation((blob) => {
      saved.push(blob as Blob);
      return "blob:usage";
    });
    vi.spyOn(URL, "revokeObjectURL").mockImplementation(() => undefined);
    vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(() => undefined);
    const calls = answer((url) =>
      url.pathname.endsWith("/export")
        ? new Response("day,endpoint,surface,calls\n2026-10-01,air,ngsi-ld,5\n", { headers: { "Content-Type": "text/csv; charset=utf-8" } })
        : jsonResponse(PROJECT_USAGE),
    );
    const { container } = mount(<ProjectUsageSection project={PROJECT} />);
    const table = await screen.findByRole("table", { name: i18n.t("endpointUsage.title") });
    expect(within(table).getByRole("link", { name: "air" })).toHaveAttribute("href", `/projects/${PROJECT}/endpoints/air`);
    expect(within(table).getByText(i18n.t("endpointUsage.none"))).toBeInTheDocument();
    expect(screen.getByText(i18n.t("endpointUsage.hint.month"))).toBeInTheDocument();
    await expectAxeClean(container);

    const month = screen.getByLabelText(i18n.t("endpointUsage.month"));
    expect(month).toHaveValue(thisMonth());
    await user.clear(month);
    expect(screen.getByRole("button", { name: i18n.t("endpointUsage.export") })).toBeDisabled();
    await user.type(month, "2026-10");
    await user.click(screen.getByRole("button", { name: i18n.t("endpointUsage.export") }));
    await waitFor(() => expect(saved).toHaveLength(1));
    expect(await saved[0]?.text()).toBe("day,endpoint,surface,calls\n2026-10-01,air,ngsi-ld,5\n");
    expect(calls.some((url) => url.pathname.endsWith("/usage/endpoints/export") && url.searchParams.get("month") === "2026-10")).toBe(true);
  });

  it("tells a person who may read no Endpoint whom to ask", async () => {
    answer(() => jsonResponse({ from: "2026-09-23", to: "2026-10-22", endpoints: [] }));
    mount(<ProjectUsageSection project={PROJECT} />);
    expect(await screen.findByText(i18n.t("endpointUsage.noEndpoints"))).toBeInTheDocument();
  });
});
