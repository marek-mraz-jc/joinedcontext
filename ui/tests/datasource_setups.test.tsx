import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  email: "jana.kovacova@banskabystrica.sk",
  roles: ["domain-editor"],
};

function blueprint(
  name: string,
  title: string,
  category: string | null,
  riskClass: string,
): Record<string, unknown> {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Blueprint",
    metadata: {
      name,
      namespace: "org",
      title: { en: title, sk: `${title} (sk)` },
      description: { en: `What ${name} does`, sk: "Popis" },
    },
    spec: {
      version: "1.2.0",
      category,
      riskClass,
      allowedRoles: ["domain-editor"],
      parameterSchema: {
        type: "object",
        required: ["webhookUrl"],
        properties: { webhookUrl: { type: "string", title: "Notification target URL" } },
      },
    },
  };
}

/** What the server returns: it has already dropped everything this caller may not run. */
const GALLERY = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items: [
    blueprint("gbfs-stations", "City-bike stations", "ingestion", "yellow"),
    blueprint("mqtt-feed", "Subscribe to an MQTT feed", "ingestion", "red"),
    blueprint("app-from-prompt", "An application from a prompt", "application", "yellow"),
  ],
};

function renderGallery(gallery: unknown = GALLERY, sources: unknown[] = []) {
  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    const request = input as Request;
    const path = new URL(request.url).pathname;
    const json = (body: unknown, status = 200) =>
      Promise.resolve(
        new Response(JSON.stringify(body), {
          status,
          headers: { "Content-Type": "application/json" },
        }),
      );

    if (path.endsWith("/auth/me")) {
      return json(IDENTITY);
    }
    if (path.endsWith("/blueprints")) {
      return json(gallery);
    }
    if (path.endsWith("/datasources")) {
      return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: sources });
    }
    return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
  });
  vi.stubGlobal("fetch", fetchMock);

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

describe("ready-made set-ups on Data sources (T-6096)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/projects/banskabystrica/datasources");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("shows a card per ingestion blueprint with its review lane, and none of another category", async () => {
    renderGallery();

    const section = await screen.findByRole("region", { name: en.datasources.setups.title });
    const card = (await within(section).findByText("City-bike stations")).closest("li") as HTMLElement;
    expect(within(card).getByText(en.flows.risk.yellow)).toBeInTheDocument();
    expect(within(card).getByText("What gbfs-stations does")).toBeInTheDocument();
    expect(within(card).getByText("Version 1.2.0")).toBeInTheDocument();
    expect(within(section).getByText("Subscribe to an MQTT feed")).toBeInTheDocument();
    // An application set-up starts from Applications → New app, not here.
    expect(screen.queryByText("An application from a prompt")).not.toBeInTheDocument();
  });

  it("asks the organization-level route, not a project collection", async () => {
    const fetchMock = renderGallery();
    await screen.findByText("City-bike stations");

    const urls = fetchMock.mock.calls.map((call) => new URL((call[0] as Request).url).pathname);
    expect(urls).toContain("/api/v1/blueprints");
    expect(urls.some((url) => url.includes("/blueprints") && url.includes("/projects/"))).toBe(false);
  });

  it("opens the wizard for the card that was pressed, and goes back to Data sources", async () => {
    const user = userEvent.setup();
    renderGallery();

    const card = (await screen.findByText("City-bike stations")).closest("li") as HTMLElement;
    await user.click(within(card).getByRole("button", { name: en.flows.run }));

    expect(screen.getByRole("heading", { level: 1, name: "Set up: City-bike stations" })).toBeInTheDocument();
    // The form is generated from the blueprint's own parameter schema (CC-24, CC-31).
    expect(screen.getByLabelText(/Notification target URL/)).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: en.datasources.setups.back }));
    expect(await screen.findByRole("heading", { level: 1, name: en.datasources.title })).toBeInTheDocument();
    expect(screen.getByText("Subscribe to an MQTT feed")).toBeInTheDocument();
  });

  // UI judge (T-6096): below a tall empty list the set-ups fell under the fold.
  it("puts the set-ups before the list while the project has no source, after the table once it has one", async () => {
    const before = (a: Element, b: Element) => Boolean(a.compareDocumentPosition(b) & Node.DOCUMENT_POSITION_FOLLOWING);
    const { unmount } = { unmount: () => document.body.replaceChildren() };
    renderGallery();
    const empty = await screen.findByText(en.datasources.empty);
    expect(before(screen.getByRole("region", { name: en.datasources.setups.title }), empty)).toBe(true);
    unmount();

    renderGallery(GALLERY, [
      { apiVersion: "joinedcontext.com/v1alpha1", kind: "DataSource", metadata: { name: "bikes", namespace: "banskabystrica" }, spec: { type: "http", http: { url: "https://example.org/a.json" } } },
    ]);
    const row = await screen.findByText("bikes");
    expect(before(row, await screen.findByRole("region", { name: en.datasources.setups.title }))).toBe(true);
  });

  it("draws no set-up section when the person may run none", async () => {
    renderGallery({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
    expect(await screen.findByRole("heading", { level: 1, name: en.datasources.title })).toBeInTheDocument();
    await waitFor(() => {
      expect(screen.queryByRole("region", { name: en.datasources.setups.title })).not.toBeInTheDocument();
    });
  });

  it("shows the blueprint titles in the active locale", async () => {
    await i18n.changeLanguage("sk");
    renderGallery();
    expect(await screen.findByText("City-bike stations (sk)")).toBeInTheDocument();
  });
});

describe("the Flows page is gone (T-6096)", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("sends the old /flows address to the project's home", async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/projects/banskabystrica/flows");
    renderGallery();

    await waitFor(() => {
      expect(window.location.pathname).toBe("/projects/banskabystrica/home");
    });
  });

  it("has no Flows entry in the sidebar", async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/projects/banskabystrica/datasources");
    renderGallery();

    const nav = await screen.findByRole("navigation", { name: en.nav.main });
    expect(within(nav).queryByRole("link", { name: "Flows" })).not.toBeInTheDocument();
    expect(within(nav).getByRole("link", { name: en.nav.datasources })).toBeInTheDocument();
  });
});
