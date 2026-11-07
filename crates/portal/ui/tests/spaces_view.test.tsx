import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";
import { digestOf } from "../src/api/drafts";
import { answeringChecks, checksSoFar, expectDenied, expectOpen } from "./checks";
import { findFormPage } from "./formPage";

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  email: "jana.kovacova@banskabystrica.sk",
  roles: ["portal-editor"],
};

const SPACES = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items: [
    {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "ContextSpace",
      metadata: {
        name: "ovzdusie",
        namespace: "banskabystrica",
        title: { sk: "Ovzdušie", en: "Air quality" },
      },
      spec: { dataModelRef: "AirQualityObserved" },
      status: {
        phase: "Live",
        observedRevision: "9c1f0ab",
        sourceUrl:
          "https://git.example.sk/bb/org/src/branch/main/projects/banskabystrica/spaces/ovzdusie/space.yaml",
      },
    },
    {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "ContextSpace",
      metadata: { name: "doprava", namespace: "banskabystrica" },
      // The reference as "Complete this space" writes it: `{ kind, name }` (MF-07).
      spec: { isSandbox: true, ttlDays: 7, dataModelRef: { kind: "DataModel", name: "citybikes" } },
      status: { phase: "Deploying" },
    },
  ],
};

const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-1a2b3c4d", namespace: "banskabystrica" },
  status: { lane: "green", phase: "PendingApproval", plan: { create: 1 } },
};

/** The project as `GET /api/v1/projects/{project}` answers it: the manifest and its usage. */
function project(quota?: number) {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Project",
    metadata: { name: "banskabystrica", namespace: "org" },
    spec: { organizationRef: "banskabystrica.sk" },
    status: {
      usage: {
        apps: { used: 0 },
        contextSpaces: { used: 2, ...(quota === undefined ? {} : { limit: quota }) },
        publicEndpoints: { used: 1, limit: 4 },
        residentPipelines: { used: 0, limit: 3 },
      },
    },
  };
}

/** A draft of a new space as the assistant leaves it behind (AG-45): what `?draft=` loads. */
const ASSISTANT_DRAFT = {
  project: "banskabystrica",
  kind: "ContextSpace",
  name: "kvalita-vzduchu",
  manifest: {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "ContextSpace",
    metadata: {
      name: "kvalita-vzduchu",
      namespace: "banskabystrica",
      title: { en: "Air quality, the new space" },
    },
    spec: { defaultLocale: "en", isSandbox: false },
  },
  verdict: {
    ok: true,
    findings: [],
    checkedAt: "2026-09-19T00:00:00Z",
    inputDigest: "sha256:0",
  },
  touchedBy: "jana.kovacova",
  touchedKind: "assistant",
  version: 1,
  updatedAt: "2026-09-19T00:00:00Z",
};

function renderSpaces(
  options: { quota?: number; refusal?: string; listFails?: string; draft?: unknown } = {
    quota: 3,
  },
) {
  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    const request = input as Request;
    const path = new URL(request.url).pathname;
    const json = (body: unknown, status = 200) =>
      Promise.resolve(
        new Response(JSON.stringify(body), {
          status,
          headers: { "Content-Type": "application/json" },
        }),
      );

    if (path.endsWith("/auth/me")) {
      return json(IDENTITY);
    }
    if (request.method === "POST") {
      return options.refusal
        ? json({ status: 403, title: "Forbidden", detail: options.refusal }, 403)
        : json(CHANGE, 202);
    }
    if (path === "/api/v1/projects/banskabystrica") {
      return json(project(options.quota));
    }
    if (path.includes("/projects/banskabystrica/projects")) {
      return json({ status: 404, title: "Resource Not Found" }, 404);
    }
    // A draft save is taken, as the server takes one; the write under test is the proposal.
    if (request.method === "PUT" && path.includes("/drafts/ContextSpace/")) {
      return json({ version: 1, manifest: null, verdict: null, touchedBy: "anna", touchedKind: "person" });
    }
    if (path.includes("/drafts/ContextSpace/")) {
      return options.draft
        ? json(options.draft)
        : json({ status: 404, title: "Not Found" }, 404);
    }
    if (path.endsWith("/spaces/ovzdusie")) {
      return json(SPACES.items[0]);
    }
    if (path.endsWith("/datamodels")) {
      return json({
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "List",
        items: [
          {
            apiVersion: "joinedcontext.com/v1alpha1",
            kind: "DataModel",
            metadata: { name: "ovzdusie-extra", namespace: "banskabystrica" },
            spec: { contextSpaceRef: { kind: "ContextSpace", name: "ovzdusie" } },
          },
        ],
      });
    }
    if (path.endsWith("/spaces")) {
      return options.listFails
        ? json({ status: 503, title: "Service Unavailable", detail: options.listFails }, 503)
        : json(SPACES);
    }
    return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
  });
  vi.stubGlobal("fetch", fetchMock);
  vi.stubGlobal("fetch", answeringChecks(globalThis.fetch));

  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <App />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return fetchMock;
}

