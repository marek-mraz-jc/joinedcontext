/**
 * "Generate your own app" (T-0312, AP-22, AP-30, AG-26).
 *
 * The two things worth asserting are the ones a user cannot check for themselves: that the
 * app's grant is derived from the endpoint it is bound to and can only be narrowed, and that
 * a deployment without a builder agent says so rather than opening a merge request nothing
 * will pick up.
 */
// covers (T-2137, the module gate in gate_modules.test.ts): the cases in this file drive
// src/pages/apps/EndpointPreview.tsx through the page they belong to; each was confirmed by
// making the module throw and watching this file go red.
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { rememberPrefill } from "../src/assistant/state";
import { expectDenied } from "./checks";
import en from "../src/locales/en.json";
import { App } from "../src/App";
import {
  ACCESS_PRESETS,
  BLUEPRINT,
  dataNeeds,
  offersPreset,
  presetOperations,
} from "../src/pages/apps/AppGenerator";

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  email: "jana.kovacova@banskabystrica.sk",
  roles: ["domain-editor"],
};

const PROJECT = "banskabystrica";
const SLUG = "k7m2qz4tv6xh3n5jb2ryd3wcfa";

const BLUEPRINT_CARD = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Blueprint",
  metadata: { name: BLUEPRINT, namespace: "org", title: { en: "App from a prompt" } },
  spec: { version: "1.4.0", riskClass: "yellow" },
};

const ENDPOINT = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Endpoint",
  metadata: { name: "ovzdusie-public", namespace: PROJECT, title: { en: "Air quality open data" } },
  spec: {
    contextSpaceRef: "ovzdusie",
    slug: SLUG,
    audience: "public",
    enabledRepresentations: ["ngsi-ld", "geojson"],
  },
};

const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-0000c3d4", namespace: PROJECT },
  status: { lane: "yellow", phase: "PendingApproval", plan: { create: 2 } },
};

/** What `POST …/agent-runs` answers with: the queued run, plus the ticket only the caller sees. */
const CREATED_RUN = {
  id: "01J8ZQ4T7K9M2N3P4Q5R6S7T8V",
  project: PROJECT,
  appName: "ovzdusie-dnes",
  endpointName: "ovzdusie-public",
  appClass: "ui-rust",
  visibility: "project",
  prompt: "A map of the stations with today's PM10",
  status: "queued",
  steps: 0,
  tokensUsed: 0,
  createdBy: IDENTITY.username,
  createdAt: "2026-09-12T08:00:00Z",
};

/** The endpoint's AuthZEN document, as the gateway's `/access` serves it to this user. */
const GRANT = {
  subject: { type: "user", id: "jana.kovacova" },
  permissions: [
    {
      resource: { type: "AirQualityObserved" },
      actions: ["queryEntity", "retrieveEntity"],
      attributes: ["location", "name", "pm10"],
    },
  ],
  prohibitions: [{ resource: { type: "AirQualityObserved" }, attributes: ["internalNote"] }],
};

/** Five entities as the gateway serves them with `options=keyValues`. */
const ENTITIES = [1, 2, 3, 4, 5].map((index) => ({
  id: `urn:ngsi-ld:AirQualityObserved:banskabystrica:ovzdusie:st-${index}`,
  type: "AirQualityObserved",
  name: `Station ${index}`,
  pm10: 12 + index,
}));

/** The projected draft-07 document the endpoint serves, which is where the bounds come from. */
const SCHEMA = {
  $schema: "http://json-schema.org/draft-07/schema#",
  $defs: {
    AirQualityObserved: {
      properties: { pm10: {}, pm25: {}, location: {}, name: {} },
    },
  },
};

interface Options {
  blueprints?: unknown[];
  endpoints?: unknown[];
  schemaStatus?: number;
  write?: { body: unknown; status: number };
  runStatus?: string;
  grantStatus?: number;
  grant?: unknown;
  entities?: { body: unknown; status: number };
}

