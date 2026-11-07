/**
 * T-1855: the router's own two pages, against the UI contract (UI-01, UI-15, UI-16, UI-48).
 *
 * Neither is a route anybody wrote on purpose, and both are what a person meets on a bad day.
 * A project list that came back 403 — an expired session, a role removed — was drawn as "the
 * repository holds no project", the Portal telling somebody their organisation's work is gone;
 * and an address with no page behind it fell through to the router's own built-in fallback,
 * two untranslated words on a white page with no way back.
 */
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { RouterProvider } from "@tanstack/react-router";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n, { SUPPORTED_LOCALES } from "../src/i18n";
import en from "../src/locales/en.json";
import { expectNoRawKeys, expectNoViolations } from "./checks";
import { createPortalRouter } from "../src/router";
import { problem, renderRoute } from "./pageHarness";
import type { AuthState } from "../src/auth/AuthProvider";

const AUTH: AuthState = {
  status: "authenticated",
  identity: {
    subject: "b7c1e0f4",
    username: "jana",
    name: "Jana",
    email: "jana@bb.sk",
    roles: [],
    front: "portal",
  },
  roles: [],
  hasRole: () => true,
  signIn: () => undefined,
  signOut: () => Promise.resolve(),
};

type Projects = { status: number; body: unknown } | "hangs";

function show(path: string, projects: Projects = { status: 200, body: { items: [] } }) {
  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    const url = new URL(input instanceof Request ? input.url : String(input), window.location.origin);
    if (url.pathname === "/api/v1/projects") {
      if (projects === "hangs") {
        return new Promise<Response>(() => undefined);
      }
      return Promise.resolve(
        new Response(JSON.stringify(projects.body), {
          status: projects.status,
          headers: { "Content-Type": "application/json" },
        }),
      );
    }
    return Promise.resolve(
      new Response(JSON.stringify({ items: [] }), {
        status: 200,
        headers: { "Content-Type": "application/json" },
      }),
    );
  });
  vi.stubGlobal("fetch", fetchMock);
  window.history.pushState({}, "", path);

  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const router = createPortalRouter();
  const view = render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <RouterProvider router={router} context={{ auth: AUTH }} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return { fetchMock, container: view.container, unmount: view.unmount, user: userEvent.setup() };
}

describe("the router's own pages", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    window.history.pushState({}, "", "/");
  });

  it("says what the API said when the project list cannot be read", async () => {
    show("/", { status: 403, body: { title: "Forbidden", detail: "your session has expired" } });
    expect(await screen.findByText("your session has expired")).toBeInTheDocument();
    expect(screen.queryByText(en.projects.empty.title)).toBeNull();
  });

  it("asks for the project list again when the failure is pressed", async () => {
    const { fetchMock, user } = show("/", { status: 500, body: { detail: "the forge is down" } });
    await screen.findByText("the forge is down");
    const before = fetchMock.mock.calls.length;
    await user.click(screen.getByRole("button", { name: en.app.error.retry }));
    expect(fetchMock.mock.calls.length).toBeGreaterThan(before);
  });

  it("sends a member, signed in with no project, to the catalogue (ADR-N-067, T-6411)", async () => {
    show("/", { status: 200, body: { items: [] } });
    await waitFor(() => expect(window.location.pathname).toBe("/catalogue"));
    expect(screen.queryByText(en.projects.empty.title)).toBeNull();
  });

  it("waits while the list is on its way", async () => {
    show("/", "hangs");
    expect(await screen.findByRole("status")).toHaveTextContent(en.projects.loading);
  });

  it("gives an address with no page behind it a page, with a way back", async () => {
    show("/projects/helsinki/this-is-not-a-page/at-all");
    expect(await screen.findByText(en.app.notFound.title)).toBeInTheDocument();
    expect(screen.getByText(/this-is-not-a-page/)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: en.app.notFound.home })).toHaveAttribute("href", "/");
  });

  it("has no axe violation on either page", async () => {
    const missing = show("/nowhere-at-all");
    await screen.findByText(en.app.notFound.title);
    await expectNoViolations(missing.container);
    missing.unmount();
    vi.unstubAllGlobals();

    const failed = show("/", { status: 403, body: { detail: "no" } });
    await screen.findByText("no");
    await expectNoViolations(failed.container);
  });

  it("shows no raw translation key in any locale the organisation offers", async () => {
    for (const locale of SUPPORTED_LOCALES) {
      await i18n.changeLanguage(locale);
      const { container, unmount } = show("/nowhere-at-all");
      await screen.findByRole("link");
      expectNoRawKeys(container);
      unmount();
      vi.unstubAllGlobals();
    }
    await i18n.changeLanguage("en");
  });
});

// T-2749: a section the project does not have showed its guessed heading over the API's raw
// "plural '...' not found" and a Retry that could never succeed.
describe("a section the API does not serve", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    window.history.pushState({}, "", "/");
  });

  it("is the Portal's not-found page, not a raw API error with a Retry", async () => {
    await renderRoute({
      path: "/projects/helsinki/nonexistent-section",
      answer: (path) =>
        path === "/api/v1/projects/helsinki/nonexistent-section"
          ? problem(404, "plural 'nonexistent-section' not found in project 'helsinki'")
          : undefined,
    });
    expect(await screen.findByText(en.app.notFound.title)).toBeInTheDocument();
    expect(screen.getByText(/nonexistent-section/)).toBeInTheDocument();
    expect(screen.queryByText(/plural '/)).toBeNull();
    expect(screen.queryByRole("button", { name: en.app.error.retry })).toBeNull();
    expect(screen.getByRole("link", { name: en.app.notFound.home })).toHaveAttribute("href", "/");
  });

  it("still lists a kind the API does serve under its own heading", async () => {
    await renderRoute({ path: "/projects/helsinki/mappings" });
    expect(await screen.findByText(en.resourceList.empty)).toBeInTheDocument();
    expect(screen.queryByText(en.app.notFound.title)).toBeNull();
  });
});

// T-4817: `/pipelines/air-quality/runs` drew the Application "Air quality" under Pipelines: the
// router handed an address no route matched whole to the deepest partial match, `$name`.
describe("an unknown sub-path of a resource", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    window.history.pushState({}, "", "/");
  });

  it.each([
    "/projects/helsinki/pipelines/air-quality/runs",
    "/projects/helsinki/spaces/air/whatever",
    "/projects/helsinki/endpoints/air/edit/more",
  ])("%s is the not-found page, never another kind's page", async (path) => {
    await renderRoute({ path });
    expect(await screen.findByText(en.app.notFound.title)).toBeInTheDocument();
    expect(screen.queryByText(en.apps.build.notOnForge)).toBeNull();
  });
});