function posts(fetchMock: ReturnType<typeof vi.fn>): Request[] {
  return fetchMock.mock.calls
    .map((call) => call[0] as Request)
    .filter((request) => request.method === "POST");
}

describe("context spaces view", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/projects/banskabystrica/spaces");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("lists every space with its title, data model and lifecycle state", async () => {
    renderSpaces();

    const row = (await screen.findByText("ovzdusie")).closest("tr") as HTMLElement;
    expect(within(row).getByText("Air quality")).toBeInTheDocument();
    // The pointer first, then every model that names the space, as its own page lists them (T-2760).
    // Each model is a link to its own page (not one mono string).
    expect(await within(row).findByRole("link", { name: "ovzdusie-extra" })).toHaveAttribute(
      "href",
      "/projects/banskabystrica/models/ovzdusie-extra",
    );
    expect(within(row).getByRole("link", { name: "AirQualityObserved" })).toBeInTheDocument();
    expect(within(row).getByText(en.phase.live)).toBeInTheDocument();

    const sandbox = (await screen.findByText(/sandbox, 7 days/)).closest("tr") as HTMLElement;
    expect(within(sandbox).getByText("doprava")).toBeInTheDocument();
    expect(within(sandbox).getByText("citybikes")).toBeInTheDocument();
    expect(within(sandbox).getByText(en.phase.deploying)).toBeInTheDocument();
  });

  /// T-0961, UI-01: a list that cannot be read says so where the list would be, in the API's
  /// own words, with the way back. It does not leave a person watching an empty table.
  it("puts the API's own reason where the table would be, with a retry", async () => {
    const fetchMock = renderSpaces({ listFails: "the mirror is still loading" });

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("the mirror is still loading");
    expect(screen.queryByRole("table")).toBeNull();
    expect(screen.queryByText("ovzdusie")).toBeNull();

    const before = fetchMock.mock.calls.length;
    await userEvent.click(within(alert).getByRole("button", { name: en.app.error.retry }));
    await waitFor(() => {
      expect(fetchMock.mock.calls.length).toBeGreaterThan(before);
    });
  });

  it("polls the list while a space is still deploying (T-1392)", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      const fetchMock = renderSpaces();
      await screen.findByText("doprava");
      const lists = () =>
        fetchMock.mock.calls.filter(([input]) => new URL((input as Request).url).pathname.endsWith("/spaces")).length;
      const before = lists();
      await vi.advanceTimersByTimeAsync(10_000);
      await waitFor(() => expect(lists()).toBeGreaterThan(before));
    } finally {
      vi.useRealTimers();
    }
  });

  it("links each mirrored space to its manifest in the forge", async () => {
    renderSpaces();
    const row = (await screen.findByText("ovzdusie")).closest("tr") as HTMLElement;
    // An item of the row's menu, not a Source column (T-6095).
    await userEvent.click(within(row).getByRole("button", { name: /More actions/ }));
    const item = await screen.findByRole("menuitem", { name: en.resourceList.viewSource });
    expect(item.closest("a")).toHaveAttribute("href", SPACES.items[0].status.sourceUrl);
  });

  it("shows every quota of the project with what it holds of it", async () => {
    renderSpaces({ quota: 3 });

    const bar = await screen.findByRole("progressbar", { name: en.quota.dimension.contextSpaces });
    expect(bar).toHaveAttribute("aria-valuenow", "2");
    expect(bar).toHaveAttribute("aria-valuemax", "3");
    expect(screen.getByText("2 of 3")).toBeInTheDocument();
    // Every dimension, not only the one this page creates; a dimension without a limit says so.
    expect(screen.getByText("1 of 4")).toBeInTheDocument();
    expect(screen.getByText(en.quota.dimension.residentPipelines)).toBeInTheDocument();
    expect(screen.getByText("0, no limit")).toBeInTheDocument();
  });

  it("puts the quota after the list, empty or not (T-1396)", async () => {
    renderSpaces({ quota: 3 });

    const bar = await screen.findByRole("progressbar", { name: en.quota.dimension.contextSpaces });
    const table = screen.getByRole("table");
    // DOCUMENT_POSITION_FOLLOWING: the bar comes after the table in reading order.
    expect(table.compareDocumentPosition(bar) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it("reads the numbers from the project route, which knows the quota in force", async () => {
    const fetchMock = renderSpaces({ quota: 3 });
    await screen.findByRole("progressbar", { name: en.quota.dimension.contextSpaces });
    const paths = fetchMock.mock.calls.map((call) => new URL((call[0] as Request).url).pathname);
    expect(paths).toContain("/api/v1/projects/banskabystrica");
    // The page no longer reads the Project manifest itself, so it cannot miss the
    // organization's default (PF-73).
    expect(paths).not.toContain("/api/v1/projects/org/projects");
  });

  it("blocks a new space once the quota is used up", async () => {
    renderSpaces({ quota: 2 });

    // A quota, not a permission — and the refusal travels with the control (UI-44, T-1795):
    // `aria-disabled` rather than `disabled`, so somebody who cannot see the button greyed out
    // still reaches it in the tab order and is told the reason, which `disabled` would have
    // taken out of the page along with the button.
    const add = await screen.findByRole("button", { name: en.spaces.add });
    await waitFor(() => expect(add).toHaveAttribute("aria-disabled", "true"));
    expect(add, "reachable, so the reason can be read").not.toBeDisabled();
    expect(add).toHaveAccessibleDescription(/quota of 2 Context Spaces is used up/);
    // Said twice on purpose, and the two are not the same thing: the button's own description,
    // which only a screen reader reaches, and the banner above the list, which everyone reads.
    const said = screen.getAllByText(/quota of 2 Context Spaces is used up/);
    expect(said.some((node) => !node.className.includes("sr-only")), "a visible banner too").toBe(
      true,
    );
  });

  // T-2758: `/spaces/new` or a draft opens the form without the disabled button, and a full
  // project used to take a whole filled form before the server said no.
  it("says the project is full inside a form opened by its address", async () => {
    window.history.pushState({}, "", "/projects/banskabystrica/spaces/new");
    renderSpaces({ quota: 2 });

    const dialog = await findFormPage();
    const said = await within(dialog).findAllByText(/quota of 2 Context Spaces is used up/);
    expect(said.some((node) => !node.className.includes("sr-only")), "a visible warning").toBe(true);
    const propose = within(dialog).getByRole("button", { name: en.spaces.propose });
    await waitFor(() => expect(propose).toHaveAttribute("aria-disabled", "true"));
  });

  it("warns of nothing inside the form while the project has room", async () => {
    window.history.pushState({}, "", "/projects/banskabystrica/spaces/new");
    renderSpaces({ quota: 3 });

    const dialog = await findFormPage();
    await within(dialog).findByLabelText(/Name/);
    expect(within(dialog).queryByText(/Context Spaces is used up/)).toBeNull();
  });

  it("proposes a change instead of writing the space directly", async () => {
    const fetchMock = renderSpaces();

    await userEvent.click(await screen.findByRole("button", { name: en.spaces.add }));
    const dialog = await findFormPage();
    await userEvent.type(within(dialog).getByLabelText(/Name/), "mobilita");
    await userEvent.click(within(dialog).getByRole("button", { name: en.form.check }));
    await waitFor(() => expectOpen(within(dialog).getByRole("button", { name: en.spaces.propose })));
    await userEvent.click(within(dialog).getByRole("button", { name: en.spaces.propose }));

    await waitFor(() => expect(posts(fetchMock)).toHaveLength(1));
    const request = posts(fetchMock)[0];
    expect(new URL(request.url).pathname).toBe("/api/v1/projects/banskabystrica/spaces");
    await expect(request.clone().json()).resolves.toMatchObject({
      kind: "ContextSpace",
      metadata: { name: "mobilita", namespace: "banskabystrica" },
    });

    expect(await screen.findByText("chg-1a2b3c4d")).toBeInTheDocument();
    expect(
      screen.getByRole("link", { name: en.changes.review }),
    ).toHaveAttribute("href", "/projects/banskabystrica/approvals/chg-1a2b3c4d");
    // Checked before it was proposed (PF-57, T-0956).
    expect(checksSoFar().some((check) => check.includes("POST /api/v1/projects/banskabystrica/spaces"))).toBe(true);
  });

  it("checks a new space like every other form: a YAML view, a Check, and Propose only after it (T-1380)", async () => {
    const checks: unknown[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL) => {
        const request = input as Request;
        const url = new URL(request.url);
        const json = (body: unknown, status = 200) =>
          new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
        if (url.pathname.endsWith("/auth/me")) return json(IDENTITY);
        if (url.pathname.endsWith("/branding")) return json({ validation: "strict" });
        if (request.method === "POST" && url.searchParams.get("dryRun") === "All") {
          const body = (await request.clone().json()) as Record<string, unknown>;
          checks.push(body);
          const manifest = { ...body };
          delete manifest.draft;
          return json({
            valid: true,
            verdict: { ok: true, findings: [], checkedAt: new Date().toISOString(), inputDigest: digestOf(manifest) },
          });
        }
        if (url.pathname === "/api/v1/projects/banskabystrica") return json(project(3));
        if (url.pathname.endsWith("/spaces")) return json(SPACES);
        return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
      }),
    );
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <I18nextProvider i18n={i18n}>
          <App />
        </I18nextProvider>
      </QueryClientProvider>,
    );

    await userEvent.click(await screen.findByRole("button", { name: en.spaces.add }));
    const dialog = await findFormPage();
    expect(within(dialog).getByRole("tab", { name: en.form.view.yaml })).toBeInTheDocument();
    await userEvent.type(within(dialog).getByLabelText(/Name/), "mobilita");
    // Refused until the check is green, and reachable while it is refused (UI-44, T-1743).
    await waitFor(() => expectDenied(within(dialog).getByRole("button", { name: en.spaces.propose })));

    await userEvent.click(within(dialog).getByRole("button", { name: en.form.check }));
    await waitFor(() => expectOpen(within(dialog).getByRole("button", { name: en.spaces.propose })));
    expect(checks[0]).toMatchObject({
      kind: "ContextSpace",
      metadata: { name: "mobilita" },
      draft: { kind: "ContextSpace", name: "mobilita" },
    });
  });

  it("shows the name to use when another project already holds the one typed (PF-76)", async () => {
    renderSpaces({
      quota: 3,
      refusal:
        "context space name 'mhd' is taken by project doprava: a space name is unique in the " +
        "organization (PF-44); propose 'banskabystrica-mhd' instead",
    });

    await userEvent.click(await screen.findByRole("button", { name: en.spaces.add }));
    const dialog = await findFormPage();
    await userEvent.type(within(dialog).getByLabelText(/Name/), "mhd");
    await userEvent.click(within(dialog).getByRole("button", { name: en.form.check }));
    await waitFor(() => expectOpen(within(dialog).getByRole("button", { name: en.spaces.propose })));
    await userEvent.click(within(dialog).getByRole("button", { name: en.spaces.propose }));

    const alert = await within(dialog).findByRole("alert");
    expect(alert).toHaveTextContent("taken by project doprava");
    expect(alert).toHaveTextContent("banskabystrica-mhd");
  });

  it("refuses a name that is not a lowercase slug (PF-09)", async () => {
    const fetchMock = renderSpaces();

    await userEvent.click(await screen.findByRole("button", { name: en.spaces.add }));
    const dialog = await findFormPage();
    await userEvent.type(within(dialog).getByLabelText(/Name/), "Mobilita Mesta");
    await userEvent.click(within(dialog).getByRole("button", { name: en.spaces.propose }));

    // The message says what a name may be, rather than naming the pattern it broke (T-0960).
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(en.form.dns1123);
    expect(posts(fetchMock)).toHaveLength(0);
  });

  /**
   * AG-45, UI-45, T-2246: one sentence in the dock drafts a space and sends the person here;
   * the form is open and filled from that draft, so nothing is typed twice. Before this, the
   * page ignored `?draft=` and the person met an empty list.
   */
  it("opens the form filled from the draft the assistant left behind", async () => {
    window.history.pushState({}, "", "/projects/banskabystrica/spaces?draft=kvalita-vzduchu");
    renderSpaces({ quota: 3, draft: ASSISTANT_DRAFT });

    const dialog = await findFormPage();
    await waitFor(() =>
      expect(within(dialog).getByLabelText(/Name/)).toHaveValue("kvalita-vzduchu"),
    );
    expect(within(dialog).getByLabelText(/Title/)).toHaveValue("Air quality, the new space");
  });

  it("opens no form when the draft the route names is gone", async () => {
    window.history.pushState({}, "", "/projects/banskabystrica/spaces?draft=kvalita-vzduchu");
    renderSpaces({ quota: 3 });

    // The dialog is open — the route asked for it — and it holds no name from a draft that the
    // Portal no longer has, so the person types their own rather than proposing someone else's.
    const dialog = await findFormPage();
    await waitFor(() => expect(within(dialog).getByLabelText(/Name/)).toHaveValue(""));
  });
});

