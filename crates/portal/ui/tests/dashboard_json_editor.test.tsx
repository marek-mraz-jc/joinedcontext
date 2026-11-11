/**
 * T-6791: the Dashboard JSON editor. The pure half (the starter, the picker's snippets, the
 * schema the editor completes from, where a parse error is) and one journey through the page:
 * insert a chart over an Endpoint the person may read, break the JSON and read where, have the
 * server refuse a widget and read its JSON path, then propose it as a Change.
 */
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { fireEvent } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createRootRoute, createRoute, createRouter, Outlet, RouterProvider } from "@tanstack/react-router";
import { I18nextProvider } from "react-i18next";
import { afterEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import {
  insertWidget,
  parseManifest,
  schemaWithEndpoints,
  starter,
  toText,
  widgetSnippet,
} from "../src/pages/dashboards/dashboardJson";
import { DashboardWidgets } from "../src/components/dashboards/DashboardWidgets";
import { DashboardJsonEditor } from "../src/pages/dashboards/DashboardJsonEditor";
import DashboardJsonMonaco from "../src/pages/dashboards/DashboardJsonMonaco";

const diagnostics = vi.hoisted(() => [] as unknown[]);

// Monaco needs a canvas and a worker jsdom has neither of: a text area takes its place, with
// the props the page hands it.
vi.mock("@monaco-editor/react", async () => {
  const { useEffect, useRef } = await import("react");
  return {
    default: function FakeEditor(given: {
      defaultValue?: string;
      onChange?: (value: string) => void;
      onMount?: (editor: unknown) => void;
      options?: { ariaLabel?: string };
    }) {
      const area = useRef<HTMLTextAreaElement>(null);
      useEffect(() => {
        given.onMount?.({
          getValue: () => area.current?.value ?? "",
          setValue: (value: string) => {
            if (area.current) area.current.value = value;
          },
        });
        // Mounted once, as Monaco is.
        // eslint-disable-next-line react-hooks/exhaustive-deps
      }, []);
      return (
        <textarea
          ref={area}
          aria-label={given.options?.ariaLabel}
          defaultValue={given.defaultValue}
          onChange={(event) => given.onChange?.(event.target.value)}
        />
      );
    },
  };
});
vi.mock("../src/pages/models/monaco-setup", () => ({}));
vi.mock("monaco-editor/languages/features/json/register", () => ({
  jsonDefaults: { setDiagnosticsOptions: (options: unknown) => diagnostics.push(options) },
}));

const { App } = await import("../src/App");

const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const IDENTITY = { subject: "b7c1e0f4", username: "jana.kovacova", name: "Jana Kováčová", roles: [] };
const SLUG = "k7m2qz4tv6xh3n5jb2ryd3wcfa";
const ENDPOINT = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Endpoint",
  metadata: { name: "bikes", namespace: "bbsk" },
  spec: { contextSpaceRef: "mobility", slug: SLUG, audience: "public", enabledRepresentations: ["ngsi-ld", "csv"] },
};
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
  spec: { version: "1.0.0", classes: ["BikeHireDockingStation"] },
};
const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-6791aa11", namespace: "bbsk" },
  status: { lane: "green", phase: "PendingApproval", plan: { create: 1 } },
};
const SCHEMA = {
  $schema: "http://json-schema.org/draft-07/schema#",
  type: "object",
  definitions: { Widget: { type: "object", properties: { endpointRef: { type: ["string", "null"] } } } },
};

describe("the dashboard JSON", () => {
  it("starts as a Dashboard of one empty page in the project", () => {
    const parsed = parseManifest(toText(starter("bbsk")));
    expect(parsed.manifest?.kind).toBe("Dashboard");
    expect(parsed.manifest?.metadata.namespace).toBe("bbsk");
    expect(parsed.manifest?.spec.pages).toEqual([{ title: "Overview", widgets: [] }]);
  });

  it("inserts a chart that reads the Endpoint's CSV and names no URL", () => {
    const widget = widgetSnippet("vega-lite", "bikes", "BikeHireDockingStation");
    expect(widget.data).toEqual({ source: "csv", type: "BikeHireDockingStation" });
    expect(widget.spec?.data).toEqual({ name: "source" });
    expect(JSON.stringify(widget)).not.toMatch(/"url"/);
    const text = insertWidget(toText(starter("bbsk")), widget);
    expect(parseManifest(text ?? "").manifest?.spec.pages[0].widgets).toEqual([widget]);
    expect(widgetSnippet("grid", "bikes", "BikeHireDockingStation")).toEqual({
      widgetType: "grid",
      endpointRef: "bikes",
      entityType: "BikeHireDockingStation",
    });
  });

  it("inserts nothing into text that does not read, and says where it stops", () => {
    expect(insertWidget("{\n  \"kind\": ", widgetSnippet("grid", "bikes", "T"))).toBeNull();
    const parsed = parseManifest("{\n  \"kind\": \"Dashboard\",\n  oops\n}");
    expect(parsed.error?.line).toBe(3);
    expect(parseManifest("[]").error?.message).toMatch(/not a JSON object/);
    expect(parseManifest("{\"kind\": \"Layer\"}").error?.message).toMatch(/Dashboard/);
  });

  it("completes endpointRef from the Endpoints the person may read only", () => {
    const schema = schemaWithEndpoints(SCHEMA, ["bikes", "parking"]) as typeof SCHEMA & {
      definitions: { Widget: { properties: { endpointRef: { enum?: string[] } } } };
    };
    expect(schema.definitions.Widget.properties.endpointRef.enum).toEqual(["bikes", "parking"]);
    // The kind's own schema is left as it was.
    expect((SCHEMA.definitions.Widget.properties.endpointRef as { enum?: string[] }).enum).toBeUndefined();
  });
});

