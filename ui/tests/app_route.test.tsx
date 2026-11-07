/**
 * Application route tests: /projects/$project/apps/$name (AP-68, AP-69, T-0559).
 *
 * Asserts that:
 * 1. The route renders the application by name with its active run.
 * 2. A live run shows its conversation replayed from the store before the stream attaches.
 * 3. A reload (fresh mount of the same URL) shows the same run rather than the form.
 * 4. No run -> the generator form prefilled with the application name.
 */
import { act, render, screen, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  createRootRoute,
  createRoute,
  createRouter,
  Outlet,
  RouterProvider,
} from "@tanstack/react-router";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { AppPage } from "../src/pages/apps/AppPage";
import { AssistantDock } from "../src/assistant/AssistantDock";

const PROJECT = "banskabystrica";
const RUN_ID = "01J8ZQ4T7K9M2N3P4Q5R6S7T8V";

/** The run's name read as words, never its id; the endpoint's title follows when it is known. */
const APP_TITLE = "Ovzdusie dnes · Air quality open data";

const RUN = {
  id: RUN_ID,
  project: PROJECT,
  appName: "ovzdusie-dnes",
  endpointName: "ovzdusie-public",
  appClass: "fullstack",
  visibility: "project",
  prompt: "A map of the stations with today's PM10",
  status: "building",
  steps: 12,
  tokensUsed: 48_210,
  createdBy: "jana.kovacova",
  createdAt: "2026-09-12T08:00:00Z",
};

const BLUEPRINT_CARD = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Blueprint",
  metadata: { name: "app-from-prompt", namespace: "org", title: { en: "App from a prompt" } },
  spec: { version: "1.4.0", riskClass: "yellow" },
};

const ENDPOINT = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Endpoint",
  metadata: { name: "ovzdusie-public", namespace: PROJECT, title: { en: "Air quality open data" } },
  spec: {
    contextSpaceRef: "ovzdusie",
    slug: "k7m2qz4tv6xh3n5jb2ryd3wcfa",
    audience: "public",
    enabledRepresentations: ["ngsi-ld", "geojson"],
  },
};

class StubEventSource {
  static opened: StubEventSource[] = [];
  readonly url: string;
  readonly withCredentials: boolean;
  closed = false;
  private readonly listeners = new Map<string, Set<EventListener>>();

  constructor(url: string, init?: EventSourceInit) {
    this.url = url;
    this.withCredentials = init?.withCredentials ?? false;
    StubEventSource.opened.push(this);
  }

  addEventListener(kind: string, listener: EventListener): void {
    const set = this.listeners.get(kind) ?? new Set<EventListener>();
    set.add(listener);
    this.listeners.set(kind, set);
  }

  removeEventListener(kind: string, listener: EventListener): void {
    this.listeners.get(kind)?.delete(listener);
  }

  close(): void {
    this.closed = true;
  }

  emit(kind: string, payload: unknown, data?: string): void {
    const frame = new MessageEvent(kind, {
      data: data ?? JSON.stringify(payload),
      lastEventId: String((payload as { seq?: number } | undefined)?.seq ?? ""),
    });
    for (const listener of [...(this.listeners.get(kind) ?? [])]) {
      listener(frame);
    }
  }
}

async function emit(kind: string, payload: unknown, data?: string) {
  await act(async () => {
    for (const source of StubEventSource.opened) {
      source.emit(kind, payload, data);
    }
  });
}

