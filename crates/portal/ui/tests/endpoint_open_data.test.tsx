/**
 * EP-187 (T-6972): the reconciler marks an open-data Endpoint `OpenData`, and one that binds a
 * write Policy also `Degraded`; the Endpoint page shows the read-only badge, and the warning
 * with the server's sentence and what to change. An internal Endpoint shows neither.
 */
import { render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { createRootRoute, createRouter, RouterProvider } from "@tanstack/react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { EndpointPage } from "../src/pages/endpoints/EndpointPage";

const PROJECT = "helsinki";
const NAME = "air-open";
const SLUG = "zt4qm7ge2xdv6ksb3ncf5arw2y";
const WHY =
  "Endpoint air-open: an open-data Endpoint is read-only (EP-187): Policy sensors-write grants updateAttrs";
const condition = (type: string, reason: string, message: string) => ({
  type,
  status: "True",
  reason,
  message,
  lastTransitionTime: "2026-11-15T08:00:00Z",
});
const endpoint = (conditions: unknown[]) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Endpoint",
  metadata: { name: NAME, namespace: PROJECT },
  spec: { contextSpaceRef: "air", slug: SLUG, audience: "public", enabledRepresentations: ["ngsi-ld"] },
  status: { phase: "Live", conditions },
});
const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });

function serve(manifest: unknown) {
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL) => {
      const path = new URL(input instanceof Request ? input.url : String(input), window.location.origin).pathname;
      const json = (body: unknown, status = 200) =>
        Promise.resolve(new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } }));
      if (path.endsWith(`/endpoints/${NAME}/admission`)) return json({ admitted: true });
      if (path.endsWith(`/endpoints/${NAME}`)) return json(manifest);
      if (path.endsWith("/permissions/me")) return json({ project: PROJECT, bootstrap: false, grants: [] });
      return json(list([]));
    }),
  );
}

function show() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const router = createRouter({ routeTree: createRootRoute({ component: () => <EndpointPage project={PROJECT} name={NAME} /> }) });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <RouterProvider router={router} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("an open-data Endpoint's page", () => {
  it("shows the read-only badge, and the warning naming the write Policy with what to change", async () => {
    serve(
      endpoint([
        condition("OpenData", "ReadOnly", "open data: every write through this Endpoint is refused (EP-187)"),
        condition("Degraded", "OpenDataWritePolicy", WHY),
      ]),
    );
    show();
    const badge = await screen.findByText(en.endpoints.page.openData);
    expect(badge.getAttribute("title")).toBe(en.endpoints.page.openDataHint);
    const title = await screen.findByText(en.endpoints.page.openDataWrite);
    const warning = title.closest('[role="status"]');
    expect(warning?.textContent).toContain(WHY);
    expect(warning?.textContent).toContain("urn:ngsi-ld:Policy:city.example:air:public-read");
  });

  it("shows the badge alone while it only reads", async () => {
    serve(endpoint([condition("OpenData", "ReadOnly", "open data")]));
    show();
    await screen.findByText(en.endpoints.page.openData);
    expect(screen.queryByText(en.endpoints.page.openDataWrite)).toBeNull();
  });

  it("shows nothing on an internal Endpoint", async () => {
    serve(endpoint([]));
    show();
    await screen.findAllByText(NAME);
    expect(screen.queryByText(en.endpoints.page.openData)).toBeNull();
    expect(screen.queryByText(en.endpoints.page.openDataWrite)).toBeNull();
  });
});
