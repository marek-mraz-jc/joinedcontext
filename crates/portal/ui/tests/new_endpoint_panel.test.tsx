/**
 * "New endpoint" of Build an app (T-2703, AP-132, API/04 §8): a space, a preset and, for a write,
 * the types it changes; the Portal renders the manifests, the panel proposes them at once as a
 * Change of its own, and hands the endpoint to the builder only once the project lists it.
 *
 * What is asserted is what a person cannot see for themselves: a read proposes the Endpoint alone,
 * a write proposes its Policy in the same Change, nothing is sent while the form is incomplete, and
 * the builder is not handed an endpoint that is not served yet.
 */
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  createMemoryHistory,
  createRootRoute,
  createRoute,
  createRouter,
  RouterProvider,
} from "@tanstack/react-router";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import {
  NewEndpointPanel,
  newEndpointMissing,
  newEndpointRequest,
} from "../src/pages/apps/NewEndpointPanel";

const PROJECT = "helsinki";
const API = "joinedcontext.com/v1alpha1";

const SPACE = { apiVersion: API, kind: "ContextSpace", metadata: { name: "mobility", namespace: PROJECT }, spec: {} };
const MODELS = [
  { name: "mobility", project: PROJECT, space: "mobility", version: "0.3.0", lifecycle: "draft", classes: ["BikeHireDockingStation"] },
];

function endpointManifest(name: string) {
  return {
    apiVersion: API,
    kind: "Endpoint",
    metadata: { name, namespace: PROJECT },
    spec: { contextSpaceRef: "mobility", slug: "k7m2qz4tv6xh3n5jb2ryd3wcfa", audience: "project-list", allowedProjects: [PROJECT] },
  };
}

const POLICY = {
  apiVersion: API,
  kind: "Policy",
  metadata: { name: `bikes-app-${PROJECT}`, namespace: PROJECT },
  spec: { operations: ["queryEntity", "retrieveEntity", "updateAttrs", "appendAttrs"] },
};

const CHANGE = {
  apiVersion: API,
  kind: "Change",
  metadata: { name: "chg-0000c3d4", namespace: PROJECT },
  status: { lane: "yellow", phase: "PendingApproval", plan: { create: 1 } },
};

interface Sent {
  method: string;
  path: string;
  body: unknown;
}

let sent: Sent[] = [];
/** What the project's endpoint list holds: the new one appears once its Change is merged. */
let listed: unknown[] = [];
/** The phase the followed Change answers with. */
let phaseNow = "PendingApproval";

function stubFetch(write: boolean) {
  sent = [];
  listed = [];
  phaseNow = "PendingApproval";
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const request = input as Request;
      const url = new URL(request.url, window.location.origin);
      const body = request.method === "GET" ? undefined : await request.clone().json().catch(() => undefined);
      sent.push({ method: request.method, path: url.pathname + url.search, body });
      const list = (items: unknown[]) => Response.json({ apiVersion: API, kind: "List", items });
      if (url.pathname === `/api/v1/projects/${PROJECT}/spaces`) {
        return list([SPACE]);
      }
      if (url.pathname === "/api/v1/organization/datamodels") {
        return Response.json({ apiVersion: API, kind: "List", items: MODELS, smartDataModels: [] });
      }
      if (url.pathname.endsWith("/datamodels/mobility/source")) {
        return new Response("classes: {}\n", { headers: { "Content-Type": "text/yaml" } });
      }
      if (url.pathname.endsWith("/assistant/propose-endpoint")) {
        return Response.json({
          lane: write ? "red" : "yellow",
          slug: "k7m2qz4tv6xh3n5jb2ryd3wcfa",
          endpoint: endpointManifest("bikes-app"),
          policies: write ? [POLICY] : [],
          prefill: {},
        });
      }
      if (url.pathname === `/api/v1/projects/${PROJECT}/changes/${CHANGE.metadata.name}`) {
        return Response.json({ ...CHANGE, status: { ...CHANGE.status, phase: phaseNow } });
      }
      if (url.pathname === `/api/v1/projects/${PROJECT}/endpoints` && request.method === "GET") {
        return list(listed);
      }
      if (url.searchParams.get("dryRun") === "All") {
        return Response.json({ valid: true, verdict: { ok: true } });
      }
      return Response.json(CHANGE, { status: 202 });
    }),
  );
}