function renderAppRoute(name: string, options: { runs?: (typeof RUN)[]; runDetail?: typeof RUN } = {}) {
  const { runs = [RUN], runDetail = RUN } = options;

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
      return json({ subject: "b7c1e0f4", username: "jana.kovacova", roles: ["domain-editor"] });
    }
    if (url.pathname.endsWith("/blueprints")) {
      return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [BLUEPRINT_CARD] });
    }
    if (url.pathname.endsWith("/endpoints")) {
      return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [ENDPOINT] });
    }
    if (url.pathname.endsWith("/access")) {
      return json({ permissions: [] });
    }
    if (url.pathname.includes("/ngsi-ld/v1/entities")) {
      return json([]);
    }
    if (url.pathname.includes("/schema/index.json")) {
      return json({ models: [{ name: "bb-air-quality", version: 1 }] });
    }
    if (url.pathname.includes("/schema/v1/json-schema")) {
      return json({ $defs: { AirQualityObserved: { properties: { pm10: {} } } } });
    }
    if (url.pathname.endsWith("/agent-runs") && request.method === "GET") {
      const appQuery = url.searchParams.get("app");
      const matched = appQuery ? runs.filter((r) => r.appName === appQuery) : runs;
      return json({ items: matched });
    }
    if (url.pathname.includes(`/agent-runs/${runDetail.id}`) && request.method === "GET") {
      return json(runDetail);
    }
    return json({ items: [] });
  });

  vi.stubGlobal("fetch", fetchMock);
  vi.stubGlobal("EventSource", StubEventSource as unknown as typeof EventSource);

  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });

  const rootRoute = createRootRoute({
    component: Outlet,
  });

  const appRoute = createRoute({
    getParentRoute: () => rootRoute,
    path: "/projects/$project/apps/$name",
    component: function AppRouteComponent() {
      const { project, name: appName } = appRoute.useParams();
      return (
        <>
          <AppPage project={project} name={appName} />
          <AssistantDock project={project} />
        </>
      );
    },
  });

  const routeTree = rootRoute.addChildren([appRoute]);
  const router = createRouter({ routeTree });

  window.history.pushState({}, "", `/projects/${PROJECT}/apps/${name}`);

  const view = render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <RouterProvider router={router} />
      </I18nextProvider>
    </QueryClientProvider>,
  );

  return { view, fetchMock };
}

describe("AppPage route /projects/$project/apps/$name", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    StubEventSource.opened = [];
    window.sessionStorage.clear();
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("renders the application by name with its active run (AP-69)", async () => {
    renderAppRoute("ovzdusie-dnes");
    expect(await screen.findByRole("heading", { name: APP_TITLE })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: en.agentRun.back })).toBeInTheDocument();
    expect(screen.queryByLabelText(en.apps.generate.prompt, { exact: false })).not.toBeInTheDocument();
  });

  it("shows a live run with conversation events on stream attachment (AP-69)", async () => {
    renderAppRoute("ovzdusie-dnes");
    await screen.findByRole("heading", { name: APP_TITLE });

    await emit("thought", { seq: 1, text: "Reading the projected schema from store" });
    // The building panel repeats the latest thought, so the line is looked for in the conversation.
    const conversation = await screen.findByRole("region", { name: en.agentRun.conversation.title });
    expect(await within(conversation).findByText("Reading the projected schema from store")).toBeInTheDocument();
  });

  it("shows the same run rather than the form on a reload (fresh mount of the same URL) (AP-69)", async () => {
    const { view } = renderAppRoute("ovzdusie-dnes");
    await screen.findByRole("heading", { name: APP_TITLE });
    expect(screen.queryByLabelText(en.apps.generate.prompt, { exact: false })).not.toBeInTheDocument();

    view.unmount();

    // Fresh mount with the same URL simulating a reload
    renderAppRoute("ovzdusie-dnes");
    await screen.findByRole("heading", { name: APP_TITLE });
    expect(screen.queryByLabelText(en.apps.generate.prompt, { exact: false })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: en.agentRun.back })).toBeInTheDocument();
  });

  it("shows the generator prefilled with the app name when no run exists (AP-68)", async () => {
    renderAppRoute("nova-aplikacia", { runs: [] });
    await screen.findByRole("heading", { name: en.apps.generate.title });

    expect(screen.getByLabelText(en.apps.generate.name, { exact: false })).toHaveValue("nova-aplikacia");
    expect(screen.getByLabelText(en.apps.generate.prompt, { exact: false })).toBeInTheDocument();
  });
});