function renderGenerator(options: Options = {}) {
  const {
    blueprints = [BLUEPRINT_CARD],
    endpoints = [ENDPOINT],
    schemaStatus = 200,
    write = { body: CREATED_RUN, status: 202 },
    runStatus = "queued",
    grantStatus = 200,
    grant = GRANT,
    entities = { body: ENTITIES, status: 200 },
  } = options;

  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    const request = input as Request;
    const url = new URL(request.url, "http://localhost");
    const json = (body: unknown, status = 200) =>
      Promise.resolve(
        new Response(JSON.stringify(body), {
          status,
          headers: {
            "Content-Type": status >= 400 ? "application/problem+json" : "application/json",
          },
        }),
      );

    if (url.pathname.endsWith("/auth/me")) {
      return json(IDENTITY);
    }
    if (url.pathname.endsWith("/api/v1/blueprints")) {
      return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: blueprints });
    }
    if (url.pathname.endsWith("/endpoints") && request.method === "GET") {
      return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: endpoints });
    }
    if (url.pathname.endsWith("/access")) {
      return json(grant, grantStatus);
    }
    if (url.pathname.includes("/ngsi-ld/v1/entities")) {
      return json(entities.body, entities.status);
    }
    if (url.pathname.includes("/schema/index.json")) {
      return json({ models: [{ name: "bb-air-quality", version: 2 }] }, schemaStatus);
    }
    if (url.pathname.includes("/schema/v2/json-schema")) {
      return json(SCHEMA, schemaStatus);
    }
    if (url.pathname.endsWith("/agent-runs") && request.method === "POST") {
      return json(write.body, write.status);
    }
    if (url.pathname.endsWith("/publish")) {
      return json(CHANGE, 202);
    }
    if (url.pathname.endsWith("/agent-runs") && request.method === "GET") {
      return json({ items: [{ ...CREATED_RUN, status: runStatus }] });
    }
    if (url.pathname.includes("/agent-runs/") && request.method === "GET") {
      return json({ ...CREATED_RUN, status: runStatus });
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

/** The route a user takes to the form: the apps catalogue, then its own button. */
async function openGenerator(user: ReturnType<typeof userEvent.setup>) {
  await user.click(await screen.findByRole("button", { name: en.apps.newAction }));
}

/** The body of the one write the form makes. */
async function runBody(fetchMock: ReturnType<typeof vi.fn>): Promise<Record<string, unknown>> {
  const request = fetchMock.mock.calls
    .map((call) => call[0] as Request)
    .find((candidate) => candidate.method === "POST");
  expect(request).toBeDefined();
  return (await request!.json()) as Record<string, unknown>;
}

/** Fills the form the way a user would, and waits for the derived needs to arrive. */
async function fill(user: ReturnType<typeof userEvent.setup>) {
  await user.type(screen.getByLabelText(en.apps.generate.name, { exact: false }), "ovzdusie-dnes");
  await user.type(
    screen.getByLabelText(en.apps.generate.prompt, { exact: false }),
    "A map of the stations with today's PM10",
  );
  await user.selectOptions(screen.getByLabelText(en.apps.generate.endpoint, { exact: false }), "ovzdusie-public");
  await screen.findByRole("group", { name: "AirQualityObserved" });
}

describe("the app generator", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", `/projects/${PROJECT}/apps`);
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("collects the prompt, the name and the kind", async () => {
    const user = userEvent.setup();
    renderGenerator();
    await openGenerator(user);
    await screen.findByLabelText(en.apps.generate.prompt, { exact: false });

    await user.type(screen.getByLabelText(en.apps.generate.prompt, { exact: false }), "Show me the buses");

    expect(screen.getByLabelText(en.apps.generate.prompt, { exact: false })).toHaveValue("Show me the buses");
    // The kit pass is the fast path, so it is what the form starts on (AP-56).
    expect(screen.getByLabelText(en.apps.generate.kind, { exact: false })).toHaveValue("ui");
  });

  it("fills the endpoint list from the project's own endpoints", async () => {
    const user = userEvent.setup();
    renderGenerator({
      endpoints: [
        ENDPOINT,
        {
          ...ENDPOINT,
          metadata: { ...ENDPOINT.metadata, name: "doprava", title: { en: "Transport" } },
        },
      ],
    });
    await openGenerator(user);

    const select = await screen.findByLabelText(en.apps.generate.endpoint, { exact: false });
    const options = within(select).getAllByRole("option");

    expect(options.map((option) => option.textContent)).toEqual([
      en.apps.generate.pickEndpoint,
      "Air quality open data",
      "Transport",
    ]);
  });

  it("derives the readable attributes from the chosen endpoint's own schema (AP-22)", async () => {
    const user = userEvent.setup();
    renderGenerator();
    await openGenerator(user);
    await screen.findByLabelText(en.apps.generate.endpoint, { exact: false });

    await user.selectOptions(screen.getByLabelText(en.apps.generate.endpoint, { exact: false }), "ovzdusie-public");

    const needs = await screen.findByRole("group", { name: "AirQualityObserved" });
    for (const attribute of ["pm10", "pm25", "location", "name"]) {
      expect(within(needs).getByLabelText(attribute)).toBeChecked();
    }
    // The endpoint's audience, not a field: an app is never wider than the data behind it.
    expect(
      screen.getByText(en.apps.generate.needs.audience.replace("{audience}", "public")),
    ).toBeInTheDocument();
  });

  it("sends the confirmed list, narrowed by what the user unticked", async () => {
    const user = userEvent.setup();
    const fetchMock = renderGenerator();
    await openGenerator(user);
    await screen.findByLabelText(en.apps.generate.endpoint, { exact: false });
    await fill(user);

    await user.click(screen.getByLabelText("pm25"));
    await user.click(screen.getByRole("button", { name: en.apps.generate.submit }));

    await waitFor(async () => {
      const body = await runBody(fetchMock);
      expect(body.appName).toBe("ovzdusie-dnes");
      expect(body.appClass).toBe("ui");
      expect(body.endpointName).toBe("ovzdusie-public");
      expect(body.prompt).toBe("A map of the stations with today's PM10");
      const needs = body.dataNeeds as { attrs: string[]; operations: string[] }[];
      expect(needs[0].attrs).toEqual(["location", "name", "pm10"]);
      expect(needs[0].operations).toEqual(["queryEntity", "retrieveEntity"]);
    });
  });

  // T-3575: a person who wants server-side state in Rust picks the wasm kind, and the run is one.
  it("offers the wasm kind and sends it as the run's appClass", async () => {
    const user = userEvent.setup();
    const fetchMock = renderGenerator();
    await openGenerator(user);
    await screen.findByLabelText(en.apps.generate.endpoint, { exact: false });
    await fill(user);

    await user.selectOptions(screen.getByLabelText(en.apps.generate.kind), "wasm");
    expect(screen.getByRole("option", { name: en.apps.generate.kinds.wasm })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: en.apps.generate.submit }));

    await waitFor(async () => {
      expect((await runBody(fetchMock)).appClass).toBe("wasm");
    });
  });

  it("reads a second endpoint the person adds and sends endpointNames, the primary first (AP-44)", async () => {
    const kpiSlug = "q3mzkq2v7w5ayxcbn4ltdj6hof";
    const kpis = {
      ...ENDPOINT,
      metadata: { ...ENDPOINT.metadata, name: "ovzdusie-kpi", title: { en: "Air quality indicators" } },
      spec: { ...ENDPOINT.spec, contextSpaceRef: "ovzdusie-kpi", slug: kpiSlug },
    };
    const user = userEvent.setup();
    const fetchMock = renderGenerator({ endpoints: [ENDPOINT, kpis] });
    await openGenerator(user);
    await screen.findByLabelText(en.apps.generate.endpoint, { exact: false });
    await fill(user);

    await user.click(screen.getByRole("button", { name: en.apps.generate.addEndpoint }));
    const group = screen.getByRole("group", { name: en.apps.generate.moreEndpoints });
    expect(within(group).queryByLabelText("Air quality open data")).toBeNull();
    await user.click(within(group).getByLabelText("Air quality indicators"));
    await waitFor(() => {
      const urls = fetchMock.mock.calls.map((call) => {
        const input = call[0] as Request | string;
        return typeof input === "string" ? input : input.url;
      });
      expect(urls.some((url) => url.includes(`/api/endpoint/${kpiSlug}/schema/v2/json-schema`))).toBe(true);
    });
    await user.click(screen.getByRole("button", { name: en.apps.generate.submit }));

    await waitFor(async () => {
      const body = await runBody(fetchMock);
      expect(body.endpointNames).toEqual(["ovzdusie-public", "ovzdusie-kpi"]);
      expect(body).not.toHaveProperty("endpointName");
      const needs = body.dataNeeds as { contextSpaceRef: { name: string }; operations: string[] }[];
      expect(needs.map((need) => need.contextSpaceRef.name)).toEqual(["ovzdusie", "ovzdusie-kpi"]);
      expect(needs[1].operations).toEqual(["queryEntity", "retrieveEntity"]);
    });
  });

  it("opens on the endpoints the assistant's Build an app path handed over (T-2696)", async () => {
    const kpis = {
      ...ENDPOINT,
      metadata: { ...ENDPOINT.metadata, name: "ovzdusie-kpi", title: { en: "Air quality indicators" } },
      spec: { ...ENDPOINT.spec, contextSpaceRef: "ovzdusie-kpi", slug: "q3mzkq2v7w5ayxcbn4ltdj6hof" },
    };
    rememberPrefill(`/projects/${PROJECT}/apps/new`, { endpoints: ["ovzdusie-public", "ovzdusie-kpi"] });
    window.history.pushState({}, "", `/projects/${PROJECT}/apps/new`);
    const user = userEvent.setup();
    const fetchMock = renderGenerator({ endpoints: [ENDPOINT, kpis] });
    // The label also names the endpoints added beside it: the choice is the select.
    const [endpoint] = (await screen.findAllByLabelText(en.apps.generate.endpoint, { exact: false })).filter(
      (element) => element.tagName === "SELECT",
    );
    await waitFor(() => expect(endpoint).toHaveValue("ovzdusie-public"));
    await screen.findByRole("group", { name: "AirQualityObserved" });
    await user.type(screen.getByLabelText(en.apps.generate.name, { exact: false }), "ovzdusie-dnes");
    await user.type(screen.getByLabelText(en.apps.generate.prompt, { exact: false }), "A map of today's PM10");
    await user.click(screen.getByRole("button", { name: en.apps.generate.submit }));
    await waitFor(async () => {
      const body = await runBody(fetchMock);
      expect(body.endpointNames).toEqual(["ovzdusie-public", "ovzdusie-kpi"]);
    });
  });

  it("offers Read and update only where the person's own grant holds its writes, and sends them (AP-132, PF-70)", async () => {
    const user = userEvent.setup();
    const fetchMock = renderGenerator({
      grant: {
        ...GRANT,
        permissions: [{ ...GRANT.permissions[0], actions: ["queryEntity", "retrieveEntity", "updateAttrs", "appendAttrs"] }],
      },
    });
    await openGenerator(user);
    await screen.findByLabelText(en.apps.generate.endpoint, { exact: false });
    await fill(user);

    const access = await screen.findByLabelText(en.apps.generate.needs.access, { exact: false });
    await waitFor(() =>
      expect(within(access).getByRole("option", { name: en.apps.generate.needs.presets.update })).toBeEnabled(),
    );
    expect(within(access).getByRole("option", { name: en.apps.generate.needs.presets.full })).toBeDisabled();
    await user.selectOptions(access, "update");
    await user.click(screen.getByRole("button", { name: en.apps.generate.submit }));

    await waitFor(async () => {
      const body = await runBody(fetchMock);
      const needs = body.dataNeeds as { operations: string[] }[];
      // No temporal read: the grant holds none, and the server would refuse it.
      expect(needs[0].operations).toEqual(["queryEntity", "retrieveEntity", "updateAttrs", "appendAttrs"]);
    });
  });

  it("keeps the write to one application role when the person names it (AP-96, T-2666)", async () => {
    const user = userEvent.setup();
    const fetchMock = renderGenerator({
      grant: {
        ...GRANT,
        permissions: [{ ...GRANT.permissions[0], actions: ["queryEntity", "retrieveEntity", "updateAttrs", "appendAttrs"] }],
      },
    });
    await openGenerator(user);
    await screen.findByLabelText(en.apps.generate.endpoint, { exact: false });
    await fill(user);

    const access = await screen.findByLabelText(en.apps.generate.needs.access, { exact: false });
    await waitFor(() =>
      expect(within(access).getByRole("option", { name: en.apps.generate.needs.presets.update })).toBeEnabled(),
    );
    await user.selectOptions(access, "update");
    const role = screen.getByLabelText(en.apps.generate.needs.writeRole, { exact: false });
    await user.type(role, "Steward");
    expect(await screen.findByText(en.apps.generate.needs.writeRoleInvalid)).toBeInTheDocument();
    await user.clear(role);
    await user.type(role, "steward");
    expect(screen.queryByText(en.apps.generate.needs.writeRoleInvalid)).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: en.apps.generate.submit }));

    await waitFor(async () => {
      const body = await runBody(fetchMock);
      const needs = body.dataNeeds as { operations: string[]; roles?: string[] }[];
      expect(needs[0].operations).toEqual(["queryEntity", "retrieveEntity"]);
      expect(needs[0].roles).toBeUndefined();
      expect(needs[1].operations).toEqual(["queryEntity", "retrieveEntity", "updateAttrs", "appendAttrs"]);
      expect(needs[1].roles).toEqual(["steward"]);
    });
  });

  it("offers only Read only where the grant is read-only, and says why", async () => {
    const user = userEvent.setup();
    renderGenerator();
    await openGenerator(user);
    await screen.findByLabelText(en.apps.generate.endpoint, { exact: false });
    await fill(user);
    const access = await screen.findByLabelText(en.apps.generate.needs.access, { exact: false });
    expect(within(access).getByRole("option", { name: en.apps.generate.needs.presets.read })).toBeEnabled();
    expect(within(access).getByRole("option", { name: en.apps.generate.needs.presets.update })).toBeDisabled();
    expect(within(access).getByRole("option", { name: en.apps.generate.needs.presets.full })).toBeDisabled();
    expect(screen.getByText(en.apps.generate.needs.accessHeld)).toBeInTheDocument();
  });

  it("navigates to the app page after creating the run (AG-43, AP-68)", async () => {
    const user = userEvent.setup();
    renderGenerator();
    await openGenerator(user);
    await screen.findByLabelText(en.apps.generate.endpoint, { exact: false });
    await fill(user);

    await user.click(screen.getByRole("button", { name: en.apps.generate.submit }));

    await waitFor(() => {
      expect(window.location.pathname).toBe(`/projects/${PROJECT}/apps/ovzdusie-dnes`);
    });
    // The page names the app in words with its endpoint's title, never by its id.
    expect(await screen.findByRole("heading", { name: "Ovzdusie dnes · Air quality open data" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: en.agentRun.back })).toBeInTheDocument();
    expect(screen.queryByLabelText(en.apps.generate.prompt, { exact: false })).not.toBeInTheDocument();
    // The builder ran in the assistant, which now follows the run it started.
    expect(window.sessionStorage.getItem("jc.assistant.run")).toContain(CREATED_RUN.id);
  });

  it("shows 409 conflict with a link to the live application when app is already being built (AP-68)", async () => {
    const user = userEvent.setup();
    renderGenerator({
      write: {
        status: 409,
        body: {
          type: "https://joinedcontext.com/errors/conflict",
          title: "Conflict",
          status: 409,
          detail: "a run for app 'ovzdusie-dnes' is already live (run id 01J8ZQ4T7K9M2N3P4Q5R6S7T8V)",
        },
      },
    });
    await openGenerator(user);
    await screen.findByLabelText(en.apps.generate.endpoint, { exact: false });
    await fill(user);

    await user.click(screen.getByRole("button", { name: en.apps.generate.submit }));

    const alert = await screen.findByRole("alert");
    expect(alert).toBeInTheDocument();
    expect(alert.textContent).toMatch(/already live|already being built/i);
    const draftOpenLabel = (en.apps as { drafts?: { open?: string } })?.drafts?.open ?? "Open";
    const link =
      within(alert).queryByRole("link") ??
      screen.getByRole("link", {
        name: new RegExp(`ovzdusie-dnes|${draftOpenLabel}|open|instead`, "i"),
      });
    expect(link).toHaveAttribute("href", `/projects/${PROJECT}/apps/ovzdusie-dnes`);
  });

  it("says a generated app is reachable only to a signed-in user (ADR-N-019)", async () => {
    const user = userEvent.setup();
    renderGenerator();
    await openGenerator(user);
    await screen.findByLabelText(en.apps.generate.endpoint, { exact: false });

    // The endpoint is public, and the app built on it still is not.
    await user.selectOptions(screen.getByLabelText(en.apps.generate.endpoint, { exact: false }), "ovzdusie-public");

    expect(await screen.findByText(en.apps.generate.needs.loginOnly)).toBeInTheDocument();
  });

  it("says in words what the endpoint lets this person read (AP-51)", async () => {
    const user = userEvent.setup();
    renderGenerator();
    await openGenerator(user);
    await screen.findByLabelText(en.apps.generate.endpoint, { exact: false });

    await user.selectOptions(screen.getByLabelText(en.apps.generate.endpoint, { exact: false }), "ovzdusie-public");

    const preview = await screen.findByRole("region", { name: en.apps.generate.preview.title });
    // One line: the types, the attributes and, for a read grant, nothing that promises a write.
    expect(
      await within(preview).findByText(
        [
          en.apps.generate.preview.readsTypes.replace("{types}", "AirQualityObserved"),
          "location, name, pm10",
          en.apps.generate.preview.readOnly,
        ].join(" · "),
        { exact: false },
      ),
    ).toBeInTheDocument();
    // A prohibition is the one thing a person cannot infer from the checklist above.
    expect(
      within(preview).getByText(
        en.apps.generate.preview.denied.replace("{attrs}", "internalNote"),
        { exact: false },
      ),
    ).toBeInTheDocument();
  });

  it("shows five entities as they are served, so the data can be judged before the build", async () => {
    const user = userEvent.setup();
    renderGenerator();
    await openGenerator(user);
    await screen.findByLabelText(en.apps.generate.endpoint, { exact: false });

    await user.selectOptions(screen.getByLabelText(en.apps.generate.endpoint, { exact: false }), "ovzdusie-public");

    // The samples fold away under one summary that counts them.
    expect(
      await screen.findByText(
        en.apps.generate.preview.samplesCount.replace("{count}", String(ENTITIES.length)),
      ),
    ).toBeInTheDocument();
    const table = screen.getByRole("table", { name: en.apps.generate.preview.samples });
    expect(within(table).getAllByRole("row")).toHaveLength(ENTITIES.length + 1);
    // The local id in the cell, the whole URN in its tooltip.
    const first = within(table).getByText("st-1");
    expect(first).toHaveAttribute("title", ENTITIES[0].id);
    expect(within(table).getByText("name=Station 1, pm10=13")).toBeInTheDocument();
  });

  it("still builds when the endpoint serves no samples and no grant", async () => {
    const user = userEvent.setup();
    renderGenerator({ grantStatus: 403, entities: { body: [], status: 200 } });
    await openGenerator(user);
    await screen.findByLabelText(en.apps.generate.endpoint, { exact: false });
    await fill(user);

    const preview = screen.getByRole("region", { name: en.apps.generate.preview.title });
    expect(
      within(preview).getByText(en.apps.generate.preview.accessUnavailable),
    ).toBeInTheDocument();
    expect(within(preview).getByText(en.apps.generate.preview.samplesEmpty)).toBeInTheDocument();
    // The model is what bounds the app, so a silent `/access` is not a reason to stop.
    expect(screen.getByRole("button", { name: en.apps.generate.submit })).toBeEnabled();
  });

  it("publishing what was built opens a merge request like any other change (AP-55)", async () => {
    const user = userEvent.setup();
    renderGenerator({ runStatus: "previewing" });
    await openGenerator(user);
    await screen.findByLabelText(en.apps.generate.endpoint, { exact: false });
    await fill(user);
    await user.click(screen.getByRole("button", { name: en.apps.generate.submit }));

    await user.click(await screen.findByRole("button", { name: en.agentRun.publish }));

    expect(await screen.findByText("chg-0000c3d4")).toBeInTheDocument();
    expect(screen.getByText(en.changes.accepted)).toBeInTheDocument();
  });

  it("names every rule the parameters broke instead of one", async () => {
    const user = userEvent.setup();
    renderGenerator({
      write: {
        status: 400,
        body: {
          type: "https://joinedcontext.com/errors/validation",
          title: "Bad Request",
          status: 400,
          detail: "the parameters do not satisfy the blueprint",
          errors: ["name: must be a DNS-1123 label", "prompt: must not be empty"],
        },
      },
    });
    await openGenerator(user);
    await screen.findByLabelText(en.apps.generate.endpoint, { exact: false });
    await fill(user);

    await user.click(screen.getByRole("button", { name: en.apps.generate.submit }));

    expect(await screen.findByText("name: must be a DNS-1123 label")).toBeInTheDocument();
    expect(screen.getByText("prompt: must not be empty")).toBeInTheDocument();
  });

  it("cannot be submitted before an endpoint bounds it", async () => {
    const user = userEvent.setup();
    renderGenerator();
    await openGenerator(user);
    await screen.findByLabelText(en.apps.generate.name, { exact: false });

    await user.type(screen.getByLabelText(en.apps.generate.name, { exact: false }), "ovzdusie-dnes");
    await user.type(screen.getByLabelText(en.apps.generate.prompt, { exact: false }), "Anything at all");

    expectDenied(screen.getByRole("button", { name: en.apps.generate.submit }), en.apps.generate.missing.endpoint);
  });

  it("says the endpoint publishes no model rather than generating against nothing", async () => {
    const user = userEvent.setup();
    renderGenerator({ schemaStatus: 406 });
    await openGenerator(user);
    await screen.findByLabelText(en.apps.generate.endpoint, { exact: false });

    await user.selectOptions(screen.getByLabelText(en.apps.generate.endpoint, { exact: false }), "ovzdusie-public");

    expect(await screen.findByText(en.apps.generate.needs.unavailable)).toBeInTheDocument();
    await user.type(screen.getByLabelText(en.apps.generate.prompt, { exact: false }), "Anything at all");
    expectDenied(screen.getByRole("button", { name: en.apps.generate.submit }), en.apps.generate.missing.needs);
  });

  it("explains itself and links the example apps when there is no builder (AG-26)", async () => {
    const user = userEvent.setup();
    const fetchMock = renderGenerator({ blueprints: [] });
    await openGenerator(user);

    expect(await screen.findByText(en.apps.generate.noBuilder)).toBeInTheDocument();
    expect(
      screen.getByRole("link", { name: en.apps.generate.examples["hsl-transport"] }),
    ).toHaveAttribute("href", "/apps/hsl-transport/");
    expect(
      screen.getByRole("link", { name: en.apps.generate.examples["air-quality"] }),
    ).toHaveAttribute("href", "/apps/air-quality/");
    // No form means no way to open a merge request nothing would pick up.
    expect(screen.queryByLabelText(en.apps.generate.prompt, { exact: false })).not.toBeInTheDocument();
    expect(
      fetchMock.mock.calls.map((call) => call[0] as Request).some((r) => r.method === "POST"),
    ).toBe(false);
  });
});