describe("the dashboard JSON editor page", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  function stubServer(endpoints: unknown[] = [ENDPOINT], dashboards: unknown[] = []) {
    const sent: { method: string; url: URL; body: string }[] = [];
    const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
      const request = input as Request;
      const url = new URL(request.url);
      const json = (body: unknown, status = 200) =>
        new Response(JSON.stringify(body), {
          status,
          headers: { "Content-Type": status >= 400 ? "application/problem+json" : "application/json" },
        });
      if (request.method !== "GET") {
        const body = await request.clone().text();
        sent.push({ method: request.method, url, body });
        if (url.searchParams.get("dryRun") === "All" && body.includes("\"url\"")) {
          return json(
            {
              title: "Invalid",
              status: 422,
              detail: "the widget's spec names a URL",
              errors: [{ path: "/spec/pages/0/widgets/0/spec/data/url", message: "a vega-lite widget reads its Endpoint, never a URL" }],
            },
            422,
          );
        }
        if (url.searchParams.get("dryRun") === "All") {
          return json({ valid: true, verdict: { ok: true, findings: [] } });
        }
        return json(CHANGE, 202);
      }
      if (url.pathname.endsWith("/auth/me")) return json(IDENTITY);
      if (url.pathname.endsWith("/kinds/Dashboard/schema")) return json(SCHEMA);
      if (url.pathname.endsWith("/endpoints")) return json(list(endpoints));
      if (url.pathname.endsWith("/spaces")) return json(list([SPACE]));
      if (url.pathname.endsWith("/datamodels")) return json(list([MODEL]));
      if (url.pathname.endsWith("/dashboards")) return json(list(dashboards));
      if (url.pathname.includes("/api/endpoint/")) {
        return new Response("id,type\nurn:ngsi-ld:BikeHireDockingStation:1,BikeHireDockingStation\n", {
          headers: { "Content-Type": "text/csv" },
        });
      }
      return json(list([]));
    });
    vi.stubGlobal("fetch", fetchMock);
    return sent;
  }

  function renderEditor(endpoints: unknown[] = [ENDPOINT]) {
    const sent = stubServer(endpoints);
    window.history.pushState({}, "", "/projects/bbsk/dashboard-editor");
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <App />
        </I18nextProvider>
      </QueryClientProvider>,
    );
    return sent;
  }

  it("inserts a chart, names a broken line, shows a refusal by its path and proposes a Change", async () => {
    const user = userEvent.setup();
    const sent = renderEditor();
    const editor = (await screen.findByRole("textbox", { name: "Manifest (JSON)" })) as HTMLTextAreaElement;
    expect(screen.getByText(/No widget yet/)).toBeInTheDocument();
    // Every field the picker shows says what it takes, with an example.
    expect(screen.getByText(/Endpoints of this project you may read, e.g. bikes/)).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "bikes" })).toBeInTheDocument();
    await waitFor(() => expect(screen.getByRole("option", { name: "BikeHireDockingStation" })).toBeInTheDocument());

    await user.click(screen.getByRole("button", { name: "Insert widget" }));
    await waitFor(() => expect(editor.value).toContain("\"widgetType\": \"vega-lite\""));
    expect(editor.value).toContain("\"endpointRef\": \"bikes\"");
    // The editor's schema completes endpointRef from the readable Endpoints.
    expect(JSON.stringify(diagnostics.at(-1))).toContain("\"enum\":[\"bikes\"]");

    fireEvent.change(editor, { target: { value: "{\n  \"kind\": \"Dashboard\",\n  oops\n}" } });
    expect(await screen.findByText("The JSON does not read")).toBeInTheDocument();
    expect(screen.getByText(/^Line 3, column/)).toBeInTheDocument();
    expect(screen.getByText(/comes back as soon as the JSON reads/)).toBeInTheDocument();

    const manifest = {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Dashboard",
      metadata: { name: "ukazovatele", namespace: "bbsk" },
      spec: {
        title: "Ukazovatele",
        visibility: "public",
        pages: [{ title: "Overview", widgets: [{ widgetType: "vega-lite", endpointRef: "bikes", data: { source: "csv", type: "BikeHireDockingStation" }, spec: { data: { url: "https://evil.example/x.csv" }, mark: "bar" } }] }],
      },
    };
    fireEvent.change(editor, { target: { value: toText(manifest) } });
    await user.click(screen.getByRole("button", { name: "Check" }));
    const refused = await screen.findByText("The server refused it");
    const alert = refused.closest("[role]") ?? refused.parentElement!;
    expect(within(alert as HTMLElement).getByText(/\/spec\/pages\/0\/widgets\/0\/spec\/data\/url/)).toBeInTheDocument();
    // The refused widget is not drawn in the preview.
    expect(screen.getByText("This widget is not drawn")).toBeInTheDocument();

    manifest.spec.pages[0].widgets[0].spec = { data: { name: "source" }, mark: "bar" } as never;
    fireEvent.change(editor, { target: { value: toText(manifest) } });
    await user.click(screen.getByRole("button", { name: "Check" }));
    expect(await screen.findByText("The server accepts this dashboard")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Propose change" }));
    await waitFor(() => expect(sent.some((s) => s.method === "POST" && !s.url.searchParams.has("dryRun"))).toBe(true));
    const proposed = sent.find((s) => s.method === "POST" && !s.url.searchParams.has("dryRun"));
    expect(proposed?.url.pathname).toBe("/api/v1/projects/bbsk/dashboards");
    expect(JSON.parse(proposed?.body ?? "{}").metadata.name).toBe("ukazovatele");
  });

  it("says what to do when the project has no Endpoint to read", async () => {
    renderEditor([]);
    expect(await screen.findByText("No endpoint to read yet")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Go to endpoints" })).toBeInTheDocument();
  });

  it("opens an existing dashboard by name, with its manifest in the editor", async () => {
    const DASHBOARD = {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Dashboard",
      metadata: { name: "ukazovatele", namespace: "bbsk" },
      spec: { title: "Ukazovatele", pages: [{ title: "Overview", widgets: [] }] },
    };
    stubServer([ENDPOINT], [DASHBOARD]);
    window.history.pushState({}, "", "/");
    const root = createRootRoute({ component: Outlet });
    const page = createRoute({
      getParentRoute: () => root,
      path: "/",
      component: () => <DashboardJsonEditor project="bbsk" name="ukazovatele" />,
    });
    const endpoints = createRoute({ getParentRoute: () => root, path: "/projects/$project/$plural", component: () => null });
    const router = createRouter({ routeTree: root.addChildren([page, endpoints]) });
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <RouterProvider router={router} />
        </I18nextProvider>
      </QueryClientProvider>,
    );
    const editor = (await screen.findByRole("textbox", { name: "Manifest (JSON)" })) as HTMLTextAreaElement;
    await waitFor(() => expect(editor.value).toContain("\"name\": \"ukazovatele\""));
  });
});

