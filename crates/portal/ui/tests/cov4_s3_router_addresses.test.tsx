// T-7706: router addresses nobody opened: welcome link, settings tab with endpoint, organization tabs and agent page, redirects, 404s, gallery.
import { cleanup, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useMemo } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import en from "../src/locales/en.json";
import { render } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import i18n from "../src/i18n";
import { RouterProvider } from "@tanstack/react-router";
import { AuthProvider, useAuth } from "../src/auth/AuthProvider";
import { createPortalRouter } from "../src/router";
import { jsonResponse, list, problem, renderRoute } from "./pageHarness";

/** An answer whose body never ends: the query stays pending. */
const never = () => new Response(new ReadableStream({ start() {} }), { headers: { "Content-Type": "application/json" } });

/** The router with no gate before it, as the catalogue's own frame meets a session that is still read. */
function RouterWithoutGate() {
  const auth = useAuth();
  const router = useMemo(() => createPortalRouter(), []);
  return <RouterProvider router={router} context={{ auth }} />;
}

const ADMIN = { project: "helsinki", bootstrap: true, grants: [] };

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("addresses of the router", () => {
  it("closes the welcome of an invitation's link and drops it from the address", async () => {
    const user = userEvent.setup();
    await renderRoute({ path: "/projects/helsinki/home?welcome=1", permissions: ADMIN });
    const welcome = await screen.findByRole("region", { name: "Welcome to helsinki" });
    await user.click(within(welcome).getByRole("button", { name: en.home.welcome.close }));
    expect(screen.queryByRole("region", { name: "Welcome to helsinki" })).not.toBeInTheDocument();
    expect(window.location.search).toBe("");
  });

  it("opens a project settings tab with the endpoint it names and refuses an unknown tab", async () => {
    await renderRoute({ path: "/projects/helsinki/settings/access?endpoint=air-quality", permissions: ADMIN });
    expect(await screen.findByRole("heading", { level: 1 })).toBeInTheDocument();
    expect(window.location.pathname).toBe("/projects/helsinki/settings/access");
    cleanup();
    await renderRoute({ path: "/projects/helsinki/settings/nonsense", permissions: ADMIN });
    expect(await screen.findByRole("heading", { level: 1, name: en.app.notFound.title })).toBeInTheDocument();
  });

  it("opens an organization tab by its address and an unknown one as not found", async () => {
    await renderRoute({ path: "/organization/people", permissions: ADMIN });
    expect(await screen.findByRole("heading", { level: 1 })).toBeInTheDocument();
    cleanup();
    await renderRoute({ path: "/organization/nonsense", permissions: ADMIN });
    expect(await screen.findByRole("heading", { level: 1, name: en.app.notFound.title })).toBeInTheDocument();
  });

  it("opens an agent's page inside the organization's frame", async () => {
    await renderRoute({
      path: "/organization/agents/helsinki/AssistantDeployment/city-chat",
      permissions: ADMIN,
      answer: (path) => (path.includes("agents") ? problem(404, "no such agent") : undefined),
    });
    expect(await screen.findByRole("button", { name: "Retry" })).toBeInTheDocument();
    expect(window.location.pathname).toBe("/organization/agents/helsinki/AssistantDeployment/city-chat");
  });

  it("opens a new datamodel on the model page with a blank model", async () => {
    await renderRoute({ path: "/projects/helsinki/datamodels/new", permissions: ADMIN });
    expect(await screen.findByText("Unnamed model")).toBeInTheDocument();
    expect(window.location.pathname).toBe("/projects/helsinki/models");
    expect(window.location.search).toContain("blank");
  });

  it("answers not found for an App-only address on another kind", async () => {
    await renderRoute({ path: "/projects/helsinki/pipelines/p1/open", permissions: ADMIN });
    expect(await screen.findByRole("heading", { level: 1, name: en.app.notFound.title })).toBeInTheDocument();
  });

  it("opens an MQTT broker's own page", async () => {
    await renderRoute({
      path: "/projects/helsinki/mqttbrokers/hall",
      permissions: ADMIN,
      answer: (path) => (path.endsWith("/mqttbrokers") ? jsonResponse(list([])) : undefined),
    });
    expect(await screen.findByRole("heading", { level: 1 })).toBeInTheDocument();
  });

  it("opens the development gallery without a session", async () => {
    await renderRoute({ path: "/__gallery", identity: null });
    expect(await screen.findByRole("heading", { level: 1 })).toBeInTheDocument();
  });

  it("opens AI connectors and settings of a person who reads no project, in the member frame", async () => {
    await renderRoute({ path: "/ai-connectors", projects: () => jsonResponse(list([])) });
    expect(await screen.findByRole("heading", { level: 1 })).toBeInTheDocument();
    cleanup();
    await renderRoute({ path: "/settings", projects: () => jsonResponse(list([])) });
    expect(await screen.findByRole("heading", { level: 1 })).toBeInTheDocument();
  });

  it("waits on an empty page while a person with no project has no known right to open one", async () => {
    await renderRoute({ path: "/", projects: () => jsonResponse(list([])), permissionsFor: () => never() });
    await waitFor(() => expect(document.querySelector("main[aria-busy=true]")).not.toBeNull());
  });

  it("draws the catalogue as an empty busy page while the project list is read", async () => {
    await renderRoute({ path: "/catalogue", projects: () => never() });
    await waitFor(() => expect(document.querySelector("main[aria-busy=true]")).not.toBeNull());
  });

  it("draws an empty busy page while the session or the project list is not known", async () => {
    await renderRoute({ path: "/ai-connectors", pending: true });
    expect(document.querySelector("main[aria-busy=true]") ?? screen.queryByRole("status")).not.toBeNull();
  });

  it("draws the catalogue as an empty busy page while the session is not known yet", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn((input: RequestInfo | URL) => {
        const url = new URL(input instanceof Request ? input.url : String(input), window.location.origin);
        return url.pathname.endsWith("/auth/me") ? new Promise<Response>(() => {}) : Promise.resolve(jsonResponse(list([])));
      }),
    );
    window.history.pushState({}, "", "/catalogue");
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <I18nextProvider i18n={i18n}>
          <AuthProvider>
            <RouterWithoutGate />
          </AuthProvider>
        </I18nextProvider>
      </QueryClientProvider>,
    );
    await waitFor(() => expect(document.querySelector("main[aria-busy=true]")).not.toBeNull());
  });
});
