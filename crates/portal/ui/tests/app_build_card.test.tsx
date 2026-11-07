/**
 * The Build card of "Build an app" in the conversation (T-2721, API/04 §Paths): what the chat
 * answered, and Build, which starts the run from the browser with the person's own session and
 * the builder's data needs.
 */
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createRootRoute, createRoute, createRouter, Outlet, RouterProvider } from "@tanstack/react-router";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { AppBuildCard, appBuildOf } from "../src/pages/apps/AppBuildCard";
import { slugOf } from "../src/pages/apps/AppGenerator";
import type { AppBuild } from "../src/pages/apps/AppBuildCard";
import { expectNoAxeViolations } from "./page_contract";

const PROJECT = "helsinki";
const SLUG = "k7m2qz4tv6xh3n5jb2ryd3wcfa";

const endpoint = (name: string, slug: string) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Endpoint",
  metadata: { name, namespace: PROJECT },
  spec: { contextSpaceRef: "traffic", slug, audience: "project-list", enabledRepresentations: ["ngsi-ld"] },
});

const SCHEMA = {
  $schema: "http://json-schema.org/draft-07/schema#",
  $defs: { Alert: { properties: { title: {}, severity: {}, location: {} } } },
};

const READS = { resource: { type: "Alert" }, actions: ["queryEntity", "retrieveEntity"], attributes: "*" };
const WRITES = { resource: { type: "Alert" }, actions: ["updateAttrs", "appendAttrs"], attributes: "*" };

interface Stub {
  grant?: unknown[];
  run?: { status: number; body: unknown };
  schemaStatus?: number;
}

function renderCard(build: AppBuild, stub: Stub = {}) {
  const { grant = [READS, WRITES], run = { status: 202, body: { id: "01J8ZQ4T7K9M2N3P4Q5R6S7T8V", appName: "alerts-desk" } }, schemaStatus = 200 } = stub;
  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    const request = input as Request;
    const url = new URL(request.url, "http://localhost");
    const json = (body: unknown, status = 200) =>
      Promise.resolve(new Response(JSON.stringify(body), { status, headers: { "Content-Type": status >= 400 ? "application/problem+json" : "application/json" } }));
    if (url.pathname.endsWith("/endpoints")) {
      return json({ kind: "List", items: [endpoint("alerts", SLUG), endpoint("roads", "r0adsr0adsr0adsr0adsr0adsr")] });
    }
    if (url.pathname.endsWith("/access")) return json({ permissions: grant, prohibitions: [] });
    if (url.pathname.endsWith("/schema/index.json")) return json({ models: [{ version: 1 }] }, schemaStatus);
    if (url.pathname.endsWith("/schema/v1/json-schema")) return json(SCHEMA, schemaStatus);
    if (url.pathname.endsWith("/agent-runs") && request.method === "POST") return json(run.body, run.status);
    return json({ kind: "List", items: [] });
  });
  vi.stubGlobal("fetch", fetchMock);
  const onStarted = vi.fn();
  const root = createRootRoute({ component: Outlet });
  const chat = createRoute({
    getParentRoute: () => root,
    path: "/",
    component: () => <AppBuildCard project={PROJECT} build={build} onStarted={onStarted} />,
  });
  const app = createRoute({ getParentRoute: () => root, path: "/projects/$project/$plural/$name", component: () => <p>the app page</p> });
  const router = createRouter({ routeTree: root.addChildren([chat, app]) });
  window.history.pushState({}, "", "/");
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>
        <RouterProvider router={router} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return { fetchMock, onStarted };
}

async function postedRun(fetchMock: ReturnType<typeof vi.fn>): Promise<Record<string, unknown>> {
  const request = fetchMock.mock.calls.map((call) => call[0] as Request).find((one) => one.method === "POST");
  expect(request).toBeDefined();
  return (await request!.json()) as Record<string, unknown>;
}

const BUILD: AppBuild = {
  endpoints: ["alerts"],
  access: "update",
  visibility: "organization",
  prompt: "A desk where the traffic office sees today's alerts",
};
/** The name the builder derives from the words, which the card starts the run as. */
const NAME = slugOf(BUILD.prompt, "alerts");