/**
 * T-1030, UI-44 and PF-61: a viewer holds `read` on `ContextSpace` and no `propose`. Looking
 * inside a space is a read, so the row's link is theirs to follow; only the controls that
 * change something are kept from them. The roles unit of the recording shows exactly this.
 */
describe("a viewer on the spaces list", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/projects/banskabystrica/spaces");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("follows the link into a space, and sees the controls that change one disabled with the reason (UI-44)", async () => {
    const viewer = {
      subject: "b7c1e0f4",
      username: "demo.viewer",
      name: "Demo Viewer",
      email: "demo.viewer@banskabystrica.sk",
      roles: ["portal-viewer"],
    };
    const readOnly = {
      project: "banskabystrica",
      bootstrap: false,
      grants: [{ role: "viewer", binding: "viewers", rule: { kinds: ["ContextSpace"], verbs: ["read"] } }],
    };
    const fetchMock = vi.fn(async (input: Request | string) => {
      const request = typeof input === "string" ? new Request(input) : input;
      const path = new URL(request.url, "http://localhost").pathname;
      const json = (body: unknown, status = 200) =>
        new Response(JSON.stringify(body), {
          status,
          headers: { "content-type": "application/json" },
        });
      if (path.endsWith("/auth/me")) return json(viewer);
      if (path.endsWith("/permissions/me")) return json(readOnly);
      if (path.endsWith("/spaces")) return json(SPACES);
      return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
    });
    vi.stubGlobal("fetch", fetchMock);
    vi.stubGlobal("fetch", answeringChecks(globalThis.fetch));

    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <App />
        </I18nextProvider>
      </QueryClientProvider>,
    );

    const row = (await screen.findByText("ovzdusie")).closest("tr") as HTMLElement;
    // The row's name opens the space (T-6095): no "Look inside" button beside it.
    const open = within(row).getByRole("link", { name: "Air quality" });
    expect(open).toBeInTheDocument();
    expect(open).toHaveAttribute("href", "/projects/banskabystrica/spaces/ovzdusie");
    // Nothing disables a link, so a viewer who may read is never stopped from looking.
    expect(open).not.toHaveAttribute("aria-disabled", "true");
    // What changes a space stays on the page, disabled, and says why (T-1383).
    // The guard remounts the control once the document arrives: queried after the wait.
    await waitFor(() => {
      expect(screen.getByRole("button", { name: en.spaces.add })).toHaveAttribute("aria-disabled");
    });
    expectDenied(
      screen.getByRole("button", { name: en.spaces.add }),
      "Disabled: your role does not permit 'propose' on 'ContextSpace' in this project",
    );
    // Delete lives in the row's menu now (T-2279); it is listed there, disabled, with the reason,
    // because a role that is too narrow has to be readable and not merely absent (UI-44).
    await userEvent.click(within(row).getByRole("button", { name: /More actions/ }));
    const denied = await screen.findByRole("menuitem", { name: new RegExp(en.resourceDelete.button) });
    expect(denied).toHaveAttribute("aria-disabled", "true");
    expect(denied).toHaveAttribute(
      "title",
      "Disabled: your role does not permit 'delete' on 'ContextSpace' in this project",
    );
  });

  it("edits a space in the kind's own form, not as a manifest (T-2278)", async () => {
    const fetchMock = renderSpaces({});

    const row = (await screen.findByText("ovzdusie")).closest("tr") as HTMLElement;
    // One control in the open (Open) and the rest behind the row's menu (T-2279).
    await userEvent.click(within(row).getByRole("button", { name: /More actions/ }));
    await userEvent.click(await screen.findByRole("menuitem", { name: en.resourceEdit.button }));
    const dialog = await findFormPage();

    // The fields of a context space, filled in from the stored manifest — and the name, which is the
    // manifest's path, is readable but not editable (MF-11).
    const name = await within(dialog).findByLabelText(/^Name/);
    expect(name).toHaveValue("ovzdusie");
    expect(name).toHaveAttribute("readonly");
    expect(within(dialog).getByLabelText(new RegExp(`^${en.spaces.field.locale}`))).toBeInTheDocument();
    // Not the manifest as text: that is what a person was handed before.
    expect(within(dialog).queryByRole("textbox", { name: "YAML" })).toBeNull();

    // And the edit is proposed as a change, from the form's own submit.
    await userEvent.click(within(dialog).getByRole("button", { name: en.resourceEdit.propose }));
    await waitFor(() =>
      expect(
        fetchMock.mock.calls
          .map((call) => call[0] as Request)
          .some((request) => request.method === "PUT" && request.url.includes("/spaces/ovzdusie")),
      ).toBe(true),
    );
  });

  it("keeps one control in the open and the rest in the row's menu (T-2279)", async () => {
    renderSpaces({});

    const row = (await screen.findByText("ovzdusie")).closest("tr") as HTMLElement;
    // The name opens the space (T-6095); everything else is one menu, not four buttons.
    expect(within(row).getByRole("link", { name: "Air quality" })).toBeInTheDocument();
    expect(within(row).queryByRole("link", { name: en.spaces.inside.open })).toBeNull();
    // One menu ends the row, Details its first item (owner 2026-10-31); the size's (i) is a reading, not an action (T-6208).
    const buttons = within(row)
      .getAllByRole("button")
      .filter((button) => !/^When the size of /.test(button.getAttribute("title") ?? ""));
    expect(buttons).toHaveLength(1);

    await userEvent.click(within(row).getByRole("button", { name: /More actions/ }));
    const menu = await screen.findByRole("menu");
    const items = within(menu)
      .getAllByRole("menuitem")
      .map((item) => item.textContent);
    expect(items).toEqual([
      en.grid.details,
      en.resourceList.open,
      en.resourceEdit.button,
      en.resourceList.viewSource,
      en.saveAs.button,
      en.workspaces.open.action,
      en.resourceDelete.button,
    ]);

    // Escape closes it and the row is where it was: the keyboard is the menu's own (UI-46).
    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("menu")).toBeNull());
  });
});
