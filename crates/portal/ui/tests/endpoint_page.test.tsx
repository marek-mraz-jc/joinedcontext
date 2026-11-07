/**
 * One endpoint, the whole window (T-2281, EP-51, UI-26, UI-44, UI-61).
 *
 * 1. Every section is on the page, with the value the manifest carries: the space, the audience and
 *    the allowed projects, the public address, the representations, the hidden attributes, the
 *    policy, the rate limit, the cache lifetime and the file limits.
 * 2. The filtering section reads the `q`, the `scopeQ`, the `geoQ` and the `temporalQ` of the named
 *    projection the endpoint publishes through — an endpoint carries none of its own (MP-01) — and
 *    says so with the projection's name.
 * 3. An endpoint with no projection says the whole space is published and how to narrow it, instead
 *    of showing four empty fields.
 * 4. "Change these settings" goes to the endpoints list with `?edit=<name>`, which opens the kind's
 *    own editor on that endpoint — one form, not a second copy of it.
 * 5. A viewer reads all of it and cannot propose: the control is disabled with the reason, and the
 *    Delete in the menu keeps its reason too (UI-44).
 * 6. The page says a change is proposed and reviewed before any control.
 * 7. An endpoint has no pause: the page says deleting is how it stops and opens that deletion, which
 *    a viewer is refused with its reason; no pause control is painted (EP-89, T-2286).
 */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";

const PROJECT = "banskabystrica";
const NAME = "ovzdusie-public";
const SLUG = "k7m2qz4tv6xh3n5jb2ryd3wcfa";

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  email: "jana.kovacova@banskabystrica.sk",
  roles: ["portal-editor"],
};

const ENDPOINT = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Endpoint",
  metadata: {
    name: NAME,
    namespace: PROJECT,
    title: "Air quality, open",
    labels: { "joinedcontext.com/space": "ovzdusie" },
  },
  spec: {
    contextSpaceRef: "ovzdusie",
    slug: SLUG,
    audience: "project-list",
    allowedProjects: ["kosice"],
    policyRef: "urn:ngsi-ld:Policy:banskabystrica.sk:ovzdusie:readers",
    enabledRepresentations: ["ngsi-ld", "csv"],
    rateLimits: { requestsPerMinute: 120, burst: 20 },
    caching: { maxAgeSeconds: 60 },
    fileLimits: { maxFileRows: 50000 },
    projection: { hiddenAttributes: ["reportedBy"] },
    projectionRef: { kind: "ModelProjection", name: "ovzdusie-open" },
  },
  status: { phase: "Live" },
};

/** The Policy `spec.policyRef` names: same space, manifest name `readers` (EP-14, GW8). */
const POLICY = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Policy",
  metadata: { name: "readers", namespace: PROJECT },
  spec: {
    contextSpaceRef: "ovzdusie",
    assigner: "did:web:banskabystrica.sk",
    assignee: { kind: "role", id: "public" },
    operations: ["retrieveOps"],
    information: [{ entities: [{ type: "AirQualityObserved" }] }],
  },
};

const SPACE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ContextSpace",
  metadata: { name: "ovzdusie", namespace: PROJECT },
  // The URN segment the endpoint's `policyRef` carries, pinned as the space does it.
  spec: { urnSegment: "ovzdusie" },
};

const PROJECTION = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ModelProjection",
  metadata: { name: "ovzdusie-open", namespace: PROJECT },
  spec: {
    contextSpaceRef: "ovzdusie",
    dataModelRef: "ovzdusie",
    classes: [{ name: "AirQualityObserved", slots: ["pm10"] }],
    filter: { q: "pm10>30", scopeQ: "/Banska Bystrica/Centrum" },
  },
};

function list(items: unknown[]) {
  return { apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items };
}

const PERMITTED = {
  project: PROJECT,
  bootstrap: false,
  grants: [
    {
      role: "endpoint-editor",
      binding: "editors",
      rule: { kinds: ["Endpoint", "ModelProjection"], verbs: ["propose", "delete"] },
    },
  ],
};

/** A viewer: they read everything and propose nothing (UI-44). */
const VIEWER = { project: PROJECT, bootstrap: false, grants: [] };