describe("AppBuildCard", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("Build starts the run with the builder's needs, the audience and the words, then follows it", async () => {
    const user = userEvent.setup();
    const { fetchMock, onStarted } = renderCard(BUILD);
    expect(NAME).toBe("desk-where-traffic");
    expect(await screen.findByRole("heading", { name: `Build ${NAME}` })).toBeInTheDocument();
    expect(screen.getByText(en.apps.buildCard.audience.organization)).toBeInTheDocument();
    const build = await screen.findByRole("button", { name: en.apps.buildCard.build });
    await waitFor(() => expect(build).toBeEnabled());
    expect(screen.getByText(en.apps.buildCard.access.update)).toBeInTheDocument();
    await user.click(build);

    const body = await postedRun(fetchMock);
    expect(body).toMatchObject({
      appName: NAME,
      appClass: "ui",
      endpointName: "alerts",
      prompt: BUILD.prompt,
      visibility: "organization",
    });
    const needs = body.dataNeeds as { operations: string[]; types: string[] }[];
    expect(needs).toHaveLength(1);
    expect(needs[0].types).toEqual(["Alert"]);
    expect(needs[0].operations).toEqual(expect.arrayContaining(["queryEntity", "retrieveEntity", "updateAttrs", "appendAttrs"]));
    expect(needs[0].operations).not.toContain("deleteEntity");
    await waitFor(() => expect(onStarted).toHaveBeenCalledWith("01J8ZQ4T7K9M2N3P4Q5R6S7T8V"));
    expect(await screen.findByText("the app page")).toBeInTheDocument();
    expect(window.location.pathname).toBe("/projects/helsinki/apps/alerts-desk");
  });

  it("a write the person's grant does not carry falls back to reading, and the card says so", async () => {
    const user = userEvent.setup();
    const { fetchMock } = renderCard({ ...BUILD, access: "full" }, { grant: [READS] });
    expect(await screen.findByText(en.apps.buildCard.readOnly.replace("{name}", "alerts"))).toBeInTheDocument();
    expect(screen.getByText(en.apps.buildCard.access.read)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: en.apps.buildCard.build }));
    const needs = (await postedRun(fetchMock)).dataNeeds as { operations: string[] }[];
    expect(needs[0].operations.filter((operation) => !/query|retrieve/.test(operation))).toEqual([]);
  });

  it("several endpoints: the first is the app's, the rest are read beside it", async () => {
    const user = userEvent.setup();
    const { fetchMock } = renderCard({ ...BUILD, access: "read", endpoints: ["alerts", "roads"] });
    const build = await screen.findByRole("button", { name: en.apps.buildCard.build });
    await waitFor(() => expect(build).toBeEnabled());
    await user.click(build);
    expect(await postedRun(fetchMock)).toMatchObject({ endpointNames: ["alerts", "roads"] });
  });

  it("an endpoint the project does not have, or one whose model cannot be read, offers no Build", async () => {
    renderCard({ ...BUILD, endpoints: ["gone"] });
    expect(await screen.findByRole("alert")).toHaveTextContent(en.apps.buildCard.noEndpoint.replace("{name}", "gone"));
    expect(screen.getByRole("button", { name: new RegExp(`^${en.apps.buildCard.build}`) })).toHaveAttribute("aria-disabled", "true");
  });

  it("a model the endpoint does not serve says so, with no Build", async () => {
    renderCard(BUILD, { schemaStatus: 503 });
    expect(await screen.findByRole("alert")).toHaveTextContent(en.apps.buildCard.unavailable.replace("{name}", "alerts"));
    expect(screen.getByRole("button", { name: new RegExp(`^${en.apps.buildCard.build}`) })).toHaveAttribute("aria-disabled", "true");
  });

  it("a live run of the same name is said in words a person can act on", async () => {
    const user = userEvent.setup();
    renderCard(BUILD, { run: { status: 409, body: { title: "Conflict", status: 409, detail: `application '${NAME}' already has a live run: 01J` } } });
    const build = await screen.findByRole("button", { name: en.apps.buildCard.build });
    await waitFor(() => expect(build).toBeEnabled());
    await user.click(build);
    expect(await screen.findByRole("alert")).toHaveTextContent(en.apps.buildCard.conflict.replace("{name}", NAME));
  });

  it("builds only from an event that names endpoints, an offered access, a run's audience and words", () => {
    expect(appBuildOf({ endpoints: ["alerts"], access: "read", visibility: "project", prompt: " a map " })).toEqual({
      endpoints: ["alerts"],
      access: "read",
      visibility: "project",
      prompt: "a map",
    });
    expect(appBuildOf({ endpoints: ["alerts"], access: "read", visibility: "public", prompt: "a map" })).toBeNull();
    expect(appBuildOf({ endpoints: ["alerts"], access: "admin", visibility: "project", prompt: "a map" })).toBeNull();
    expect(appBuildOf({ endpoints: [], access: "read", visibility: "project", prompt: "a map" })).toBeNull();
    expect(appBuildOf({ endpoints: ["alerts"], access: "read", visibility: "project", prompt: "  " })).toBeNull();
  });

  // T-3986: the app builder draws this card only in a model run, which the live walk cannot start (spend guard).
  it("has_no_serious_axe_violation_as_a_model_run_draws_it", async () => {
    renderCard(BUILD);
    await screen.findByRole("button", { name: en.apps.buildCard.build });
    await expectNoAxeViolations(document.body);
  });
});
