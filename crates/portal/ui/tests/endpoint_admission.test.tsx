/**
 * T-6802: a person the gateway's door refuses (an administrator outside the Endpoint's project
 * groups) sees the refused state on the Endpoint page and on settings/access, and the page asks
 * nothing under `/api/endpoint/{slug}`; an admitted person reads as before.
 */
import { render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { createRootRoute, createRouter, RouterProvider } from "@tanstack/react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { EndpointPage } from "../src/pages/endpoints/EndpointPage";
import { EndpointsPage } from "../src/routes/EndpointsPage";
import { EffectivePermissions } from "../src/pages/access/EffectivePermissions";
import { REQUEST_ACCESS_ANCHOR } from "../src/components/endpoints/EndpointAdmission";

const PROJECT = "helsinki";
const NAME = "app-air-quality";
const SLUG = "3jlwl5dg2oidblwqhc6qdi3zxl";
const ENDPOINT = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Endpoint",
  metadata: { name: NAME, namespace: PROJECT },
  spec: { contextSpaceRef: "air", slug: SLUG, audience: "project-list", enabledRepresentations: ["ngsi-ld", "mcp"] },
  status: { phase: "Live" },
};
const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });

/**
 * Every path the page asked, after a stubbed fetch that answers `admitted`; `bootstrap` makes the
 * person the project's administrator, who may edit the Endpoint.
 */
function serve(admitted: boolean, bootstrap = false): string[] {
  const asked: string[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL) => {
      const path = new URL(input instanceof Request ? input.url : String(input), window.location.origin).pathname;
      asked.push(path);
      const json = (body: unknown, status = 200) =>
        Promise.resolve(new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } }));
      if (path.endsWith(`/endpoints/${NAME}/admission`)) return json({ admitted });
      if (path.endsWith(`/endpoints/${NAME}`)) return json(ENDPOINT);
      if (path.endsWith("/endpoints")) return json(list([ENDPOINT]));
      if (path.endsWith("/permissions/me")) return json({ project: PROJECT, bootstrap, grants: [] });
      if (path.startsWith("/api/endpoint/")) return json({ title: "Forbidden", status: 403 }, 403);
      return json(list([]));
    }),
  );
  return asked;
}

function show(node: () => React.JSX.Element) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const router = createRouter({ routeTree: createRootRoute({ component: node }) });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <RouterProvider router={router} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

const gatewayReads = (asked: string[]) => asked.filter((path) => path.startsWith("/api/endpoint/"));

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("the Endpoint page for a person the gateway does not admit", () => {
  it("shows the refused state and reads nothing through the Endpoint", async () => {
    const asked = serve(false);
    show(() => <EndpointPage project={PROJECT} name={NAME} />);
    expect(await screen.findByText(en.endpoints.refused.title)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: en.endpoints.refused.action })).toHaveAttribute("href", `#${REQUEST_ACCESS_ANCHOR}`);
    expect(document.getElementById(REQUEST_ACCESS_ANCHOR)).not.toBeNull();
    // Let every query the page started settle before counting.
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(gatewayReads(asked)).toEqual([]);
  });

  it("reads through the Endpoint once admitted", async () => {
    const asked = serve(true);
    show(() => <EndpointPage project={PROJECT} name={NAME} />);
    await vi.waitFor(() => expect(gatewayReads(asked).length).toBeGreaterThan(0));
    expect(screen.queryByText(en.endpoints.refused.title)).not.toBeInTheDocument();
  });
});

describe("the Endpoint edit page for a person the gateway does not admit", () => {
  it("shows the refused state in place of the schema and data, and reads nothing through the Endpoint", async () => {
    const asked = serve(false, true);
    show(() => <EndpointsPage project={PROJECT} edit={NAME} />);
    expect(await screen.findByText(en.endpoints.refused.title)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: en.endpoints.refused.action })).toHaveAttribute(
      "href",
      `/projects/${PROJECT}/endpoints/${NAME}#${REQUEST_ACCESS_ANCHOR}`,
    );
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(gatewayReads(asked)).toEqual([]);
  });

  it("reads the schema through the Endpoint once admitted", async () => {
    const asked = serve(true, true);
    show(() => <EndpointsPage project={PROJECT} edit={NAME} />);
    await vi.waitFor(() => expect(asked).toContain(`/api/endpoint/${SLUG}/schema/index.json`));
    expect(screen.queryByText(en.endpoints.refused.title)).not.toBeInTheDocument();
  });
});

describe("settings/access for a person the gateway does not admit", () => {
  it("shows the refused state with a link to ask, and never asks /access", async () => {
    const asked = serve(false);
    show(() => <EffectivePermissions project={PROJECT} />);
    expect(await screen.findByText(en.endpoints.refused.title)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: en.endpoints.refused.action })).toHaveAttribute(
      "href",
      `/projects/${PROJECT}/endpoints/${NAME}#${REQUEST_ACCESS_ANCHOR}`,
    );
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(gatewayReads(asked)).toEqual([]);
  });

  it("asks /access once admitted", async () => {
    const asked = serve(true);
    show(() => <EffectivePermissions project={PROJECT} />);
    await vi.waitFor(() => expect(asked).toContain(`/api/endpoint/${SLUG}/access`));
    expect(screen.queryByText(en.endpoints.refused.title)).not.toBeInTheDocument();
  });
});