describe("the derived grant", () => {
  const endpoint = ENDPOINT as never;
  const types = [{ name: "AirQualityObserved", attributes: ["location", "pm10", "pm25"] }];

  it("carries the endpoint's space and representations, and reads only", () => {
    const [need] = dataNeeds(endpoint, types, []);
    expect(need.contextSpaceRef).toEqual({ kind: "ContextSpace", name: "ovzdusie" });
    expect(need.representations).toEqual(["ngsi-ld", "geojson"]);
    expect(need.operations).toEqual(["queryEntity", "retrieveEntity"]);
  });

  it("reads for everyone and writes for the named role only, as two needs", () => {
    const [read, write] = dataNeeds(endpoint, types, [], ACCESS_PRESETS.update, "steward");
    expect(read.operations).toEqual(["queryEntity", "retrieveEntity", "queryTemporal", "retrieveTemporal"]);
    expect(read).not.toHaveProperty("roles");
    expect(write.operations).toEqual(ACCESS_PRESETS.update);
    expect(write.roles).toEqual(["steward"]);
    expect(dataNeeds(endpoint, types, [], ACCESS_PRESETS.update)).toHaveLength(1);
    expect(dataNeeds(endpoint, types, [], ACCESS_PRESETS.read, "steward")).toHaveLength(1);
  });

  it("drops a type whose every attribute the user unticked", () => {
    expect(
      dataNeeds(endpoint, types, [
        "AirQualityObserved.location",
        "AirQualityObserved.pm10",
        "AirQualityObserved.pm25",
      ]),
    ).toEqual([]);
  });

  it("can only narrow: an attribute the endpoint never published cannot appear", () => {
    const [need] = dataNeeds(endpoint, types, ["AirQualityObserved.pm25"]);
    expect(need.attrs).toEqual(["location", "pm10"]);
    expect(need.attrs).not.toContain("internalNote");
  });
});