describe("the dashboard JSON editor's parts", () => {
  it("binds the schema to its own model and keeps typed text it only echoes", async () => {
    const changes: string[] = [];
    const schema = { type: "object" };
    const view = render(
      <DashboardJsonMonaco value="{}" onChange={(next) => changes.push(next)} schema={schema} ariaLabel="Manifest" height="10rem" />,
    );
    const options = diagnostics.at(-1) as { schemas: { fileMatch: string[]; schema: unknown }[] };
    expect(options.schemas).toEqual([
      expect.objectContaining({ fileMatch: ["inmemory://dashboard/manifest.json"], schema }),
    ]);
    const area = screen.getByRole("textbox", { name: "Manifest" }) as HTMLTextAreaElement;
    fireEvent.change(area, { target: { value: "{\"a\": 1}" } });
    expect(changes).toEqual(["{\"a\": 1}"]);
    // A value from outside, the picker's, replaces the text.
    await act(async () => {
      view.rerender(
        <DashboardJsonMonaco value={JSON.stringify({ b: 2 })} onChange={(next) => changes.push(next)} schema={null} ariaLabel="Manifest" height="10rem" />,
      );
    });
    expect(area.value).toBe(JSON.stringify({ b: 2 }));
    expect((diagnostics.at(-1) as { schemas: unknown[] }).schemas).toEqual([]);
  });

  it("draws nothing for a page without widgets and says which widget it cannot draw", () => {
    const { container } = render(
      <I18nextProvider i18n={i18n}>
        <DashboardWidgets project="bbsk" widgets={[]} />
      </I18nextProvider>,
    );
    expect(container).toBeEmptyDOMElement();
    render(
      <I18nextProvider i18n={i18n}>
        <DashboardWidgets project="bbsk" widgets={[{ widget: { widgetType: "map", endpointRef: "bikes" } }]} />
      </I18nextProvider>,
    );
    expect(screen.getByRole("status")).toHaveTextContent("This page has a map widget the Portal does not draw yet.");
  });
});
