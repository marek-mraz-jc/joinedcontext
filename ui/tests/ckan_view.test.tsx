// covers (T-2137, the module gate in gate_modules.test.ts): the cases in this file drive
// src/pages/ckan/CkanPage.tsx through the page they belong to; each was confirmed by
// making the module throw and watching this file go red.
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { byLabel } from "./formContract";
import { narrowsAndOrders } from "./grid_checks";
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
  roles: ["portal-approver"],
};

const SLUG = "zt4qm7ge2xdv6ksb3ncf5arw2y";

const STATUS = {
  instances: [
    {
      name: "open-data",
      url: "https://data.banskabystrica.sk",
      organizationDefault: "mesto-banska-bystrica",
      apiTokenRef: "ckan-open-data",
    },
  ],
  publications: [
    {
      endpoint: "ovzdusie-public",
      phase: "Live",
      instance: "open-data",
      instanceMissing: false,
      organization: "mesto-banska-bystrica",
      dataset: "kvalita-ovzdusia",
      datasetUrl: "https://data.banskabystrica.sk/dataset/kvalita-ovzdusia",
      resources: [
        {
          name: "NGSI-LD API",
          url: `https://bb.example.com/api/endpoint/${SLUG}/ngsi-ld/v1/`,
          format: "NGSI-LD",
        },
        {
          name: "CSV",
          url: `https://bb.example.com/api/endpoint/${SLUG}/file.csv`,
          format: "CSV",
        },
      ],
    },
    {
      endpoint: "hluk-public",
      instance: "gone",
      instanceMissing: true,
      dataset: "hluk",
      resources: [],
    },
  ],
};

const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-0000a1b2", namespace: "banskabystrica" },
  status: { lane: "yellow", phase: "PendingApproval", plan: { create: 1 } },
};

/** What the reconciler said about the last runs, newest first, as the activity feed lists it. */
const ACTIVITY = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ActivityList",
  items: [
    {
      time: "2026-10-06T10:00:00Z",
      project: "banskabystrica",
      kind: "catalogue.published",
      source: "ckan",
      severity: "error",
      summary: "Endpoint hluk-public was not published: no CkanInstance 'gone' in this project",
      details: { reason: "no CkanInstance 'gone' in this project", object: "endpoints/hluk-public" },
    },
    {
      time: "2026-10-06T09:00:00Z",
      project: "banskabystrica",
      kind: "catalogue.published",
      source: "ckan",
      severity: "info",
      summary: "Endpoint ovzdusie-public is dataset kvalita-ovzdusia.",
      details: { dataset: "kvalita-ovzdusia", object: "endpoints/ovzdusie-public" },
    },
    {
      time: "2026-10-05T09:00:00Z",
      project: "banskabystrica",
      kind: "catalogue.published",
      source: "ckan",
      severity: "error",
      summary: "Endpoint ovzdusie-public was not published: catalogue answered 500",
      details: { reason: "catalogue answered 500", object: "endpoints/ovzdusie-public" },
    },
  ],
};