describe("the access presets (AP-132)", () => {
  const grant = (actions: string[], prohibited: string[] = [], type = "Alert") => ({
    permissions: [{ resource: { type }, actions }],
    prohibitions: [{ resource: { type }, actions: prohibited }],
  });

  it("each preset adds to the one before it, and only Read only writes nothing", () => {
    expect(ACCESS_PRESETS.read).toEqual(["queryEntity", "retrieveEntity", "queryTemporal", "retrieveTemporal"]);
    expect(ACCESS_PRESETS.update).toEqual([...ACCESS_PRESETS.read, "updateAttrs", "appendAttrs"]);
    expect(ACCESS_PRESETS.full).toEqual([...ACCESS_PRESETS.update, "createEntity", "deleteEntity"]);
  });

  it("is offered only when every write it adds is held on every type, and a prohibition takes one away", () => {
    const editor = grant(["queryEntity", "updateAttrs", "appendAttrs"]);
    expect(offersPreset("read", editor, ["Alert"])).toBe(true);
    expect(offersPreset("update", editor, ["Alert"])).toBe(true);
    expect(offersPreset("full", editor, ["Alert"])).toBe(false);
    expect(offersPreset("update", editor, ["Alert", "Road"]), "no grant on Road").toBe(false);
    expect(offersPreset("update", grant(["updateAttrs", "appendAttrs"], ["appendAttrs"]), ["Alert"])).toBe(false);
    expect(offersPreset("full", grant(ACCESS_PRESETS.full.slice(), [], "*"), ["Alert", "Road"])).toBe(true);
    expect(offersPreset("update", undefined, ["Alert"]), "no document yet").toBe(false);
    expect(offersPreset("read", editor, []), "nothing to read").toBe(false);
  });

  it("never carries more than the preset, and a temporal read only where it is held", () => {
    const reader = grant(["queryEntity", "retrieveEntity", "queryTemporal"]);
    expect(presetOperations("read", reader, ["Alert"])).toEqual(["queryEntity", "retrieveEntity", "queryTemporal"]);
    expect(presetOperations("read", undefined, ["Alert"])).toEqual(["queryEntity", "retrieveEntity"]);
    const full = grant(ACCESS_PRESETS.full.slice());
    expect(presetOperations("update", full, ["Alert"])).toEqual(ACCESS_PRESETS.update);
    expect(presetOperations("update", full, ["Alert"])).not.toContain("createEntity");
  });
});
