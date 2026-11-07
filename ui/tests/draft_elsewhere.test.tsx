/**
 * T-2768: a page opened on `?draft=` that holds no draft of that name says so, and links a draft
 * of another kind to its own page. The owner landed on the model import screen with
 * `?draft=sample-endpoint`, an endpoint's draft, and nothing said why.
 */
import { render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, describe, expect, it, vi } from "vitest";
import { createRootRoute, createRoute, createRouter, Outlet, RouterProvider } from "@tanstack/react-router";
import i18n from "../src/i18n";
import { DraftElsewhere, draftPageOf } from "../src/assistant/DraftElsewhere";

const DRAFTS = [
  { kind: "Endpoint", name: "sample-endpoint", touchedBy: "piper@hel.fi", touchedKind: "assistant", version: 1, updatedAt: "2026-09-25T08:00:00Z" },
  { kind: "Layer", name: "bikes-layer", touchedBy: "piper@hel.fi", touchedKind: "person", version: 2, updatedAt: "2026-09-25T08:00:00Z" },
];

function renderAt(address: string) {
  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    const url = new URL((input as Request).url, "http://localhost");
    const body = url.pathname.endsWith("/drafts") ? { items: DRAFTS } : { items: [] };
    return Promise.resolve(new Response(JSON.stringify(body), { headers: { "Content-Type": "application/json" } }));
  });
  vi.stubGlobal("fetch", fetchMock);
  const root = createRootRoute({ component: Outlet });
  const page = createRoute({
    getParentRoute: () => root,
    path: "/projects/$project/$plural",
    component: function Page() {
      const { project, plural } = page.useParams();
      return (
        <main>
          <DraftElsewhere project={project} page={plural} />
          <p>page {plural}</p>
        </main>
      );
    },
  });
  window.history.pushState({}, "", address);
  const router = createRouter({ routeTree: root.addChildren([page]) });
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>
        <RouterProvider router={router} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return fetchMock;
}

describe("DraftElsewhere (T-2768)", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("links an endpoint's draft opened on the models page to the endpoints page", async () => {
    renderAt("/projects/helsinki/models?draft=sample-endpoint");
    expect(await screen.findByText("No draft called “sample-endpoint” on this page")).toBeTruthy();
    const link = screen.getByRole("link", { name: "Open it in Endpoints" });
    expect(link.getAttribute("href")).toBe("/projects/helsinki/endpoints?draft=sample-endpoint");
  });

  it("says nothing on the page that holds the draft, a layer on the dashboards page too", async () => {
    const fetchMock = renderAt("/projects/helsinki/endpoints?draft=sample-endpoint");
    await screen.findByText("page endpoints");
    await vi.waitFor(() => expect(fetchMock).toHaveBeenCalled());
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(screen.queryByRole("status")).toBeNull();
    expect(draftPageOf("Layer")).toBe("dashboards");
  });

  it("says a name no draft has is none, with no link to follow", async () => {
    renderAt("/projects/helsinki/spaces?draft=trams");
    expect(await screen.findByText(/holds no draft of that name that you may read/)).toBeTruthy();
    expect(screen.queryByRole("link")).toBeNull();
  });

  it.each([
    ["a page that opens no draft", "/projects/helsinki/activity?draft=sample-endpoint", "page activity"],
    ["no draft in the address", "/projects/helsinki/models", "page models"],
  ])("reads no drafts on %s", async (_, address, shown) => {
    const fetchMock = renderAt(address);
    await screen.findByText(shown);
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(fetchMock).not.toHaveBeenCalled();
  });
});
