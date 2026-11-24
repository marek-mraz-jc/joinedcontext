// T-7589: the JSON editor on a phone (folding picker, manifest/preview tabs, refused widget) and a check that never reaches the server.
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createRootRoute, createRoute, createRouter, Outlet, RouterProvider } from "@tanstack/react-router";
import { I18nextProvider } from "react-i18next";
import userEvent from "@testing-library/user-event";
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
const refuse = vi.hoisted(() => ({ on: false }));
vi.mock("../src/api/odata", async (importOriginal) => {
  const real = await importOriginal<typeof import("../src/api/odata")>();
  return {
    ...real,
    // A check that fails before it reaches the service, as a plain Error and not as a refusal.
    proposeManifest: (...args: Parameters<typeof real.proposeManifest>) =>
      refuse.on ? Promise.reject(new Error("the check could not start")) : real.proposeManifest(...args),
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


const DASHBOARD = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Dashboard",
  metadata: { name: "dash1", namespace: "bbsk" },
  spec: {
    title: "Dash",
    visibility: "organization",
    pages: [{ title: "Overview", widgets: [{ widgetType: "vega-lite", endpointRef: "bikes", data: { source: "csv", type: "BusStation" } }] }],
  },
};

function stubServer(dryRun: () => Response | Promise<Response>) {
  const sent: { method: string; url: URL }[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const request = input as Request;
      const url = new URL(request.url);
      const json = (body: unknown, status = 200) =>
        new Response(JSON.stringify(body), { status, headers: { "Content-Type": status >= 400 ? "application/problem+json" : "application/json" } });
      if (request.method !== "GET") {
        sent.push({ method: request.method, url });
        return dryRun();
      }
      if (url.pathname.endsWith("/auth/me")) return json(IDENTITY);
      if (url.pathname.endsWith("/kinds/Dashboard/schema")) return json(SCHEMA);
      if (url.pathname.endsWith("/endpoints")) return json({ apiVersion: "v1", kind: "List", items: ENDPOINTS });
      if (url.pathname.endsWith("/spaces")) return json({ apiVersion: "v1", kind: "List", items: [SPACE] });
      if (url.pathname.endsWith("/datamodels")) return json({ apiVersion: "v1", kind: "List", items: [MODEL] });
      if (url.pathname.endsWith("/dashboards")) return json({ apiVersion: "v1", kind: "List", items: [DASHBOARD] });
      return json({ apiVersion: "v1", kind: "List", items: [] });
    }),
  );
  return sent;
}

function onPhone(matches: boolean) {
  vi.stubGlobal("matchMedia", (query: string) => ({
    matches,
    media: query,
    onchange: null,
    addListener: () => {},
    removeListener: () => {},
    addEventListener: () => {},
    removeEventListener: () => {},
    dispatchEvent: () => false,
  }));
}

describe("DashboardJsonEditor, phone and failed check", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  afterEach(() => {
    cleanup();
    refuse.on = false;
    vi.unstubAllGlobals();
  });

  it("shows the message of a check that failed before it reached the server", async () => {
    refuse.on = true;
    stubServer(() => new Response("{}"));
    const user = userEvent.setup();
    renderEditor(<DashboardJsonEditor project="bbsk" name="dash1" />);
    await screen.findByRole("textbox", { name: en.dashboards.json.source });
    await user.click(await screen.findByRole("button", { name: en.dashboards.json.check }));
    expect(await screen.findByText(en.dashboards.json.refused)).toBeInTheDocument();
    expect(screen.getByText(/the check could not start/)).toBeInTheDocument();
  });

  it("lists only the findings of a passed check that are errors", async () => {
    stubServer(
      () =>
        new Response(
          JSON.stringify({
            valid: true,
            verdict: {
              ok: false,
              findings: [
                { level: "error", path: "/spec/title", message: "title is taken" },
                { level: "warning", path: "/spec/description", message: "description is short" },
              ],
            },
          }),
          { status: 200, headers: { "Content-Type": "application/json" } },
        ),
    );
    const user = userEvent.setup();
    renderEditor(<DashboardJsonEditor project="bbsk" name="dash1" />);
    await screen.findByRole("textbox", { name: en.dashboards.json.source });
    await user.click(await screen.findByRole("button", { name: en.dashboards.json.check }));
    expect(await screen.findByText(en.dashboards.json.refused)).toBeInTheDocument();
    expect(screen.getByText(/title is taken/)).toBeInTheDocument();
    expect(screen.queryByText(/description is short/)).not.toBeInTheDocument();
  });

  it("folds the picker on a phone and opens it again", async () => {
    onPhone(true);
    stubServer(() => new Response("{}"));
    const user = userEvent.setup();
    renderEditor(<DashboardJsonEditor project="bbsk" name="dash1" />);
    const summary = await screen.findByText(en.dashboards.json.picker);
    const details = summary.closest("details") as HTMLDetailsElement;
    expect(details.open).toBe(false);
    await user.click(summary);
    await waitFor(() => expect(details.open).toBe(true));
    await user.click(summary);
    await waitFor(() => expect(details.open).toBe(false));
  });

  it("takes a phone from the refused widget back to the manifest", async () => {
    onPhone(true);
    const sent = stubServer(
      () =>
        new Response(
          JSON.stringify({ title: "Invalid", status: 422, detail: "refused", errors: [{ path: "/spec/pages/0/widgets/0/data/type", message: "unknown entity type" }] }),
          { status: 422, headers: { "Content-Type": "application/problem+json" } },
        ),
    );
    const user = userEvent.setup();
    renderEditor(<DashboardJsonEditor project="bbsk" name="dash1" />);
    await screen.findByRole("textbox", { name: en.dashboards.json.source });
    await user.click(await screen.findByRole("button", { name: en.dashboards.json.check }));
    await waitFor(() => expect(sent.some((s) => s.url.searchParams.get("dryRun") === "All")).toBe(true));
    await user.click(await screen.findByRole("tab", { name: en.dashboards.json.previewNeedsFix }));
    await user.click(await screen.findByRole("button", { name: en.dashboards.json.toManifest }));
    expect(screen.getByRole("tab", { name: en.dashboards.json.source })).toHaveAttribute("aria-selected", "true");
  });
});
