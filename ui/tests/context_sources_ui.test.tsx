/**
 * T-4819: a registration has a page of its own that says what its broker forwards, and a saved
 * data source is tested from its form, with the pipelines that read it beside the test.
 */
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import en from "../src/locales/en.json";
import i18n from "../src/i18n";
import { App } from "../src/App";
import { ConnectionTest } from "../src/pages/datasources/ConnectionTest";
import type { Manifest } from "../src/api/manifest";
import { lastError } from "../src/routes/RegistrationPage";

const IDENTITY = { subject: "s1", username: "steward", email: "demo.steward@hel.fi", roles: ["portal-editor"] };
const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const SOURCE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "DataSource",
  metadata: { name: "fmi-air-quality", namespace: "helsinki" },
  spec: { type: "http", http: { url: "https://opendata.fmi.fi/aq.json" } },
  status: { phase: "Live" },
};
const READER = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Pipeline",
  metadata: { name: "air-quality-ingest", namespace: "helsinki", title: { en: "Air quality ingest" } },
  spec: { source: { dataSourceRef: { kind: "DataSource", name: "fmi-air-quality" } } },
};
const REGISTRATION: Manifest = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ContextSourceRegistration",
  metadata: { name: "helsinki-indicators", namespace: "helsinki", title: { en: "Helsinki indicators" } },
  spec: {
    contextSpaceRef: "hub",
    endpointRef: "indicators",
    information: [{ entities: [{ type: "KeyPerformanceIndicator" }], propertyNames: ["kpiValue"] }],
    operations: ["retrieveEntity", "queryEntity"],
  },
  status: {
    phase: "Error",
    conditions: [{ type: "RegistrationWritten", status: "False", message: "the broker answered 409 Conflict" }],
  },
};

function renderAt(path: string, test: unknown) {
  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    const request = input as Request;
    const url = new URL(request.url);
    const json = (body: unknown, status = 200) =>
      Promise.resolve(new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } }));
    if (url.pathname.endsWith("/auth/me")) return json(IDENTITY);
    if (url.pathname.endsWith("/datasources/fmi-air-quality/test")) return json(test);
    if (url.pathname.endsWith("/csrs/helsinki-indicators")) return json(REGISTRATION);
    if (url.pathname.endsWith("/datasources")) return json(list([SOURCE]));
    if (url.pathname.endsWith("/pipelines")) return json(list([READER]));
    return json(list([]));
  });
  vi.stubGlobal("fetch", fetchMock);
  window.history.pushState({}, "", path);
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

describe("context sources and the connection test (T-4819)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it("a registration's page says what it forwards and why the broker refused it", async () => {
    renderAt("/projects/helsinki/csrs/helsinki-indicators", {});
    expect(await screen.findByRole("heading", { name: "Helsinki indicators" })).toBeInTheDocument();
    expect(screen.getByText("KeyPerformanceIndicator")).toBeInTheDocument();
    expect(screen.getByText("kpiValue")).toBeInTheDocument();
    expect(screen.getByText("retrieveEntity, queryEntity")).toBeInTheDocument();
    expect(screen.getByRole("alert")).toHaveTextContent("the broker answered 409 Conflict");
    expect(screen.getByRole("link", { name: "indicators" })).toHaveAttribute("href", "/projects/helsinki/endpoints/indicators");
    // Edit is a button on the header's row, not a link at the far edge of the page.
    expect(screen.getByRole("button", { name: en.registrations.page.edit })).toBeInTheDocument();
  });

  it("tests a saved source and shows its first record and the pipelines that read it", async () => {
    const fetchMock = renderAt("/projects/helsinki/datasources/fmi-air-quality/edit", {
      records: 3,
      bytes: 120,
      sample: { station: "Kallio", pm10: 12 },
    });
    const section = await screen.findByTestId("datasource-connection", undefined, { timeout: 4000 });
    expect(await within(section).findByRole("link", { name: "Air quality ingest" })).toBeInTheDocument();
    await userEvent.click(within(section).getByRole("button", { name: "Test connection" }));
    expect(await within(section).findByText(/Connected: 3 records read/)).toBeInTheDocument();
    // T-5516: the first record reads as rows, never as JSON text.
    expect(within(section).getByText("Kallio")).toBeInTheDocument();
    const posted = fetchMock.mock.calls.map((call) => call[0] as Request).filter((r) => r.url.endsWith("/test"));
    expect(posted.map((r) => r.method)).toEqual(["POST"]);
  });

  it("says in plain words why nothing was read", async () => {
    renderAt("/projects/helsinki/datasources/fmi-air-quality/edit", {
      skipped: "the feed did not answer within the test's three seconds",
    });
    const section = await screen.findByTestId("datasource-connection", undefined, { timeout: 4000 });
    await userEvent.click(within(section).getByRole("button", { name: "Test connection" }));
    expect(
      await within(section).findByText("Nothing was read: the feed did not answer within the test's three seconds. Check the address and the credentials, then test again."),
    ).toBeInTheDocument();
  });

  it("names the condition that refused the registration, and nothing when every condition holds", () => {
    expect(lastError(REGISTRATION)?.message).toBe("the broker answered 409 Conflict");
    expect(lastError({ ...REGISTRATION, status: { phase: "Live", conditions: [{ type: "RegistrationWritten", status: "True" }] } })).toBeUndefined();
  });

  it("posts the test for the source it is given and shows what came back", async () => {
    const fetchMock = vi.fn((input: RequestInfo | URL) => {
      const url = new URL((input as Request).url);
      const body = url.pathname.endsWith("/test") ? { records: 1, bytes: 9, sample: { id: 1 } } : list([]);
      return Promise.resolve(new Response(JSON.stringify(body), { status: 200, headers: { "Content-Type": "application/json" } }));
    });
    vi.stubGlobal("fetch", fetchMock);
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <ConnectionTest project="helsinki" name="demo-counters" />
        </I18nextProvider>
      </QueryClientProvider>,
    );
    await userEvent.click(screen.getByRole("button", { name: "Test connection" }));
    expect(await screen.findByText(/Connected: 1 record/)).toBeInTheDocument();
    const posted = fetchMock.mock.calls.map((call) => call[0] as Request).filter((r) => r.url.endsWith("/test"));
    expect(posted.map((r) => new URL(r.url).pathname)).toEqual(["/api/v1/projects/helsinki/datasources/demo-counters/test"]);
  });
});
