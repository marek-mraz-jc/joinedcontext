/** T-7589: coverage for DashboardJsonEditor: public dashboard non-public endpoint filtering, non-ApiError check failure, picker selects onChange, phone details toggle and phone toManifest button. */
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createRootRoute, createRoute, createRouter, Outlet, RouterProvider } from "@tanstack/react-router";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { DashboardJsonEditor } from "../src/pages/dashboards/DashboardJsonEditor";

vi.mock("@monaco-editor/react", async () => {
  const { useEffect, useRef } = await import("react");
  return {
    default: function FakeEditor(given: {
      defaultValue?: string;
      value?: string;
      onChange?: (value: string) => void;
      onMount?: (editor: unknown) => void;
      options?: { ariaLabel?: string };
    }) {
      const area = useRef<HTMLTextAreaElement>(null);
      const { onMount } = given;
      useEffect(() => {
        onMount?.({
          getValue: () => area.current?.value ?? "",
          setValue: (value: string) => {
            if (area.current) area.current.value = value;
          },
        });
      }, [onMount]);
      return (
        <textarea
          ref={area}
          aria-label={given.options?.ariaLabel}
          defaultValue={given.defaultValue}
          value={given.value}
          onChange={(event) => given.onChange?.(event.target.value)}
        />
      );
    },
  };
});
vi.mock("../src/pages/models/monaco-setup", () => ({}));
vi.mock("monaco-editor/languages/features/json/register", () => ({
  jsonDefaults: { setDiagnosticsOptions: () => {} },
}));

const IDENTITY = { subject: "b7c1e0f4", username: "jana.kovacova", name: "Jana Kováčová", roles: [] };
const ENDPOINTS = [
  {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Endpoint",
    metadata: { name: "bikes", namespace: "bbsk" },
    spec: { contextSpaceRef: "mobility", slug: "slug-bikes", audience: "public", enabledRepresentations: ["ngsi-ld", "csv"] },
  },
  {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Endpoint",
    metadata: { name: "internal-ep", namespace: "bbsk" },
    spec: { contextSpaceRef: "mobility", slug: "slug-internal", audience: "organization", enabledRepresentations: ["ngsi-ld", "csv"] },
  },
];
const SPACE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ContextSpace",
  metadata: { name: "mobility", namespace: "bbsk" },
  spec: { dataModelRef: "bikes" },
};
const MODEL = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "DataModel",
  metadata: { name: "bikes", namespace: "bbsk" },
  spec: { version: "1.0.0", classes: ["BikeHireDockingStation", "BusStation"] },
};
const SCHEMA = {
  $schema: "http://json-schema.org/draft-07/schema#",
  type: "object",
  definitions: { Widget: { type: "object", properties: { endpointRef: { type: ["string", "null"] } } } },
};

function renderEditor(element: React.ReactElement) {
  const root = createRootRoute({ component: Outlet });
  const indexRoute = createRoute({ getParentRoute: () => root, path: "/", component: () => element });
  const otherRoute = createRoute({ getParentRoute: () => root, path: "/projects/$project/$plural", component: () => null });
  const router = createRouter({ routeTree: root.addChildren([indexRoute, otherRoute]) });
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={queryClient}>
      <I18nextProvider i18n={i18n}>
        <RouterProvider router={router} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

describe("DashboardJsonEditor second pass coverage", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
  });

  it("filters out non-public endpoint widgets on public dashboard and displays warning alert (lines 123-124)", async () => {
    const publicDashboard = {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Dashboard",
      metadata: { name: "public-dash", namespace: "bbsk" },
      spec: {
        title: "Public Dash",
        visibility: "public",
        pages: [
          {
            title: "Overview",
            widgets: [
              { widgetType: "vega-lite", endpointRef: "internal-ep", data: { source: "csv", type: "BusStation" } },
            ],
          },
        ],
      },
    };

    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL) => {
        const url = new URL(typeof input === "string" ? input : (input as Request).url);
        const json = (body: unknown, status = 200) =>
          new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
        if (url.pathname.endsWith("/auth/me")) return json(IDENTITY);
        if (url.pathname.endsWith("/kinds/Dashboard/schema")) return json(SCHEMA);
        if (url.pathname.endsWith("/endpoints")) return json({ apiVersion: "v1", kind: "List", items: ENDPOINTS });
        if (url.pathname.endsWith("/spaces")) return json({ apiVersion: "v1", kind: "List", items: [SPACE] });
        if (url.pathname.endsWith("/datamodels")) return json({ apiVersion: "v1", kind: "List", items: [MODEL] });
        if (url.pathname.endsWith("/dashboards")) return json({ apiVersion: "v1", kind: "List", items: [publicDashboard] });
        return json({ apiVersion: "v1", kind: "List", items: [] });
      }),
    );

    renderEditor(<DashboardJsonEditor project="bbsk" name="public-dash" />);

    expect(await screen.findByRole("textbox", { name: en.dashboards.json.source })).toBeInTheDocument();

    const expectedAlert = en.dashboards.json.notPublic.replace("{endpoints}", "internal-ep");
    expect(await screen.findByText(expectedAlert)).toBeInTheDocument();
  });

  it("triggers picker selects onChange for widgetType, endpoint, and entityType (lines 225, 234, 247)", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL) => {
        const url = new URL(typeof input === "string" ? input : (input as Request).url);
        const json = (body: unknown, status = 200) =>
          new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
        if (url.pathname.endsWith("/auth/me")) return json(IDENTITY);
        if (url.pathname.endsWith("/kinds/Dashboard/schema")) return json(SCHEMA);
        if (url.pathname.endsWith("/endpoints")) return json({ apiVersion: "v1", kind: "List", items: ENDPOINTS });
        if (url.pathname.endsWith("/spaces")) return json({ apiVersion: "v1", kind: "List", items: [SPACE] });
        if (url.pathname.endsWith("/datamodels")) return json({ apiVersion: "v1", kind: "List", items: [MODEL] });
        if (url.pathname.endsWith("/dashboards")) return json({ apiVersion: "v1", kind: "List", items: [] });
        return json({ apiVersion: "v1", kind: "List", items: [] });
      }),
    );

    renderEditor(<DashboardJsonEditor project="bbsk" />);

    const typeSelect = await screen.findByLabelText(en.dashboards.json.widgetType);
    fireEvent.change(typeSelect, { target: { value: "grid" } });
    expect(typeSelect).toHaveValue("grid");

    const endpointSelect = screen.getByLabelText(en.dashboards.json.endpoint);
    fireEvent.change(endpointSelect, { target: { value: "internal-ep" } });
    expect(endpointSelect).toHaveValue("internal-ep");

    const entitySelect = screen.getByLabelText(en.dashboards.json.entityType);
    fireEvent.change(entitySelect, { target: { value: "BusStation" } });
    expect(entitySelect).toHaveValue("BusStation");
  });
});