function renderPanel(onLive = vi.fn()) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  // The Change notice links to the change, so the panel renders inside a router.
  const root = createRootRoute();
  const page = createRoute({
    getParentRoute: () => root,
    path: "$",
    component: () => <NewEndpointPanel project={PROJECT} onLive={onLive} onCancel={() => {}} />,
  });
  const router = createRouter({
    routeTree: root.addChildren([page]),
    history: createMemoryHistory({ initialEntries: [`/projects/${PROJECT}/apps/new`] }),
  });
  render(
    <I18nextProvider i18n={i18n}>
      <QueryClientProvider client={client}>
        <RouterProvider router={router} />
      </QueryClientProvider>
    </I18nextProvider>,
  );
  return onLive;
}

const t = en.apps.generate.newEndpoint;

async function fillReadOnly(user: ReturnType<typeof userEvent.setup>) {
  await user.type(screen.getByRole("textbox", { name: new RegExp(t.name) }), "bikes-app");
  await user.click(screen.getByRole("combobox", { name: t.space }));
  await user.click(await screen.findByRole("option", { name: /mobility/ }));
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});
afterEach(() => {
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

describe("the request and what stops it", () => {
  it("asks for this project's own endpoint, and the types only for a write", () => {
    expect(newEndpointRequest("bikes-app", "mobility", "read", ["Road"])).toEqual({
      contextSpace: "mobility",
      name: "bikes-app",
      access: "read",
    });
    expect(newEndpointRequest("bikes-app", "mobility", "full", ["Road"])).toEqual({
      contextSpace: "mobility",
      name: "bikes-app",
      access: "full",
      entityTypes: ["Road"],
    });
  });

  it("names every missing field at once", () => {
    expect(newEndpointMissing("", "", "update", [])).toEqual(["name", "space", "types"]);
    expect(newEndpointMissing("Bikes App", "mobility", "read", [])).toEqual(["name"]);
    expect(newEndpointMissing("-bikes", "mobility", "read", [])).toEqual(["name"]);
    expect(newEndpointMissing("a".repeat(64), "mobility", "read", [])).toEqual(["name"]);
    expect(newEndpointMissing("bikes-app", "mobility", "read", [])).toEqual([]);
    expect(newEndpointMissing("bikes-app", "mobility", "update", ["Road"])).toEqual([]);
  });
});

describe("New endpoint", () => {
  it("proposes a read-only endpoint alone and hands it over only once it is served", async () => {
    stubFetch(false);
    const user = userEvent.setup();
    const onLive = renderPanel();
    // Three presets, read first, and no types asked for a read.
    const presets = await screen.findAllByRole("radio");
    expect(presets.map((radio) => radio.closest("label")?.textContent)).toEqual([
      en.apps.generate.needs.presets.read,
      en.apps.generate.needs.presets.update,
      en.apps.generate.needs.presets.full,
    ]);
    expect(presets[0]).toBeChecked();
    expect(screen.queryByRole("combobox", { name: t.types })).toBeNull();

    await fillReadOnly(user);
    await user.click(screen.getByRole("button", { name: t.propose }));

    await screen.findByText(new RegExp(`bikes-app is proposed \\(yellow lane\\)`));
    const rendered = sent.find((request) => request.path.endsWith("/assistant/propose-endpoint"));
    expect(rendered?.body).toEqual({ contextSpace: "mobility", name: "bikes-app", access: "read" });
    // The Endpoint alone, checked then proposed on the endpoints route; no import, no Policy.
    const writes = sent.filter((request) => request.method === "POST" && !request.path.includes("propose-endpoint"));
    expect(writes.map((request) => request.path)).toEqual([
      `/api/v1/projects/${PROJECT}/endpoints?dryRun=All`,
      `/api/v1/projects/${PROJECT}/endpoints`,
    ]);
    expect((writes[1].body as { kind: string }).kind).toBe("Endpoint");
    expect(onLive).not.toHaveBeenCalled();

    // Merged and served: the list holds it, and the builder is handed it with the preset.
    listed = [endpointManifest("bikes-app")];
    await waitFor(() => expect(onLive).toHaveBeenCalledWith("bikes-app", "read"), { timeout: 8000 });
    expect(onLive).toHaveBeenCalledTimes(1);
  }, 15000);

  // F-R2UI2-014: the wait polled the endpoint list for ever after a rejection.
  it("stops waiting for the endpoint once its Change is rejected", async () => {
    stubFetch(false);
    const user = userEvent.setup();
    renderPanel();
    await screen.findAllByRole("radio");
    await fillReadOnly(user);
    phaseNow = "Rejected";
    await user.click(screen.getByRole("button", { name: t.propose }));
    expect(await screen.findByText(en.agentRun.follow.rejected)).toBeInTheDocument();
    expect(screen.queryByText(/bikes-app is proposed/)).toBeNull();
    const listReads = () => sent.filter((request) => request.method === "GET" && request.path === `/api/v1/projects/${PROJECT}/endpoints`).length;
    const before = listReads();
    await new Promise((resolve) => setTimeout(resolve, 6000));
    expect(listReads()).toBe(before);
  }, 15000);

  it("proposes a write preset's Policy in the same Change, and sends nothing without a type", async () => {
    stubFetch(true);
    const user = userEvent.setup();
    renderPanel();
    await screen.findAllByRole("radio");
    await fillReadOnly(user);
    await user.click(screen.getByRole("radio", { name: en.apps.generate.needs.presets.update }));

    await user.click(screen.getByRole("button", { name: t.propose }));
    expect(await screen.findByText(t.typesMissing)).toBeInTheDocument();
    expect(sent.some((request) => request.method === "POST")).toBe(false);

    await user.click(screen.getByRole("combobox", { name: t.types }));
    await user.click(await screen.findByRole("option", { name: /BikeHireDockingStation/ }));
    await user.click(screen.getByRole("button", { name: t.propose }));

    await screen.findByText(/bikes-app is proposed \(red lane\)/);
    const rendered = sent.find((request) => request.path.endsWith("/assistant/propose-endpoint"));
    expect(rendered?.body).toEqual({
      contextSpace: "mobility",
      name: "bikes-app",
      access: "update",
      entityTypes: ["BikeHireDockingStation"],
    });
    const imported = sent.filter((request) => request.path.startsWith(`/api/v1/projects/${PROJECT}/import`));
    expect(imported.map((request) => request.path)).toEqual([
      `/api/v1/projects/${PROJECT}/import?dryRun=All`,
      `/api/v1/projects/${PROJECT}/import`,
    ]);
    const kinds = (imported[1].body as { manifests: { kind: string }[] }).manifests.map((m) => m.kind);
    expect(kinds).toEqual(["Policy", "Endpoint"]);
  });

  it("says why the Portal refused the rendering and proposes nothing", async () => {
    stubFetch(false);
    const user = userEvent.setup();
    renderPanel();
    await screen.findAllByRole("radio");
    await fillReadOnly(user);
    sent = [];
    const original = vi.mocked(fetch).getMockImplementation();
    vi.mocked(fetch).mockImplementation(async (input) => {
      const request = input as Request;
      if (request.url.endsWith("/assistant/propose-endpoint")) {
        return new Response(
          JSON.stringify({ title: "Bad Request", status: 400, detail: "a write preset needs entityTypes" }),
          { status: 400, headers: { "Content-Type": "application/problem+json" } },
        );
      }
      return original!(input);
    });
    await user.click(screen.getByRole("button", { name: t.propose }));
    expect(await screen.findByText("a write preset needs entityTypes")).toBeInTheDocument();
    expect(sent.filter((request) => request.method === "POST")).toEqual([]);
  });
});