function renderCkan(status: unknown = STATUS, permissions?: unknown) {
  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    const request = input as Request;
    const path = new URL(request.url).pathname;
    const json = (body: unknown, code = 200) =>
      Promise.resolve(
        new Response(JSON.stringify(body), {
          status: code,
          headers: {
            "Content-Type": code >= 400 ? "application/problem+json" : "application/json",
          },
        }),
      );

    if (path.endsWith("/auth/me")) {
      return json(IDENTITY);
    }
    if (path.endsWith("/branding")) {
      return json({ instanceName: "joinedcontext", languages: { default: "en", offered: ["en"] } });
    }
    if (path.endsWith("/ckan/status")) {
      return json(status);
    }
    if (path.endsWith("/activity") && new URL(request.url).searchParams.get("kind") === "catalogue.published") {
      return json(ACTIVITY);
    }
    if (permissions !== undefined && path.endsWith("/permissions/me")) {
      return json(permissions);
    }
    if (request.method !== "GET") {
      return json(CHANGE, 202);
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

describe("ckan publishing manager", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/projects/banskabystrica/ckan");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  // T-6206: the catalogues are on the shared grid: search, sort by address, filter by organization.
  it("searches, sorts and filters the catalogues", async () => {
    const instance = (name: string, url: string, organizationDefault: string) => ({ name, url, organizationDefault, apiTokenRef: `ckan-${name}` });
    renderCkan({
      ...STATUS,
      instances: [
        instance("open-data", "https://data.banskabystrica.sk", "mesto"),
        instance("archive", "https://archiv.banskabystrica.sk", "archiv"),
        instance("region", "https://data.bbsk.sk", "mesto"),
      ],
    });
    const table = await screen.findByRole("table", { name: en.ckan.instances.title });
    await narrowsAndOrders(table, {
      search: { label: `Search ${en.ckan.instances.title}`, text: "archiv", kept: ["archive"] },
      sort: { column: en.ckan.instances.url, ascending: ["archive", "open-data", "region"] },
      filter: { value: /^mesto/, kept: ["open-data", "region"] },
    });
  });

  it("lists the catalogue by its token reference and never a token (EP-67)", async () => {
    renderCkan();

    const row = (await screen.findByText("open-data")).closest("tr") as HTMLElement;
    expect(within(row).getByText("https://data.banskabystrica.sk")).toBeInTheDocument();
    expect(within(row).getByText("mesto-banska-bystrica")).toBeInTheDocument();
    expect(within(row).getByText("ckan-open-data")).toBeInTheDocument();
    // The form asks for the name of a secret, not for a credential to paste.
    expect(screen.getByText(en.ckan.instances.tokenHelp)).toBeInTheDocument();
    expect(document.querySelector("input[type='password']")).toBeNull();
  });

  it("shows the dataset link and one link per published resource (EP-64)", async () => {
    renderCkan();

    // `ExternalLink` adds "Opens in a new tab" for a screen reader, so the name is a prefix match.
    const dataset = await screen.findByRole("link", { name: /^kvalita-ovzdusia/ });
    expect(dataset).toHaveAttribute(
      "href",
      "https://data.banskabystrica.sk/dataset/kvalita-ovzdusia",
    );
    const entry = dataset.closest("li") as HTMLElement;
    expect(within(entry).getByRole("link", { name: /^NGSI-LD/ })).toHaveAttribute(
      "href",
      `https://bb.example.com/api/endpoint/${SLUG}/ngsi-ld/v1/`,
    );
    expect(within(entry).getByRole("link", { name: /^CSV/ })).toHaveAttribute(
      "href",
      `https://bb.example.com/api/endpoint/${SLUG}/file.csv`,
    );
    // CKAN holds no rows (T-5060): the entry names no sheet.
    expect(within(entry).queryByText(/DataStore|sheet/i)).not.toBeInTheDocument();
    expect(within(entry).getByText("published to open-data")).toBeInTheDocument();
  });

  it("says each publication's phase and what its last run did, a failure in words (T-3091)", async () => {
    renderCkan();
    const live = await screen.findByText("ovzdusie-public");
    const entry = live.closest("li") as HTMLElement;
    expect(within(entry).getByText(/^Live$/i)).toBeInTheDocument();
    // The newest event of that endpoint, not the older failure before it.
    expect(await within(entry).findByTestId("ckan-last-run")).toHaveTextContent(
      /^Last change .*: Endpoint ovzdusie-public is dataset kvalita-ovzdusia\.$/,
    );
    const missing = (await screen.findByText("hluk-public")).closest("li") as HTMLElement;
    const failed = await within(missing).findByTestId("ckan-last-run");
    expect(failed).toHaveTextContent(/^Last run failed .*no CkanInstance 'gone' in this project\. Fix the cause, then publish again\.$/);
    expect(failed).toHaveClass("text-danger");
  });

  it("says which endpoint points at a catalogue that is not there", async () => {
    renderCkan();

    const chip = await screen.findByText("catalogue gone is missing");
    const entry = chip.closest("li") as HTMLElement;
    expect(within(entry).getByText("hluk-public")).toBeInTheDocument();
    expect(within(entry).queryByRole("link")).toBeNull();
  });

  it("proposes a catalogue as a change and never sends a credential (CC-03, MF-24)", async () => {
    const fetchMock = renderCkan();
    const user = userEvent.setup();

    // The three fields the form needs are marked required, so each label reads "Name*" and an
    // exact query finds nothing (T-1765).
    const field = (label: string) => byLabel(label);
    await user.type(await screen.findByLabelText(en.ckan.instances.name, { exact: false }), "open-data-2");
    const url = field(en.ckan.instances.url);
    await user.clear(url);
    await user.type(url, "https://data.example.org");
    await user.type(field(en.ckan.instances.organization), "mesto");
    await user.type(field(en.ckan.instances.tokenRef), "ckan-second");
    await user.click(screen.getByRole("button", { name: en.ckan.instances.propose }));

    const write = await waitFor(() => {
      const request = fetchMock.mock.calls
        .map((call) => call[0] as Request)
        .find((candidate) => candidate.method === "POST");
      expect(request).toBeDefined();
      return request as Request;
    });
    expect(new URL(write.url).pathname).toBe(
      "/api/v1/projects/banskabystrica/ckaninstances",
    );
    const body = JSON.parse(await write.clone().text()) as Record<string, never>;
    expect(body.kind).toBe("CkanInstance");
    expect(body.spec).toMatchObject({
      url: "https://data.example.org",
      organizationDefault: "mesto",
      apiTokenRef: { name: "ckan-second", key: "apiToken" },
    });
    const sent = JSON.stringify(body);
    for (const forbidden of ["apiToken\":\"", "password", "\"token\""]) {
      expect(sent).not.toContain(forbidden);
    }
    // The steward is told where the proposal went, as with every other write.
    expect(await screen.findByText(/chg-0000a1b2/)).toBeInTheDocument();
  });

  it("says so when the project publishes nowhere", async () => {
    renderCkan({ instances: [], publications: [] });

    expect(await screen.findByText(en.ckan.instances.empty)).toBeInTheDocument();
    expect(screen.getByText(en.ckan.publications.empty)).toBeInTheDocument();
  });
});

/**
 * T-2243, UI-44: the catalogue's own control carries the refusal. A viewer used to be offered
 * `Propose catalogue`, enabled, and met the 403 only after filling the form — found by the live walk
 * over every page (`ui/e2e/live/viewer-refused.spec.ts`).
 */
describe("the catalogue control carries the refusal (T-2243)", () => {
  const VIEWER = { project: "banskabystrica", bootstrap: false, grants: [] };
  const PUBLISHER = {
    project: "banskabystrica",
    bootstrap: false,
    grants: [
      {
        role: "catalogue-publisher",
        binding: "publishers",
        rule: { kinds: ["CkanInstance"], verbs: ["propose"] },
      },
    ],
  };

  it("a viewer reads why instead of a form whose submit only refuses (T-4997)", async () => {
    renderCkan(STATUS, VIEWER);
    expect(await screen.findByText(en.form.mayNotCreate)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: en.ckan.instances.propose })).not.toBeInTheDocument();
  });

  it("a publisher is offered it", async () => {
    renderCkan(STATUS, PUBLISHER);
    const control = await waitFor(() =>
      screen.getByRole("button", { name: en.ckan.instances.propose }),
    );
    expect(control).not.toHaveAttribute("aria-disabled", "true");
    expect(control).toBeEnabled();
  });
});