/**
 * The calls one path received, with the query and the body as they went (T-2281). The API client
 * sends a `Request`, whose body is only readable as a promise, so this is awaited.
 */
async function sentTo(fetchMock: ReturnType<typeof vi.fn>, path: string) {
  const calls = fetchMock.mock.calls.map(([input, init]) => ({
    url: new URL(input instanceof Request ? input.url : String(input), window.location.origin),
    text:
      input instanceof Request
        ? input.clone().text()
        : Promise.resolve(String((init as RequestInit | undefined)?.body ?? "{}")),
  }));
  return Promise.all(
    calls
      .filter((call) => call.url.pathname.endsWith(path))
      .map(async (call) => ({
        url: call.url,
        body: JSON.parse((await call.text) || "{}") as {
          spec?: { filter?: Record<string, string>; classes?: unknown };
          status?: unknown;
        },
      })),
  );
}

function renderPage({
  endpoint = ENDPOINT,
  projections = [PROJECTION],
  policies = [POLICY],
  permissions = PERMITTED,
  check = { ok: true },
  counts = { all: 3, matching: 1 },
}: {
  endpoint?: unknown;
  projections?: unknown[];
  policies?: unknown[];
  permissions?: unknown;
  /** What the mandatory dry run answers (PF-57). */
  check?: { ok: boolean; message?: string };
  /** What a counted read through the endpoint answers, with and without the filter (T-2283). */
  counts?: { all: number; matching: number } | null;
} = {}) {
  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    const url = new URL(input instanceof Request ? input.url : String(input), window.location.origin);
    const path = url.pathname;
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
    // Who reads the Endpoint is the Organization administrators' (T-6412): this caller is refused.
    if (path === "/api/v1/organization/data-access") {
      return json({ title: "Forbidden", status: 403, detail: "read on Person" }, 403);
    }
    if (path.startsWith(`/api/endpoint/${SLUG}/ngsi-ld/v1/entities`)) {
      if (counts === null) {
        return Promise.resolve(new Response("{}", { status: 403 }));
      }
      const asked = url.searchParams.get("q");
      return Promise.resolve(
        new Response(JSON.stringify([]), {
          status: 200,
          headers: {
            "Content-Type": "application/ld+json",
            "NGSILD-Results-Count": String(asked ? counts.matching : counts.all),
          },
        }),
      );
    }
    if (path.endsWith("/permissions/me")) {
      return json(permissions);
    }
    // The filter proof's left side (T-3802): nothing under the draft, so the page shows no read error.
    if (path.endsWith(`/endpoints/${NAME}/preview`)) {
      return json({ entities: [], count: 0 });
    }
    // T-5118: the page's Usage section, with nobody having called yet.
    if (path.endsWith(`/endpoints/${NAME}/usage`)) {
      return json({ name: NAME, from: "2026-09-23", to: "2026-10-22", days: [], totals: {}, total: 0 });
    }
    // T-5531: the page's "Who may do what", with no Policy naming anyone yet.
    if (path.endsWith(`/endpoints/${NAME}/access/matrix`)) {
      return json({ endpoint: NAME, space: "air", principals: [], types: [], cells: [], edges: [] });
    }
    if (path.endsWith(`/endpoints/${NAME}`)) {
      return endpoint === null ? json({ title: "Not Found", detail: "no such endpoint" }, 404) : json(endpoint);
    }
    if (path.includes("/projections/")) {
      if (url.searchParams.get("dryRun") === "All") {
        return json(
          check.ok
            ? { valid: true, verdict: { ok: true, findings: [] } }
            : { valid: false, verdict: { ok: false, findings: [{ message: check.message }] } },
        );
      }
      return json({ kind: "Change", metadata: { name: "chg-1" }, spec: {}, status: { phase: "Pending" } });
    }
    if (path.endsWith("/projections")) {
      return json(list(projections));
    }
    if (path.endsWith("/policies")) {
      return json(list(policies));
    }
    if (path.endsWith("/spaces")) {
      return json(list([SPACE]));
    }
    if (path.endsWith("/endpoints")) {
      return json(list(endpoint === null ? [] : [endpoint]));
    }
    return json(list([]));
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

describe("the endpoint's own settings page", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", `/projects/${PROJECT}/endpoints/${NAME}`);
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("holds every setting the manifest carries, one section per aspect", async () => {
    renderPage();

    expect(await screen.findByRole("heading", { name: "Air quality, open" })).toBeInTheDocument();
    for (const section of [
      en.endpoints.page.filtering,
      en.endpoints.page.whoMayCall,
      en.endpoints.page.traffic,
    ]) {
      expect(screen.getByRole("heading", { name: section })).toBeInTheDocument();
    }

    // The share is managed further down (T-3275).
    expect(screen.getAllByText("kosice").length).toBeGreaterThanOrEqual(1);
    expect(screen.getByRole("heading", { name: en.endpoints.shareFlow.section })).toBeInTheDocument();

    // What it answers is one row of links under the header (owner 2026-10-30).
    expect(screen.getByRole("link", { name: "ngsi-ld" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "csv" })).toBeInTheDocument();

    // Who may call it: the policy the URN names, what it grants, and to whom (T-2282, GW34).
    expect(screen.getByRole("link", { name: "readers" })).toBeInTheDocument();
    expect(screen.getByText(en.choice.operationGroup.retrieveOps)).toBeInTheDocument();
    expect(screen.getByText("retrieveEntity, queryEntity")).toBeInTheDocument();
    expect(screen.getByText(en.endpoints.page.grantChanges.nothing)).toBeInTheDocument();
    expect(screen.getByTitle("role:public")).toHaveTextContent("public");
    expect(screen.getByText("120 requests a minute · burst 20")).toBeInTheDocument();
    expect(screen.getByText("A caller may cache an answer for 60 seconds.")).toBeInTheDocument();
    expect(screen.getByText("At most 50000 rows in a file")).toBeInTheDocument();

    // The owner took the review sentence and the fact sections off the page (2026-10-30).
    expect(screen.queryByText(en.endpoints.page.proposedNotice)).not.toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: en.endpoints.page.whatItIs })).not.toBeInTheDocument();
  });

  it("offers the one connector for every endpoint, never this endpoint's own slug in it (T-4114, EP-113)", async () => {
    renderPage();
    const panel = (await screen.findByRole("heading", { name: en.endpoints.hub.title })).closest(
      "section",
    ) as HTMLElement;
    expect(within(panel).getByText(`${window.location.origin}/api/mcp`, { selector: "code" })).toBeInTheDocument();
    expect(panel.textContent).not.toContain(SLUG);
  });

  it("reads the filtering from the named projection it publishes through", async () => {
    renderPage();

    expect(await screen.findByRole("heading", { name: en.endpoints.page.filtering })).toBeInTheDocument();
    expect(screen.getByLabelText(en.endpoints.filter.q)).toHaveValue("pm10>30");
    expect(screen.getByLabelText(en.endpoints.filter.scopeQ)).toHaveValue("/Banska Bystrica/Centrum");
    // Every condition says what it narrows, and an empty one shows it narrows nothing.
    expect(screen.getByText(en.endpoints.filter.qHelp)).toBeInTheDocument();
    expect(screen.getByLabelText(en.endpoints.filter.geoQ)).toHaveValue("");
    expect(screen.getByRole("link", { name: "ovzdusie-open" })).toBeInTheDocument();
  });

  it("proposes a changed filter on the projection, checked first, and nothing else of it", async () => {
    const fetchMock = renderPage();
    const user = userEvent.setup();

    const q = await screen.findByLabelText(en.endpoints.filter.q);
    // Untouched, there is nothing to propose.
    // Disabled with its reason (T-3852): aria-disabled, so the reason stays reachable.
    expect(screen.getByRole("button", { name: en.endpoints.page.filterPropose })).toHaveAttribute(
      "aria-disabled",
      "true",
    );

    await user.clear(q);
    await user.type(q, "pm10>50");
    await user.click(screen.getByRole("button", { name: en.endpoints.page.filterPropose }));

    await waitFor(async () => {
      expect(await sentTo(fetchMock, "/projections/ovzdusie-open")).toHaveLength(2);
    });
    const [dryRun, real] = await sentTo(fetchMock, "/projections/ovzdusie-open");
    // The check comes first, on the same route and body, and only a green one proposes (PF-57).
    expect(dryRun.url.searchParams.get("dryRun")).toBe("All");
    expect(real.url.searchParams.get("dryRun")).toBeNull();
    expect(real.body.spec?.filter).toEqual({ q: "pm10>50", scopeQ: "/Banska Bystrica/Centrum" });
    // The rest of the projection travels as it was, and the status the platform computes does not.
    expect(real.body.spec?.classes).toEqual(PROJECTION.spec.classes);
    expect(real.body.status).toBeUndefined();
  });

  it("removes a condition it was asked to empty instead of storing an empty query", async () => {
    const fetchMock = renderPage();
    const user = userEvent.setup();

    await user.clear(await screen.findByLabelText(en.endpoints.filter.scopeQ));
    await user.click(screen.getByRole("button", { name: en.endpoints.page.filterPropose }));

    await waitFor(async () => {
      expect(await sentTo(fetchMock, "/projections/ovzdusie-open")).toHaveLength(2);
    });
    const [, real] = await sentTo(fetchMock, "/projections/ovzdusie-open");
    expect(real.body.spec?.filter).toEqual({ q: "pm10>30" });
  });

  it("builds a condition instead of asking a person to type NGSI-LD (T-2283)", async () => {
    const fetchMock = renderPage();
    const user = userEvent.setup();

    // The attributes offered are the ones the projection publishes.
    const attribute = await screen.findByLabelText(en.endpoints.condition.attribute);
    expect([...(attribute as HTMLSelectElement).options].map((option) => option.value)).toEqual(["pm10"]);

    await user.selectOptions(screen.getByLabelText(en.endpoints.condition.operator), "gt");
    await user.type(screen.getByLabelText(en.endpoints.condition.value), "50");
    await user.click(screen.getByRole("button", { name: en.endpoints.condition.add }));

    // Appended to what was there, with `;`, which is what the gateway reads as "and": the stored query
    // is never rewritten from a guess, so a filter nobody could parse cannot be lost.
    expect(screen.getByLabelText(en.endpoints.filter.q)).toHaveValue("pm10>30;pm10>50");

    await user.click(screen.getByRole("button", { name: en.endpoints.page.filterPropose }));
    await waitFor(async () => {
      expect(await sentTo(fetchMock, "/projections/ovzdusie-open")).toHaveLength(2);
    });
    const [, real] = await sentTo(fetchMock, "/projections/ovzdusie-open");
    expect(real.body.spec?.filter?.q).toBe("pm10>30;pm10>50");
  });

  it("refuses a condition with no value at the field, and adds nothing", async () => {
    renderPage();
    const user = userEvent.setup();

    await user.click(await screen.findByRole("button", { name: en.endpoints.condition.add }));

    expect(await screen.findByRole("alert")).toHaveTextContent(en.endpoints.condition.needsValue);
    expect(screen.getByLabelText(en.endpoints.filter.q)).toHaveValue("pm10>30");
    // Nothing to propose either: the query is what it was.
    // Disabled with its reason (T-3852): aria-disabled, so the reason stays reachable.
    expect(screen.getByRole("button", { name: en.endpoints.page.filterPropose })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
  });

  it("quotes a value that is not a number, so a typed word cannot break the query", async () => {
    renderPage();
    const user = userEvent.setup();

    await user.type(await screen.findByLabelText(en.endpoints.condition.value), 'Kamppi "1"');
    await user.click(screen.getByRole("button", { name: en.endpoints.condition.add }));

    // The value is quoted and its own quotes escaped, by the SDK's compiler, so a typed word cannot
    // end the literal early and change what the query means.
    expect(screen.getByLabelText(en.endpoints.filter.q)).toHaveValue(
      `pm10>30;pm10=="Kamppi \\"1\\""`,
    );
  });

  it("says how many entities the filter matches, read through the endpoint itself (T-2283)", async () => {
    const fetchMock = renderPage();

    expect(await screen.findByText("1 of 3 entities match this filter")).toBeInTheDocument();
    // Both counts go through this endpoint's own URL, under the reader's own rights, and ask for the
    // projection's type, because NGSI-LD refuses a query with no selector.
    const reads = fetchMock.mock.calls
      .map(([input]) => new URL(input instanceof Request ? input.url : String(input), window.location.origin))
      .filter((url) => url.pathname.startsWith(`/api/endpoint/${SLUG}/ngsi-ld/v1/entities`));
    expect(reads).toHaveLength(2);
    expect(reads.every((url) => url.searchParams.get("type") === "AirQualityObserved")).toBe(true);
    expect(reads.every((url) => url.searchParams.get("count") === "true")).toBe(true);
    expect(reads.map((url) => url.searchParams.get("q"))).toEqual([null, "pm10>30"]);
  });

  // F-R2UI2-008: every keystroke in the query box was two counted reads through the gateway.
  it("counts a typed filter once the typing pauses, and the whole type only once", async () => {
    const fetchMock = renderPage();
    const user = userEvent.setup();
    await screen.findByText("1 of 3 entities match this filter");
    await user.type(screen.getByLabelText(en.endpoints.filter.q), ";pm10<90");
    await waitFor(() => {
      const asked = fetchMock.mock.calls
        .map(([input]) => new URL(input instanceof Request ? input.url : String(input), window.location.origin))
        .filter((url) => url.pathname.startsWith(`/api/endpoint/${SLUG}/ngsi-ld/v1/entities`))
        .map((url) => url.searchParams.get("q"));
      expect(asked).toEqual([null, "pm10>30", "pm10>30;pm10<90"]);
    });
  });

  it("warns before it is proposed that a filter matching nothing publishes nothing", async () => {
    renderPage({ counts: { all: 3, matching: 0 } });

    const verdict = await screen.findByText(/0 of 3 entities match this filter/);
    expect(verdict).toHaveTextContent(en.endpoints.count.none);
  });

  it("counts nothing for an endpoint that is not Live, and says why", async () => {
    const fetchMock = renderPage({ endpoint: { ...ENDPOINT, status: { phase: "Pending" } } });

    expect(await screen.findByText(en.endpoints.count.unavailable)).toBeInTheDocument();
    // An endpoint that serves nothing is not read: no number, and no request either.
    expect(
      fetchMock.mock.calls.some(([input]) =>
        (input instanceof Request ? input.url : String(input)).includes("/ngsi-ld/v1/entities"),
      ),
    ).toBe(false);
  });

  it("says why the count could not be read instead of showing a zero", async () => {
    renderPage({ counts: null });

    expect(await screen.findByText(/could not be counted/)).toBeInTheDocument();
    expect(screen.queryByText(/entities match this filter/)).not.toBeInTheDocument();
  });

  it("refuses a scope that is not a path and an area missing a part, at the field (T-2283, UI-45)", async () => {
    const fetchMock = renderPage();
    const user = userEvent.setup();

    const scope = await screen.findByLabelText(en.endpoints.filter.scopeQ);
    await user.clear(scope);
    await user.type(scope, "Kamppi");
    expect(await screen.findByText(en.endpoints.filter.fault.scopePath)).toBeInTheDocument();
    expect(scope).toHaveAttribute("aria-invalid", "true");
    // Disabled with its reason (T-3852): aria-disabled, so the reason stays reachable.
    expect(screen.getByRole("button", { name: en.endpoints.page.filterPropose })).toHaveAttribute(
      "aria-disabled",
      "true",
    );

    await user.clear(scope);
    await user.type(scope, "/Helsinki/Kamppi");
    expect(screen.queryByText(en.endpoints.filter.fault.scopePath)).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: en.endpoints.page.filterPropose })).toBeEnabled();

    // An area without its three parts cannot be read by the gateway, so it is not sent either.
    await user.type(await screen.findByLabelText(en.endpoints.filter.geoQ), "georel=near");
    expect(await screen.findByText(en.endpoints.filter.fault.geoParts)).toBeInTheDocument();
    // Disabled with its reason (T-3852): aria-disabled, so the reason stays reachable.
    expect(screen.getByRole("button", { name: en.endpoints.page.filterPropose })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
    expect(await sentTo(fetchMock, "/projections/ovzdusie-open")).toHaveLength(0);
  });

  it("sets the area from four corners, through the SDK's own geo query (T-2283, UI-72)", async () => {
    const fetchMock = renderPage();
    const user = userEvent.setup();

    // The corners the wrong way round are refused, and nothing is written.
    await user.type(await screen.findByLabelText(en.endpoints.area.west), "21.5");
    await user.type(screen.getByLabelText(en.endpoints.area.south), "49.0");
    await user.type(screen.getByLabelText(en.endpoints.area.east), "19.0");
    await user.type(screen.getByLabelText(en.endpoints.area.north), "48.5");
    await user.click(screen.getByRole("button", { name: en.endpoints.area.set }));
    expect(await screen.findByText(en.endpoints.area.fault.order)).toBeInTheDocument();
    expect(screen.getByLabelText(en.endpoints.filter.geoQ)).toHaveValue("");

    // Corrected, the box becomes the four parts a gateway can read, closed and counter-clockwise.
    await user.clear(screen.getByLabelText(en.endpoints.area.east));
    await user.type(screen.getByLabelText(en.endpoints.area.east), "22.0");
    await user.clear(screen.getByLabelText(en.endpoints.area.north));
    await user.type(screen.getByLabelText(en.endpoints.area.north), "49.5");
    await user.click(screen.getByRole("button", { name: en.endpoints.area.set }));

    expect(screen.getByLabelText(en.endpoints.filter.geoQ)).toHaveValue(
      "georel=within;geometry=Polygon;coordinates=[[[21.5,49],[22,49],[22,49.5],[21.5,49.5],[21.5,49]]];geoproperty=location",
    );
    // A written area is a proposal like any other, and the field's own check no longer objects.
    await user.click(screen.getByRole("button", { name: en.endpoints.page.filterPropose }));
    await waitFor(async () => {
      expect(await sentTo(fetchMock, "/projections/ovzdusie-open")).toHaveLength(2);
    });
    const [, real] = await sentTo(fetchMock, "/projections/ovzdusie-open");
    expect(real.body.spec?.filter?.geoQ).toContain("georel=within");
  });

  it("says what a red check found and proposes nothing", async () => {
    const fetchMock = renderPage({ check: { ok: false, message: "scopeQ is not a scope path" } });
    const user = userEvent.setup();

    await user.type(await screen.findByLabelText(en.endpoints.filter.scopeQ), "?");
    await user.click(screen.getByRole("button", { name: en.endpoints.page.filterPropose }));

    expect(await screen.findByRole("alert")).toHaveTextContent("scopeQ is not a scope path");
    // One call, the check; the real one never went.
    expect(await sentTo(fetchMock, "/projections/ovzdusie-open")).toHaveLength(1);
  });

  it("says the whole space is published when there is no projection to filter with", async () => {
    const spec = { ...ENDPOINT.spec };
    delete (spec as { projectionRef?: unknown }).projectionRef;
    renderPage({ endpoint: { ...ENDPOINT, spec } });

    expect(await screen.findByText(en.endpoints.page.noProjection)).toBeInTheDocument();
    expect(screen.queryByText(en.endpoints.filter.qHelp)).not.toBeInTheDocument();
  });

  it("links Your access here to the Data access table opened on this endpoint (T-5593)", async () => {
    renderPage();
    expect(await screen.findByRole("link", { name: en.projectSettings.access.title })).toHaveAttribute(
      "href",
      `/projects/${PROJECT}/settings/access?endpoint=${NAME}`,
    );
  });

  it("opens the kind's own editor on this endpoint at its edit address", async () => {
    renderPage();
    const user = userEvent.setup();

    await user.click(await screen.findByRole("button", { name: en.endpoints.page.change }));

    // The form is a page of its own (T-2474), so the change has an address to reload and share.
    await waitFor(() => {
      expect(window.location.pathname).toBe(`/projects/${PROJECT}/endpoints/${NAME}/edit`);
    });
    // The list's editor opens on that endpoint: its name is in the form, not an empty one.
    expect(await screen.findByDisplayValue(NAME)).toBeInTheDocument();
  });

  it("lets a viewer read every setting and refuses the change with its reason", async () => {
    renderPage({ permissions: VIEWER });

    expect(await screen.findByRole("heading", { name: "Air quality, open" })).toBeInTheDocument();
    expect(screen.getByLabelText(en.endpoints.filter.q)).toHaveValue("pm10>30");

    // The guard remounts the control once the document arrives, so it is queried after the wait.
    await waitFor(() => {
      expect(screen.getByRole("button", { name: en.endpoints.page.change })).toHaveAttribute(
        "aria-disabled",
        "true",
      );
    });
    // Every door says which verb on which kind is missing: the endpoint's settings, its
    // publication to the catalogue (EP-83) and the filter.
    expect(screen.getByRole("button", { name: en.catalogue.publish.open })).toHaveAttribute("aria-disabled", "true");
    expect(screen.getAllByTitle(/'propose' on 'Endpoint'/)).toHaveLength(2);
    expect(screen.getByTitle(/'propose' on 'ModelProjection'/)).toBeInTheDocument();
  });

  it("keeps the rest of the actions in one menu, and a denied one with its reason", async () => {
    renderPage({ permissions: VIEWER });
    const user = userEvent.setup();

    await waitFor(() => {
      expect(screen.getByRole("button", { name: en.endpoints.page.change })).toHaveAttribute(
        "aria-disabled",
        "true",
      );
    });
    await user.click(screen.getByRole("button", { name: /More actions for/ }));
    const menu = await screen.findByRole("menu");
    expect(within(menu).getByRole("menuitem", { name: en.endpoints.copyUrl })).toBeInTheDocument();
    // The accessible name carries the reason too, so the item is matched by pattern (T-2279).
    const remove = within(menu).getByRole("menuitem", { name: new RegExp(en.resourceDelete.button) });
    expect(remove).toHaveAttribute("aria-disabled", "true");
    expect(remove).toHaveAttribute("title", expect.stringContaining("delete"));
  });

  it("says an endpoint stops by being deleted and opens that deletion (EP-89)", async () => {
    renderPage();
    const user = userEvent.setup();

    const section = (await screen.findByRole("heading", { name: en.endpoints.page.stop.title })).closest("section");
    expect(section).not.toBeNull();
    expect(within(section as HTMLElement).getByText(en.endpoints.page.stop.lead)).toBeInTheDocument();
    // No control pretends to pause what the manifest cannot pause.
    expect(screen.queryByRole("button", { name: /pause|resume|retire/i })).not.toBeInTheDocument();

    await user.click(within(section as HTMLElement).getByRole("button", { name: en.endpoints.page.stop.delete }));
    // The same deletion as the menu's: the name typed back, then a change for an approver.
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByRole("button", { name: new RegExp(en.resourceDelete.propose) })).toBeInTheDocument();
  });

  it("refuses a viewer the deletion that stops an endpoint, with its reason (EP-89)", async () => {
    renderPage({ permissions: VIEWER });

    await waitFor(() => {
      expect(screen.getByRole("button", { name: new RegExp(en.endpoints.page.stop.delete) })).toHaveAttribute(
        "aria-disabled",
        "true",
      );
    });
    expect(screen.getByRole("button", { name: new RegExp(en.endpoints.page.stop.delete) })).toHaveAttribute(
      "title",
      expect.stringContaining("'delete' on 'Endpoint'"),
    );
  });

  it("says why an endpoint could not be read instead of an empty page", async () => {
    renderPage({ endpoint: null });

    expect(await screen.findByRole("alert")).toHaveTextContent("no such endpoint");
    // A 404 answers the same the second time (T-2834): no Retry, the heading and the way back.
    expect(screen.queryByRole("button", { name: en.app.error.retry })).toBeNull();
    expect(screen.getByRole("heading", { level: 1 })).toBeInTheDocument();
    expect(screen.getAllByRole("link", { name: en.endpoints.page.back }).length).toBeGreaterThan(0);
  });
});
