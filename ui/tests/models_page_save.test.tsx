import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { RouterProvider, createRootRoute, createRoute, createRouter } from "@tanstack/react-router";
import i18n from "../src/i18n";
import { queryKeys } from "../src/api/client";
import { rememberPrefill, settlePrefill } from "../src/assistant/state";
import en from "../src/locales/en.json";
import { ModelsPage } from "../src/pages/models/ModelsPage";
import { parseModel } from "../src/pages/models/linkml";

const PUBLISHED_LINKML = `id: https://example.org/models/air-quality
name: air-quality
prefixes:
  aq: https://example.org/aq/
default_prefix: aq
imports:
  - linkml:types
classes:
  AirQualityObserved:
    class_uri: https://example.org/aq/AirQualityObserved
    slots:
      - dateObserved
slots:
  dateObserved:
    range: string
    required: true
`;

/** The Propose button, whatever version the edit makes (T-4830). */
const PROPOSE = /^Propose v\d+\.\d+\.\d+$/;

/** The published model with one optional slot more: an edit there is something to propose. */
const EDITED_LINKML = PUBLISHED_LINKML.replace("      - dateObserved\n", "      - dateObserved\n      - pm10\n").concat(
  "  pm10:\n    range: float\n",
);

function renderWithClient(
  ui: React.ReactElement,
  queryClient = new QueryClient({
    defaultOptions: {
      queries: { retry: false },
    },
  }),
) {
  const rootRoute = createRootRoute();
  const home = createRoute({
    getParentRoute: () => rootRoute,
    path: "/",
    component: () => <QueryClientProvider client={queryClient}>{ui}</QueryClientProvider>,
  });
  const approvals = createRoute({
    getParentRoute: () => rootRoute,
    path: "/projects/$project/approvals/$id",
    component: () => <p>approval page</p>,
  });
  const router = createRouter({ routeTree: rootRoute.addChildren([home, approvals]) });
  return render(
    <I18nextProvider i18n={i18n}>
      <RouterProvider router={router} />
    </I18nextProvider>,
  );
}


/** Every call as path+query and method, whichever shape it went out in (a Request, or a URL and init). */
function requests(mock: ReturnType<typeof vi.fn>): { url: string; method: string }[] {
  return mock.mock.calls.map((call: unknown[]) => {
    const [req, init] = call as [RequestInfo | URL, RequestInit | undefined];
    const raw = typeof req === "string" ? req : req instanceof Request ? req.url : req.toString();
    const url = new URL(raw, window.location.origin);
    return {
      url: `${url.pathname}${url.search}`,
      method: init?.method ?? (req instanceof Request ? req.method : "GET"),
    };
  });
}

/** New data model is named in its window (owner 2026-11-01, T-6344); then the empty editor opens. */
async function nameNewModel(user: ReturnType<typeof userEvent.setup>, name: string): Promise<void> {
  const dialog = await screen.findByRole("dialog", { name: en.models.newDialog.title });
  await user.type(within(dialog).getByRole("textbox", { name: en.models.identity.name }), name);
  await user.click(within(dialog).getByRole("button", { name: en.models.newDialog.create }));
}

/** Types come in from Add type: a Smart Data Model or a spreadsheet (owner 2026-11-01, T-6344). */
async function addTypeFrom(user: ReturnType<typeof userEvent.setup>, from: "sdm" | "file"): Promise<void> {
  await user.click(await screen.findByRole("button", { name: en.models.addClass }));
  const dialog = screen.getByRole("dialog", { name: en.models.addClass });
  await user.click(within(dialog).getByRole("button", { name: new RegExp(en.models.addFrom[from]) }));
}

/** A model handed to the page (a file, the assistant) is renamed behind the pencil, as a saved one. */
async function renameTo(user: ReturnType<typeof userEvent.setup>, name: string): Promise<void> {
  await user.click(await screen.findByRole("button", { name: /^Rename/ }));
  const field = screen.getByRole("textbox", { name: en.models.identity.name });
  await user.clear(field);
  await user.type(field, `${name}{Enter}`);
}

describe("ModelsPage save and source loading (DM-56)", () => {
  let originalFetch: typeof global.fetch;

  beforeEach(async () => {
    originalFetch = global.fetch;
    await i18n.changeLanguage("en");
  });

  afterEach(() => {
    // A hand-off replays on its route until the person leaves it; each test starts without one.
    settlePrefill("/elsewhere");
    global.fetch = originalFetch;
    vi.restoreAllMocks();
  });

  it("opening a published model calls GET .../source and the editor shows it", async () => {
    const fetchMock = vi.fn().mockImplementation((req: RequestInfo | URL) => {
      const urlStr = typeof req === "string" ? req : req instanceof Request ? req.url : req.toString();
      if (urlStr.includes("/datamodels/air-quality/source")) {
        return Promise.resolve(new Response(PUBLISHED_LINKML, { status: 200 }));
      }
      if (urlStr.includes("/spaces") || urlStr.includes("/endpoints")) {
        return Promise.resolve(new Response(JSON.stringify({ items: [] }), { status: 200 }));
      }
      return Promise.resolve(new Response("{}", { status: 200 }));
    });
    global.fetch = fetchMock;

    renderWithClient(
      <ModelsPage
        project="ovzdusie"
        baseline={{
          name: "air-quality",
          version: "1.0.0",
          lifecycle: "published",
        }}
      />,
    );

    // Verify GET source called
    await waitFor(() => {
      expect(requests(fetchMock)).toContainEqual({
        url: "/api/v1/projects/ovzdusie/datamodels/air-quality/source",
        method: "GET",
      });
    });

    // Check that editor view loaded the classes
    await waitFor(() => {
      expect(screen.getAllByText("AirQualityObserved").length).toBeGreaterThan(0);
    });
  });

  it("opens when another page already cached the spaces and endpoints lists as the API answers them", async () => {
    const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
    const space = {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "ContextSpace",
      metadata: { name: "air", namespace: "ovzdusie" },
      spec: { dataModelRef: "air-quality" },
    };
    const fetchMock = vi.fn().mockImplementation((req: RequestInfo | URL) => {
      const urlStr = typeof req === "string" ? req : req instanceof Request ? req.url : req.toString();
      if (urlStr.includes("/datamodels/air-quality/source")) {
        return Promise.resolve(new Response(PUBLISHED_LINKML, { status: 200 }));
      }
      if (urlStr.includes("/spaces")) {
        return Promise.resolve(new Response(JSON.stringify(list([space])), { status: 200 }));
      }
      return Promise.resolve(new Response(JSON.stringify(list([])), { status: 200 }));
    });
    global.fetch = fetchMock;

    // The keys are shared: the Spaces or Endpoints page, or the assistant, filled them first.
    const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    queryClient.setQueryData(queryKeys.list("ovzdusie", "spaces"), list([space]));
    queryClient.setQueryData(queryKeys.list("ovzdusie", "endpoints"), list([]));

    renderWithClient(
      <ModelsPage
        project="ovzdusie"
        baseline={{ name: "air-quality", version: "1.0.0", lifecycle: "published" }}
      />,
      queryClient,
    );

    await waitFor(() => {
      expect(screen.getAllByText("AirQualityObserved").length).toBeGreaterThan(0);
    });
    expect(queryClient.getQueryData(queryKeys.list("ovzdusie", "spaces"))).toMatchObject({
      kind: "List",
    });
  });

  it("Save calls PUT with text body and Change notice appears on 202", async () => {
    const user = userEvent.setup();
    const fetchMock = vi.fn().mockImplementation((req: RequestInfo | URL, init?: RequestInit) => {
      const urlStr = typeof req === "string" ? req : req instanceof Request ? req.url : req.toString();
      const method = init?.method ?? (req instanceof Request ? req.method : "GET");

      if (urlStr.includes("/datamodels/air-quality/source") && method === "GET") {
        return Promise.resolve(new Response(PUBLISHED_LINKML, { status: 200 }));
      }
      if (urlStr.includes("/datamodels/air-quality/source") && method === "PUT") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              apiVersion: "joinedcontext.com/v1alpha1",
              kind: "Change",
              metadata: { name: "mr-88", namespace: "ovzdusie" },
              status: {
                phase: "PendingApproval",
                lane: "yellow",
                mergeRequest: "https://forge.example.sk/pulls/88",
                plan: { create: 0, update: 6, delete: 0 },
              },
            }),
            { status: 202, headers: { "Content-Type": "application/json" } },
          ),
        );
      }
      if (urlStr.includes("/spaces") || urlStr.includes("/endpoints")) {
        return Promise.resolve(new Response(JSON.stringify({ items: [] }), { status: 200 }));
      }
      return Promise.resolve(new Response("{}", { status: 200 }));
    });
    global.fetch = fetchMock;

    // An edit, so there is something to propose (T-4830).
    rememberPrefill("/", { source: EDITED_LINKML });
    renderWithClient(
      <ModelsPage
        project="ovzdusie"
        baseline={{
          name: "air-quality",
          version: "1.0.0",
          lifecycle: "published",
        }}
      />,
    );

    const saveBtn = await screen.findByRole("button", { name: PROPOSE });
    await user.click(saveBtn);

    await waitFor(() => {
      expect(requests(fetchMock)).toContainEqual({
        url: "/api/v1/projects/ovzdusie/datamodels/air-quality/source",
        method: "PUT",
      });
    });

    // Check notice rendered
    await waitFor(() => {
      expect(screen.getByText(/mr-88/i)).toBeInTheDocument();
    });
  });

  it("keeps Propose closed on an unedited model, then shows the diff and the version an edit makes (T-4830)", async () => {
    global.fetch = vi.fn().mockImplementation((req: RequestInfo | URL) => {
      const url = typeof req === "string" ? req : req instanceof Request ? req.url : req.toString();
      if (url.includes("/datamodels/air-quality/source")) return Promise.resolve(new Response(PUBLISHED_LINKML, { status: 200 }));
      return Promise.resolve(new Response(JSON.stringify({ items: [] }), { status: 200 }));
    });
    const view = renderWithClient(
      <ModelsPage project="ovzdusie" baseline={{ name: "air-quality", version: "1.0.0", lifecycle: "published" }} />,
    );
    const closed = await screen.findByRole("button", { name: "Propose v1.0.0" });
    await waitFor(() => expect(closed).toHaveAttribute("aria-disabled", "true"));
    expect(closed).toHaveAccessibleDescription(en.models.source.unchanged);
    // The reason is also in the open, not only in a tooltip: a pale button alone read as broken (T-6254).
    expect(screen.getByText(en.models.source.unchanged, { selector: "p" })).toBeVisible();
    view.unmount();

    rememberPrefill("/", { source: EDITED_LINKML });
    renderWithClient(
      <ModelsPage project="ovzdusie" baseline={{ name: "air-quality", version: "1.0.0", lifecycle: "published" }} />,
    );
    const open = await screen.findByRole("button", { name: "Propose v1.1.0" });
    await waitFor(() => expect(open).not.toHaveAttribute("aria-disabled", "true"));
    expect(screen.getByText("1 added · 0 changed · 0 removed · v1.0.0 → v1.1.0")).toBeVisible();
  });

  it("Save shows problem detail on 400 response", async () => {
    const user = userEvent.setup();
    const fetchMock = vi.fn().mockImplementation((req: RequestInfo | URL, init?: RequestInit) => {
      const urlStr = typeof req === "string" ? req : req instanceof Request ? req.url : req.toString();
      const method = init?.method ?? (req instanceof Request ? req.method : "GET");

      if (urlStr.includes("/datamodels/air-quality/source") && method === "GET") {
        return Promise.resolve(new Response(PUBLISHED_LINKML, { status: 200 }));
      }
      if (urlStr.includes("/datamodels/air-quality/source") && method === "PUT") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              type: "https://joinedcontext.com/errors/invalid-request",
              title: "Invalid Request",
              status: 400,
              detail: "a breaking change cannot be saved under version 1.0.0; publish it as 2.0.0",
              errors: ["slot 'pm10' was removed"],
            }),
            { status: 400, headers: { "Content-Type": "application/problem+json" } },
          ),
        );
      }
      if (urlStr.includes("/spaces") || urlStr.includes("/endpoints")) {
        return Promise.resolve(new Response(JSON.stringify({ items: [] }), { status: 200 }));
      }
      return Promise.resolve(new Response("{}", { status: 200 }));
    });
    global.fetch = fetchMock;

    // An edit, so there is something to propose (T-4830).
    rememberPrefill("/", { source: EDITED_LINKML });
    renderWithClient(
      <ModelsPage
        project="ovzdusie"
        baseline={{
          name: "air-quality",
          version: "1.0.0",
          lifecycle: "published",
        }}
      />,
    );

    const saveBtn = await screen.findByRole("button", { name: PROPOSE });
    await user.click(saveBtn);

    await waitFor(() => {
      expect(
        screen.getByText(/a breaking change cannot be saved under version 1.0.0/i),
      ).toBeInTheDocument();
    });
  });

  it("keeps Save closed while the model has a broken relationship, and says why (DM-68, T-2736)", async () => {
    const user = userEvent.setup();
    const broken = `${PUBLISHED_LINKML}  station:
    range: AirQualityObserved
    annotations:
      ngsi_ld_kind: Relationship
`.replace("      - dateObserved\n", "      - dateObserved\n      - station\n");
    const fetchMock = vi.fn().mockImplementation((req: RequestInfo | URL, init?: RequestInit) => {
      const urlStr = typeof req === "string" ? req : req instanceof Request ? req.url : req.toString();
      const method = init?.method ?? (req instanceof Request ? req.method : "GET");
      if (urlStr.includes("/datamodels/air-quality/source") && method === "GET") {
        return Promise.resolve(new Response(broken, { status: 200 }));
      }
      if (urlStr.includes("/spaces") || urlStr.includes("/endpoints")) {
        return Promise.resolve(new Response(JSON.stringify({ items: [] }), { status: 200 }));
      }
      return Promise.resolve(new Response("{}", { status: 200 }));
    });
    global.fetch = fetchMock;

    renderWithClient(
      <ModelsPage project="ovzdusie" baseline={{ name: "air-quality", version: "1.0.0", lifecycle: "published" }} />,
    );

    const saveBtn = await screen.findByRole("button", { name: PROPOSE });
    await waitFor(() => expect(saveBtn).toHaveAttribute("aria-disabled", "true"));
    expect(saveBtn).toHaveAccessibleDescription(/fix the errors the editor lists before saving\. left: 1\./i);
    expect(screen.getByText(/fix the errors the editor lists before saving\. left: 1\./i, { selector: "p" })).toBeVisible();
    await user.click(saveBtn);
    expect(requests(fetchMock).filter((call) => call.method === "PUT")).toEqual([]);
  });

  it("shows each broken rule the server names beside its detail (DM-68, T-2736)", async () => {
    const user = userEvent.setup();
    global.fetch = vi.fn().mockImplementation((req: RequestInfo | URL, init?: RequestInit) => {
      const urlStr = typeof req === "string" ? req : req instanceof Request ? req.url : req.toString();
      const method = init?.method ?? (req instanceof Request ? req.method : "GET");
      if (urlStr.includes("/datamodels/air-quality/source") && method === "GET") {
        return Promise.resolve(new Response(PUBLISHED_LINKML, { status: 200 }));
      }
      if (urlStr.includes("/datamodels/air-quality/source") && method === "PUT") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              type: "https://joinedcontext.com/errors/invalid-request",
              title: "Invalid Request",
              status: 400,
              detail: "the model breaks 1 relationship rule; nothing was saved (DM-68)",
              errors: ["slots.station: inverse-missing: station (Reading → Station) names no inverse"],
            }),
            { status: 400, headers: { "Content-Type": "application/problem+json" } },
          ),
        );
      }
      if (urlStr.includes("/spaces") || urlStr.includes("/endpoints")) {
        return Promise.resolve(new Response(JSON.stringify({ items: [] }), { status: 200 }));
      }
      return Promise.resolve(new Response("{}", { status: 200 }));
    });

    // An edit, so there is something to propose (T-4830).
    rememberPrefill("/", { source: EDITED_LINKML });
    renderWithClient(
      <ModelsPage project="ovzdusie" baseline={{ name: "air-quality", version: "1.0.0", lifecycle: "published" }} />,
    );
    await user.click(await screen.findByRole("button", { name: PROPOSE }));
    expect(
      await screen.findByText(
        "the model breaks 1 relationship rule; nothing was saved (DM-68): slots.station: inverse-missing: station (Reading → Station) names no inverse",
      ),
    ).toBeInTheDocument();
  });

  describe("a model the assistant changed (T-0738, AG-77)", () => {
    afterEach(() => {
      window.history.pushState({}, "", "/");
    });

    /** The published model, its manifest in the list, and every PUT the page sends. */
    function forge() {
      const puts: string[] = [];
      global.fetch = vi.fn().mockImplementation(async (req: RequestInfo | URL, init?: RequestInit) => {
        const url = typeof req === "string" ? req : req instanceof Request ? req.url : req.toString();
        const method = init?.method ?? (req instanceof Request ? req.method : "GET");
        if (url.includes("/datamodels/air-quality/source") && method === "PUT") {
          puts.push(init?.body ? String(init.body) : await (req as Request).clone().text());
          return Promise.resolve(new Response(JSON.stringify({ kind: "Change", metadata: { name: "mr-9" } }), { status: 202 }));
        }
        if (url.includes("/datamodels/air-quality/source")) {
          return Promise.resolve(new Response(PUBLISHED_LINKML, { status: 200 }));
        }
        if (url.includes("/datamodels")) {
          const model = {
            apiVersion: "joinedcontext.com/v1alpha1",
            kind: "DataModel",
            metadata: { name: "air-quality", namespace: "ovzdusie" },
            spec: { version: "1.0.0", lifecycle: "published" },
          };
          return Promise.resolve(new Response(JSON.stringify({ kind: "List", items: [model] }), { status: 200 }));
        }
        return Promise.resolve(new Response(JSON.stringify({ kind: "List", items: [] }), { status: 200 }));
      });
      return puts;
    }

    it("opens with the operations applied, and a removal saves only once the breaking change is confirmed", async () => {
      const user = userEvent.setup();
      const puts = forge();
      rememberPrefill("/", { operations: [{ op: "removeSlot", name: "dateObserved" }] });
      window.history.pushState({}, "", "/?edit=air-quality");

      renderWithClient(<ModelsPage project="ovzdusie" />);

      expect(
        await screen.findByRole("heading", { name: `${en.models.severity.breaking} · ${en.lane.red}` }),
      ).toBeInTheDocument();
      expect(screen.getByText("the slot was removed; a renamed slot is a removal and an addition", { exact: false })).toBeInTheDocument();
      const save = screen.getByRole("button", { name: PROPOSE });
      // Closed with its reason (T-6254): aria-disabled, the reason in the open.
      expect(save).toHaveAttribute("aria-disabled", "true");
      expect(save).toHaveAccessibleDescription(en.models.source.confirmFirst.replace("{version}", "2.0.0"));

      await user.click(screen.getByRole("checkbox", { name: "Save it as version 2.0.0, a breaking change" }));
      expect(save).not.toHaveAttribute("aria-disabled");
      await user.click(save);
      await waitFor(() => expect(puts).toHaveLength(1));
      expect(puts[0]).not.toContain("dateObserved");
      expect(puts[0]).toContain("class_uri: https://example.org/aq/AirQualityObserved");
    });

    it("says which operation the model refused, and keeps the source as it is", async () => {
      forge();
      rememberPrefill("/", { operations: [{ op: "removeSlot", name: "pm25" }] });
      window.history.pushState({}, "", "/?edit=air-quality");

      renderWithClient(<ModelsPage project="ovzdusie" />);

      const alert = await screen.findByRole("alert");
      expect(alert).toHaveTextContent(en.models.source.handOffRefused);
      expect(alert).toHaveTextContent("unknown slot 'pm25'");
      expect(screen.getByRole("button", { name: PROPOSE })).toBeEnabled();
    });
  });

  it("names a new model and creates it as a project model, naming no space (T-0789, DM-57, T-6162)", async () => {
    const user = userEvent.setup();
    const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
    const space = {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "ContextSpace",
      metadata: { name: "mobility", namespace: "ovzdusie" },
      spec: {},
    };
    // The model the project already has lives in a space of its own: one model per space (DM-61).
    const air = { ...space, metadata: { name: "air", namespace: "ovzdusie" } };
    const taken = {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "DataModel",
      metadata: { name: "air-quality", namespace: "ovzdusie" },
      spec: { contextSpaceRef: "air", linkml: "./air-quality.linkml.yaml" },
    };
    const fetchMock = vi.fn().mockImplementation((req: RequestInfo | URL, init?: RequestInit) => {
      const urlStr = typeof req === "string" ? req : req instanceof Request ? req.url : req.toString();
      const method = init?.method ?? (req instanceof Request ? req.method : "GET");
      if (urlStr.includes("/datamodels/bikes/source") && method === "PUT") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              apiVersion: "joinedcontext.com/v1alpha1",
              kind: "Change",
              metadata: { name: "mr-91", namespace: "ovzdusie" },
              status: {
                phase: "PendingApproval",
                lane: "green",
                mergeRequest: "https://forge.example.sk/pulls/91",
                plan: { create: 6, update: 0, delete: 0 },
              },
            }),
            { status: 202, headers: { "Content-Type": "application/json" } },
          ),
        );
      }
      if (urlStr.includes("/spaces")) {
        return Promise.resolve(new Response(JSON.stringify(list([space, air])), { status: 200 }));
      }
      if (urlStr.includes("/datamodels")) {
        return Promise.resolve(new Response(JSON.stringify(list([taken])), { status: 200 }));
      }
      return Promise.resolve(new Response(JSON.stringify(list([])), { status: 200 }));
    });
    global.fetch = fetchMock;

    renderWithClient(<ModelsPage project="ovzdusie" />);

    // A blank draft has no manifest, so nothing can be saved until it is named and placed.
    expect(screen.queryByRole("button", { name: PROPOSE })).not.toBeInTheDocument();

    // A new model opens on the New data model window (owner 2026-11-01, T-6344).
    const dialog = await screen.findByRole("dialog", { name: en.models.newDialog.title });
    const name = within(dialog).getByRole("textbox", { name: en.models.identity.name });
    await user.type(name, "air-quality");
    // The window names no space (T-6162): which spaces use a model stays on the space pages.
    expect(within(dialog).queryByRole("combobox", { name: /space/i })).not.toBeInTheDocument();

    // A name the project already carries would fork a second model under one name: said in the window.
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(/already has a model named/i);
    expect(within(dialog).getByRole("button", { name: en.models.newDialog.create })).toHaveAttribute("aria-disabled", "true");

    await user.clear(name);
    await user.type(name, "Bikes");
    await user.click(within(dialog).getByRole("button", { name: en.models.newDialog.create }));
    await user.click(await screen.findByRole("button", { name: PROPOSE }));

    await waitFor(() => {
      expect(requests(fetchMock)).toContainEqual({
        url: "/api/v1/projects/ovzdusie/datamodels/bikes/source",
        method: "PUT",
      });
    });
    expect(await screen.findByText(/mr-91/i)).toBeInTheDocument();
  });

  it("refuses Check and Save where they stand for a person who may not propose a DataModel (T-2848, UI-44)", async () => {
    const user = userEvent.setup();
    const reader = { project: "ovzdusie", bootstrap: false, grants: [{ rule: { kinds: ["*"], verbs: ["read"] } }] };
    const fetchMock = vi.fn().mockImplementation((req: RequestInfo | URL) => {
      const urlStr = typeof req === "string" ? req : req instanceof Request ? req.url : req.toString();
      if (urlStr.includes("/permissions/me")) {
        return Promise.resolve(new Response(JSON.stringify(reader), { status: 200, headers: { "Content-Type": "application/json" } }));
      }
      if (urlStr.includes("/datamodels/air-quality/source")) {
        return Promise.resolve(new Response(PUBLISHED_LINKML, { status: 200 }));
      }
      return Promise.resolve(new Response(JSON.stringify({ items: [] }), { status: 200, headers: { "Content-Type": "application/json" } }));
    });
    global.fetch = fetchMock;

    renderWithClient(<ModelsPage project="ovzdusie" baseline={{ name: "air-quality", version: "1.0.0", lifecycle: "published" }} />);

    const reason = en.permissions.denied.replaceAll("''", "'").replace("{verb}", "propose").replace("{kind}", "DataModel");
    for (const name of [en.models.source.saveCheck, PROPOSE]) {
      const button = await screen.findByRole("button", { name });
      await waitFor(() => expect(button).toHaveAttribute("aria-disabled", "true"));
      expect(button).toHaveAccessibleDescription(reason);
      await user.click(button);
    }
    const writes = requests(fetchMock).filter((r) => r.method !== "GET");
    expect(writes).toEqual([]);
  });

  it("creates a Smart Data Model imported into an empty editor as a new model in the space picked (T-2847)", async () => {
    const user = userEvent.setup();
    const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
    const air = { apiVersion: "joinedcontext.com/v1alpha1", kind: "ContextSpace", metadata: { name: "air" }, spec: {} };
    const catalogue = {
      refreshedAt: "2026-09-06T04:00:00Z",
      stale: false,
      subjects: [
        {
          name: "dataModel.Environment",
          title: "Environment",
          models: [
            {
              id: "dataModel.Environment/AirQualityObserved",
              name: "AirQualityObserved",
              description: "An observation of air quality conditions.",
              attributes: ["dateObserved"],
            },
          ],
        },
      ],
    };
    const fetchMock = vi.fn().mockImplementation((req: RequestInfo | URL, init?: RequestInit) => {
      const urlStr = typeof req === "string" ? req : req instanceof Request ? req.url : req.toString();
      const method = init?.method ?? (req instanceof Request ? req.method : "GET");
      const json = (body: unknown, status = 200) =>
        Promise.resolve(new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } }));
      if (urlStr.includes("/tools/sdm-catalog")) return json(catalogue);
      if (urlStr.includes("/tools/import-sdm")) return json({ linkml: PUBLISHED_LINKML.replace("name: air-quality", "name: AirQualityObserved") });
      if (urlStr.includes("/datamodels/") && method === "PUT") {
        return json(
          {
            apiVersion: "joinedcontext.com/v1alpha1",
            kind: "Change",
            metadata: { name: "mr-92", namespace: "ovzdusie" },
            status: { phase: "PendingApproval", lane: "green", plan: { create: 2, update: 0, delete: 0 } },
          },
          202,
        );
      }
      if (urlStr.includes("/datamodels/")) return json({ status: 404, title: "Not Found" }, 404);
      if (urlStr.includes("/spaces")) return json(list([air]));
      return json(list([]));
    });
    global.fetch = fetchMock;
    window.history.replaceState(null, "", "/?new=sdm");
    try {
      renderWithClient(<ModelsPage project="ovzdusie" />);
      // The model keeps the name typed for it; the Smart Data Model brings a type (owner 2026-11-01).
      await nameNewModel(user, "Air quality");
      await addTypeFrom(user, "sdm");
      await user.click(await screen.findByRole("button", { name: /AirQualityObserved/ }));
      await user.click(screen.getByRole("button", { name: "Import AirQualityObserved" }));

      expect(await screen.findByRole("button", { name: "AirQualityObserved" })).toBeInTheDocument();
      expect(screen.getByText("air-quality.linkml.yaml")).toBeInTheDocument();

      await user.click(screen.getByRole("button", { name: PROPOSE }));
      await waitFor(() => {
        expect(requests(fetchMock)).toContainEqual({
          url: "/api/v1/projects/ovzdusie/datamodels/air-quality/source",
          method: "PUT",
        });
      });
      // Nothing read or wrote a model the project does not have.
      expect(requests(fetchMock).filter((r) => r.url.includes("/datamodels/AirQualityObserved"))).toEqual([]);
    } finally {
      window.history.replaceState(null, "", "/");
    }
  });

  it("refuses a second model for a space that has one, even when the address names the space (DM-61, T-2765)", async () => {
    const user = userEvent.setup();
    const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
    const air = { apiVersion: "joinedcontext.com/v1alpha1", kind: "ContextSpace", metadata: { name: "air" }, spec: {} };
    const taken = {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "DataModel",
      metadata: { name: "air-quality" },
      spec: { contextSpaceRef: "air", linkml: "./air-quality.linkml.yaml" },
    };
    global.fetch = vi.fn().mockImplementation((req: RequestInfo | URL) => {
      const urlStr = typeof req === "string" ? req : req instanceof Request ? req.url : req.toString();
      const body = urlStr.includes("/spaces") ? list([air]) : urlStr.includes("/datamodels") ? list([taken]) : list([]);
      return Promise.resolve(new Response(JSON.stringify(body), { status: 200 }));
    });
    // The route this test mounts is `/`; the page reads the address's query either way.
    window.history.replaceState(null, "", "/?new=blank&space=air");
    try {
      renderWithClient(<ModelsPage project="ovzdusie" />);
      const dialog = await screen.findByRole("dialog", { name: en.models.newDialog.title });
      await user.type(within(dialog).getByRole("textbox", { name: en.models.identity.name }), "second");
      expect(within(dialog).getByRole("button", { name: en.models.newDialog.create })).toHaveAttribute("aria-disabled", "true");
      expect(await within(dialog).findByRole("alert")).toHaveTextContent(
        en.models.create.spaceTaken.replaceAll("{space}", "air").replaceAll("{model}", "air-quality"),
      );
      expect(screen.queryByRole("button", { name: PROPOSE })).not.toBeInTheDocument();
    } finally {
      window.history.replaceState(null, "", "/");
    }
  });

  /**
   * A model inferred from a file used to live in this component alone: a reload, a crash or the
   * assistant navigating away lost it silently. It is kept in the shared draft store instead
   * (T-1029, AG-61, DM-57).
   */
  describe("an unpublished model survives a reload", () => {
    const INFERRED = "name: sensors\nclasses:\n  Sensors: {}\n";

    it("keeps the inferred model as a draft and names it in the URL", async () => {
      const user = userEvent.setup();
      const drafts: { url: string; body: unknown }[] = [];
      const fetchMock = vi.fn().mockImplementation(async (req: RequestInfo | URL, init?: RequestInit) => {
        const urlStr = typeof req === "string" ? req : req instanceof Request ? req.url : req.toString();
        const method = init?.method ?? (req instanceof Request ? req.method : "GET");
        if (urlStr.includes("/drafts/DataModel/") && method === "PUT") {
          const text = init?.body ? String(init.body) : await (req as Request).clone().text();
          drafts.push({ url: urlStr, body: JSON.parse(text) });
          return Promise.resolve(
            new Response(
              JSON.stringify({
                project: "ovzdusie",
                kind: "DataModel",
                name: "sensors",
                manifest: {},
                touchedBy: "demo.steward",
                touchedKind: "person",
                version: 1,
                updatedAt: "2026-09-17T10:00:00Z",
              }),
              { status: 200, headers: { "Content-Type": "application/json" } },
            ),
          );
        }
        return Promise.resolve(
          new Response(JSON.stringify({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] }), {
            status: 200,
          }),
        );
      });
      global.fetch = fetchMock;

      // The dock hands the page an inferred model, as the file drop does.
      window.history.replaceState(null, "", "/");
      rememberPrefill("/", { source: INFERRED });
      renderWithClient(<ModelsPage project="ovzdusie" />);

      await renameTo(user, "sensors");

      await waitFor(() => {
        expect(drafts).toHaveLength(1);
      });
      expect(drafts[0].url).toContain("/api/v1/projects/ovzdusie/drafts/DataModel/sensors");
      // The draft is the inferred model, titled with the name typed for it (T-6162).
      const kept = drafts[0].body as { manifest: { kind: string; spec: { linkml: string } } };
      expect(kept.manifest.kind).toBe("DataModel");
      expect(parseModel(kept.manifest.spec.linkml)).toEqual({ ...parseModel(INFERRED), title: { en: "sensors" } });
      expect(new URL(window.location.href).searchParams.get("draft")).toBe("sensors");
      expect(await screen.findByText(en.models.create.held.replace("{name}", "sensors"))).toBeInTheDocument();
    });

    it("stops keeping and says so when somebody else saved the draft first", async () => {
      const user = userEvent.setup();
      let puts = 0;
      const fetchMock = vi.fn().mockImplementation(async (req: RequestInfo | URL, init?: RequestInit) => {
        const urlStr = typeof req === "string" ? req : req instanceof Request ? req.url : req.toString();
        const method = init?.method ?? (req instanceof Request ? req.method : "GET");
        if (urlStr.includes("/drafts/DataModel/") && method === "PUT") {
          puts += 1;
          return new Response(JSON.stringify({ current: 7 }), {
            status: 409,
            headers: { "Content-Type": "application/json" },
          });
        }
        return new Response(JSON.stringify({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] }), {
          status: 200,
        });
      });
      global.fetch = fetchMock;
      window.history.replaceState(null, "", "/");
      rememberPrefill("/", { source: INFERRED });

      renderWithClient(<ModelsPage project="ovzdusie" />);

      await renameTo(user, "sensors");

      expect(await screen.findByText(en.models.create.heldConflict)).toBeInTheDocument();
      // Nothing of theirs is overwritten: the page does not try again on the next rename.
      const sofar = puts;
      await renameTo(user, "sensors-2");
      await new Promise((resolve) => setTimeout(resolve, 900));
      expect(puts).toBe(sofar);
    });

    it("keeps an imported Smart Data Model too (T-1105)", async () => {
      const user = userEvent.setup();
      const drafts: string[] = [];
      const catalogue = {
        refreshedAt: "2026-09-06T04:00:00Z",
        stale: false,
        subjects: [
          {
            name: "dataModel.Transportation",
            title: "Transportation",
            models: [{ id: "dataModel.Transportation/Vehicle", name: "Vehicle", attributes: ["speed"] }],
          },
        ],
      };
      const imported = "id: https://smartdatamodels.org/Vehicle\nname: Vehicle\nclasses:\n  Vehicle:\n    slots: [speed]\nslots:\n  speed:\n    range: float\n";
      const fetchMock = vi.fn().mockImplementation(async (req: RequestInfo | URL, init?: RequestInit) => {
        const urlStr = typeof req === "string" ? req : req instanceof Request ? req.url : req.toString();
        const method = init?.method ?? (req instanceof Request ? req.method : "GET");
        if (urlStr.includes("/drafts/DataModel/") && method === "PUT") {
          drafts.push(urlStr);
          return new Response(
            JSON.stringify({
              project: "ovzdusie",
              kind: "DataModel",
              name: "Vehicle",
              manifest: {},
              touchedBy: "demo.steward",
              touchedKind: "person",
              version: 1,
              updatedAt: "2026-09-17T10:00:00Z",
            }),
            { status: 200, headers: { "Content-Type": "application/json" } },
          );
        }
        if (urlStr.includes("/tools/sdm-catalog")) {
          return new Response(JSON.stringify(catalogue), {
            status: 200,
            headers: { "Content-Type": "application/json" },
          });
        }
        if (urlStr.includes("/tools/import-sdm")) {
          return new Response(JSON.stringify({ linkml: imported }), {
            status: 200,
            headers: { "Content-Type": "application/json" },
          });
        }
        return new Response(JSON.stringify({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] }), {
          status: 200,
          headers: { "Content-Type": "application/json" },
        });
      });
      global.fetch = fetchMock;
      window.history.replaceState(null, "", "/");
      settlePrefill("/elsewhere");

      renderWithClient(<ModelsPage project="ovzdusie" />);

      await nameNewModel(user, "Vehicle");
      await addTypeFrom(user, "sdm");
      await user.click(await screen.findByRole("button", { name: /Vehicle/ }));
      await user.click(await screen.findByRole("button", { name: "Import Vehicle" }));

      await waitFor(() => {
        expect(drafts).toHaveLength(1);
      });
      // Kept under the manifest name Save would create, not the upstream class name (T-2847).
      expect(drafts[0]).toContain("/drafts/DataModel/vehicle");
    });

    it("opens the draft the URL names, with its source and its space", async () => {
      const fetchMock = vi.fn().mockImplementation((req: RequestInfo | URL) => {
        const urlStr = typeof req === "string" ? req : req instanceof Request ? req.url : req.toString();
        if (urlStr.includes("/drafts/DataModel/sensors")) {
          return Promise.resolve(
            new Response(
              JSON.stringify({
                project: "ovzdusie",
                kind: "DataModel",
                name: "sensors",
                manifest: { kind: "DataModel", spec: { linkml: INFERRED, space: "mobility" } },
                touchedBy: "demo.steward",
                touchedKind: "person",
                version: 3,
                updatedAt: "2026-09-17T10:00:00Z",
              }),
              { status: 200, headers: { "Content-Type": "application/json" } },
            ),
          );
        }
        return Promise.resolve(
          new Response(JSON.stringify({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] }), {
            status: 200,
          }),
        );
      });
      global.fetch = fetchMock;
      window.history.replaceState(null, "", "/?draft=sensors");
      // Nothing was handed to this page: it opens on what the draft store holds.
      settlePrefill("/elsewhere");

      renderWithClient(<ModelsPage project="ovzdusie" />);

      // The editor opens on the model that was in hand, not on the import tab.
      expect(screen.queryByRole("dialog", { name: en.models.newDialog.title })).toBeNull();
      expect((await screen.findAllByText("sensors")).length).toBeGreaterThan(0);
      await waitFor(() => {
        expect(screen.getAllByText("Sensors").length).toBeGreaterThan(0);
      });
    });
  });
});
