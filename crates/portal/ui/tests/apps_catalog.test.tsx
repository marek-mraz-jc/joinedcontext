import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";
import { draftState, openBlockedReason } from "../src/pages/apps/AppsCatalog";
import type { Manifest } from "../src/api/manifest";
import { renderRoute } from "./pageHarness";

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  email: "jana.kovacova@banskabystrica.sk",
  roles: ["domain-editor"],
};

function app(overrides: Record<string, unknown> = {}, spec: Record<string, unknown> = {}) {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "App",
    metadata: {
      name: "mapa-ovzdusia",
      namespace: "banskabystrica",
      title: { en: "Air quality map" },
      description: { en: "Stations coloured by PM10" },
      ...overrides,
    },
    status: {
      phase: "Live",
      observedRevision: "9f1c2ab",
      sourceUrl: "https://git.example.sk/city/config/src/branch/app/mapa-ovzdusia",
    },
    spec: {
      kind: "static",
      visibility: "project",
      lifecycle: "preview",
      embeddable: true,
      dataNeeds: [
        {
          contextSpaceRef: { name: "ovzdusie" },
          types: ["AirQualityObserved"],
          operations: ["queryEntity"],
        },
      ],
      ...spec,
    },
  };
}

const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-0000a1b2", namespace: "banskabystrica" },
  status: { lane: "yellow", phase: "PendingApproval", plan: { update: 1 } },
};

/** What the App probe last said, per project (AP-136); each test starts with none. */
let APP_CHECKS: unknown[] = [];

/** What the dry run before a write answers: green unless a test asks for a red check (PF-57). */
const GREEN = { valid: true, verdict: { ok: true, findings: [] } };

function renderCatalog(
  apps: unknown[],
  writeResponse: { body: unknown; status: number } = { body: CHANGE, status: 202 },
  runs: unknown[] = [],
  check: unknown = GREEN,
  builds: Record<string, unknown> = {},
  permissions?: unknown,
) {
  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    const request = input as Request;
    const path = new URL(request.url).pathname;
    const json = (body: unknown, status = 200) =>
      Promise.resolve(
        new Response(JSON.stringify(body), {
          status,
          headers: {
            "Content-Type": status >= 400 ? "application/problem+json" : "application/json",
          },
        }),
      );

    if (path.endsWith("/auth/me")) {
      return json(IDENTITY);
    }
    if (path.endsWith("/permissions/me") && permissions) {
      return json(permissions);
    }
    if (path.endsWith("/apps") && request.method === "GET") {
      return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: apps });
    }
    const build = /\/apps\/([^/]+)\/build$/.exec(path);
    if (build && request.method === "GET") {
      return build[1] in builds
        ? json(builds[build[1]])
        : json({ title: "Not Found", status: 404 }, 404);
    }
    if (path.endsWith("/app-checks")) {
      return json({ checks: APP_CHECKS });
    }
    if (path.endsWith("/agent-runs") && request.method === "GET") {
      return json({ items: runs });
    }
    if (request.method !== "GET") {
      // A write is checked on the same route and verb first (T-2264); the answer under test
      // belongs to the real one.
      return new URL(request.url).searchParams.get("dryRun") === "All"
        ? json(check)
        : json(writeResponse.body, writeResponse.status);
    }
    return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
  });
  vi.stubGlobal("fetch", fetchMock);

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

/** The same app with a build the lane published (AP-13a). */
function built(manifest: ReturnType<typeof app>) {
  return {
    ...manifest,
    status: {
      ...manifest.status,
      build: {
        digest: "sha256:4f1b9c0e2d7a6b5c4f1b9c0e2d7a6b5c4f1b9c0e2d7a6b5c4f1b9c0e2d7a6b5c",
        commit: "9f1c2ab",
        sdkVersion: "0.4.0",
        builtAt: "2026-09-21T10:00:00Z",
      },
    },
  };
}

/** The card's one menu button, named after the app (UI-26). */
const more = (title: string) => en.rowActions.more.replace("{name}", title);

async function cardOf(title: string): Promise<HTMLElement> {
  return (await screen.findByText(title)).closest("li") as HTMLElement;
}

/** Opens a card's ⋯ menu and chooses one of its items. */
async function choose(user: ReturnType<typeof userEvent.setup>, title: string, item: string) {
  await user.click(within(await cardOf(title)).getByRole("button", { name: more(title) }));
  await user.click(await screen.findByRole("menuitem", { name: new RegExp(`^${item}`) }));
}

function writes(fetchMock: ReturnType<typeof vi.fn>): Request[] {
  return fetchMock.mock.calls
    .map((call) => call[0] as Request)
    .filter((request) => request.method !== "GET");
}

describe("apps catalog", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/projects/banskabystrica/apps");
    APP_CHECKS = [];
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("renders one card per app with its lifecycle, visibility and data needs (AP-18)", async () => {
    renderCatalog([app(), app({ name: "hluk", title: { en: "Noise" } }, { lifecycle: "published" })]);

    const card = (await screen.findByText("Air quality map")).closest("li") as HTMLElement;
    // The chip, not the button: the colour is never the only carrier, so it has the tooltip.
    expect(within(card).getByTitle(en.appLifecycle.previewHelp)).toHaveTextContent(
      en.appLifecycle.preview,
    );
    expect(within(card).getByText("Visible to members of this project")).toBeInTheDocument();
    expect(within(card).getByText(/AirQualityObserved/)).toBeInTheDocument();
    expect(within(card).getByText(/ovzdusie/)).toBeInTheDocument();
    // A card keeps its width however narrow the page beside the assistant is: the grid fits as
    // many 14rem columns as there is room for, never a column count set by the window's width.
    expect(card.closest("ul")?.className).toContain("grid-cols-[repeat(auto-fill,minmax(14rem,1fr))]");
    expect(card.closest("ul")?.className).not.toMatch(/\b(sm|lg|xl):grid-cols-/);

  });

  it("a published app carries the probe's verdict in words, a preview and an unchecked app none (AP-136)", async () => {
    APP_CHECKS = [
      { name: "hluk", state: "red", at: "2026-09-25T09:00:00Z", reason: "no row read in 60 s" },
      { name: "mapa-ovzdusia", state: "green", at: "2026-09-25T09:00:00Z" },
    ];
    renderCatalog([
      built(app({ name: "hluk", title: { en: "Noise" } }, { lifecycle: "published" })),
      built(app({ name: "teploty", title: { en: "Temperatures" } }, { lifecycle: "published" })),
      app(),
    ]);
    const noise = await cardOf("Noise");
    expect(await within(noise).findByText(en.apps.check.red)).toBeInTheDocument();
    expect(within(noise).getByText(/: no row read in 60 s$/)).toBeInTheDocument();
    // A preview is not probed, whatever the probe said of its name; an app not yet checked shows nothing.
    expect(within(await cardOf("Air quality map")).queryByText(en.apps.check.green)).toBeNull();
    expect(within(await cardOf("Temperatures")).queryByText(/^Checked/)).toBeNull();
  });

  // T-3038: one click on a served app's card is the app, framed under the Portal's header.
  it("a served app's card opens the app itself: its title is the link, one menu, no Open button (T-3038, AP-122)", async () => {
    renderCatalog([built(app({ name: "hluk", title: { en: "Noise" } }, { lifecycle: "published" }))]);

    const card = await cardOf("Noise");
    const link = await within(card).findByRole("link", { name: "Noise" });
    await waitFor(() => {
      expect(link).toHaveAttribute("href", "/projects/banskabystrica/apps/hluk/open");
    });
    expect(link).not.toHaveAttribute("target");
    expect(within(card).queryByRole("link", { name: en.apps.openAction })).toBeNull();
    expect(within(card).queryByRole("button", { name: new RegExp(`^${en.apps.openAction}`) })).toBeNull();
    expect(within(card).getByRole("button", { name: more("Noise") })).toBeInTheDocument();
    expect([...within(card).queryAllByRole("button"), ...within(card).queryAllByRole("link")]).toHaveLength(2);
  });

  it("a click on the card's body lands in the framed app; the menu's click does not navigate (T-3038)", async () => {
    const user = userEvent.setup();
    renderCatalog([built(app({ name: "hluk", title: { en: "Noise" } }, { lifecycle: "published" }))]);
    const card = await cardOf("Noise");
    await waitFor(() => {
      expect(within(card).getByRole("link", { name: "Noise" })).toHaveAttribute("href", "/projects/banskabystrica/apps/hluk/open");
    });

    await user.click(within(card).getByRole("button", { name: more("Noise") }));
    expect(await screen.findByRole("menu")).toBeInTheDocument();
    expect(window.location.pathname).toBe("/projects/banskabystrica/apps");
    await user.keyboard("{Escape}");
    await waitFor(() => {
      expect(screen.queryByRole("menu")).toBeNull();
    });

    await user.click(within(card).getByText("Visible to members of this project"));
    await waitFor(() => {
      expect(window.location.pathname).toBe("/projects/banskabystrica/apps/hluk/open");
    });
  });

  it("Enter on the card's focused link opens the app (T-3038, UI keyboard)", async () => {
    const user = userEvent.setup();
    renderCatalog([built(app({ name: "hluk", title: { en: "Noise" } }, { lifecycle: "published" }))]);
    const link = within(await cardOf("Noise")).getByRole("link", { name: "Noise" });
    await waitFor(() => {
      expect(link).toHaveAttribute("href", "/projects/banskabystrica/apps/hluk/open");
    });
    link.focus();
    await user.keyboard("{Enter}");
    await waitFor(() => {
      expect(window.location.pathname).toBe("/projects/banskabystrica/apps/hluk/open");
    });
  });

  it("the menu takes the owner to the app's page, which the card no longer opens (T-3038)", async () => {
    const user = userEvent.setup();
    renderCatalog([built(app({ name: "hluk", title: { en: "Noise" } }, { lifecycle: "published" }))]);
    await choose(user, "Noise", en.apps.openPage.details);
    await waitFor(() => {
      expect(window.location.pathname).toBe("/projects/banskabystrica/apps/hluk");
    });
  });

  it("a preview says why it does not open and its card opens the app's page (T-2618, T-3038, UI-44)", async () => {
    const user = userEvent.setup();
    renderCatalog([app()]);

    const card = await cardOf("Air quality map");
    expect(card).toHaveTextContent(en.apps.openDisabled.preview);
    expect(within(card).queryByRole("button", { name: new RegExp(`^${en.apps.openAction}`) })).toBeNull();
    expect(within(card).getByRole("link", { name: "Air quality map" })).toHaveAttribute(
      "href",
      "/projects/banskabystrica/apps/mapa-ovzdusia",
    );

    // Opened with the keyboard: Tab order is the title, then the menu.
    within(card).getByRole("button", { name: more("Air quality map") }).focus();
    await user.keyboard("{Enter}");
    const items = (await screen.findAllByRole("menuitem")).map((item) => item.textContent ?? "");
    expect(items.slice(0, 4).map((text) => text.split(" — ")[0])).toEqual([
      en.apps.previewAction,
      en.apps.publishAction,
      en.apps.rebuildAction,
      en.apps.retireAction,
    ]);
    const item = (label: string) => screen.getByRole("menuitem", { name: new RegExp(`^${label}`) });
    expect(item(en.apps.previewAction)).not.toHaveAttribute("aria-disabled", "true");
    expect(item(en.apps.publishAction)).not.toHaveAttribute("aria-disabled", "true");
    expect(item(en.apps.rebuildAction)).toHaveAttribute("aria-disabled", "true");
    expect(item(en.apps.rebuildAction)).toHaveTextContent(en.apps.rebuildOnlyPublished);
    expect(item(en.apps.retireAction)).toHaveTextContent(en.apps.retireOnlyPublished);
    // The manifest's own four follow in the same menu: one menu per card.
    expect(item(en.resourceEdit.button)).toBeInTheDocument();
    expect(item(en.resourceDelete.button)).toBeInTheDocument();
    expect(screen.getAllByRole("menu")).toHaveLength(1);
    await user.keyboard("{Escape}");
    await waitFor(() => {
      expect(screen.queryByRole("menu")).toBeNull();
    });
  });

  it("lists the source, the latest run and the package only when each exists, as new-tab links (T-2618, AP-103)", async () => {
    const user = userEvent.setup();
    const FORGE = "https://forge.example/joinedcontext";
    renderCatalog(
      [
        built(app({ name: "bikes", title: { en: "Bikes" } }, { lifecycle: "published" })),
        built(
          app({ name: "hluk", title: { en: "Noise" } }, { lifecycle: "published" }),
        ),
      ],
      undefined,
      [],
      GREEN,
      {
        bikes: {
          repositoryUrl: `${FORGE}-apps/helsinki_bikes`,
          configurationUrl: `${FORGE}/helsinki`,
          run: { status: "completed", conclusion: "success", commit: "9f1c2ab", url: `${FORGE}/helsinki_bikes/actions/runs/7` },
          packageUrl: `${FORGE}/-/packages/generic/app-bikes/9f1c2ab`,
          rebuild: { allowed: true },
        },
      },
    );

    await user.click(within(await cardOf("Bikes")).getByRole("button", { name: more("Bikes") }));
    const link = (label: string) => screen.getByRole("menuitem", { name: label });
    // The App's whole source and the project's configuration first (T-3039), in that order.
    const items = screen.getAllByRole("menuitem").map((item) => item.textContent ?? "");
    expect(items.indexOf(en.apps.sourceCode)).toBe(items.indexOf(en.apps.openPage.details) + 1);
    expect(items.indexOf(en.apps.projectConfiguration)).toBe(items.indexOf(en.apps.sourceCode) + 1);
    for (const [label, href] of [
      [en.apps.sourceCode, `${FORGE}-apps/helsinki_bikes`],
      [en.apps.projectConfiguration, `${FORGE}/helsinki`],
      [en.apps.history, "https://git.example.sk/city/config/src/branch/app/mapa-ovzdusia"],
      [en.apps.latestRun, `${FORGE}/helsinki_bikes/actions/runs/7`],
      [en.apps.package, `${FORGE}/-/packages/generic/app-bikes/9f1c2ab`],
    ]) {
      expect(link(label).tagName).toBe("A");
      expect(link(label)).toHaveAttribute("href", href);
      expect(link(label)).toHaveAttribute("target", "_blank");
      expect(link(label)).toHaveAttribute("rel", "noreferrer noopener");
    }
    expect(screen.getByRole("menuitem", { name: en.apps.rebuildAction })).not.toHaveAttribute("aria-disabled", "true");
    await user.keyboard("{Escape}");

    // No build of its own on the forge: no source repository, no run, no package, and Rebuild
    // says why; a person the forge would not let read the configuration is not offered it.
    await user.click(within(await cardOf("Noise")).getByRole("button", { name: more("Noise") }));
    expect(await screen.findByRole("menuitem", { name: en.apps.history })).toBeInTheDocument();
    expect(screen.queryByRole("menuitem", { name: en.apps.sourceCode })).toBeNull();
    expect(screen.queryByRole("menuitem", { name: en.apps.projectConfiguration })).toBeNull();
    expect(screen.queryByRole("menuitem", { name: en.apps.latestRun })).toBeNull();
    expect(screen.queryByRole("menuitem", { name: en.apps.package })).toBeNull();
    expect(screen.getByRole("menuitem", { name: new RegExp(`^${en.apps.rebuildAction}`) })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
  });

  it("a person who may only read sees publish, retire and edit disabled with the permission sentence (T-2618, UI-44)", async () => {
    const user = userEvent.setup();
    renderCatalog([app()], undefined, [], GREEN, {}, {
      grants: [{ rule: { kinds: ["App"], verbs: ["read"] } }],
      bootstrap: false,
    });

    await user.click(within(await cardOf("Air quality map")).getByRole("button", { name: more("Air quality map") }));
    const denied = i18n.t("permissions.denied", { verb: "propose", kind: "App" });
    for (const label of [en.apps.publishAction, en.resourceEdit.button, en.saveAs.button]) {
      const item = await screen.findByRole("menuitem", { name: new RegExp(`^${label}`) });
      expect(item).toHaveAttribute("aria-disabled", "true");
      expect(item).toHaveTextContent(denied);
    }
  });

  it("rebuild dispatches the workflow and says so (T-2618, AP-103)", async () => {
    const user = userEvent.setup();
    const fetchMock = renderCatalog(
      [built(app({ name: "bikes", title: { en: "Bikes" } }, { lifecycle: "published" }))],
      { body: { dispatched: true }, status: 202 },
      [],
      GREEN,
      { bikes: { repositoryUrl: "https://forge.example/r", run: null, rebuild: { allowed: true } } },
    );

    await choose(user, "Bikes", en.apps.rebuildAction);

    await waitFor(() => {
      expect(writes(fetchMock).map((request) => new URL(request.url).pathname)).toContain(
        "/api/v1/projects/banskabystrica/apps/bikes/rebuild",
      );
    });
    expect(await screen.findByText(en.apps.build.started)).toBeInTheDocument();
  });

  it("offers no Open on a retired app (T-2759, AP-86)", async () => {
    renderCatalog([app({}, { lifecycle: "retired" })]);
    const card = (await screen.findByText("Air quality map")).closest("li") as HTMLElement;
    expect(within(card).queryByRole("button", { name: en.apps.openAction })).toBeNull();
    expect(within(card).queryByRole("link", { name: en.apps.openAction })).toBeNull();
  });

  it("retiring asks first, then proposes lifecycle retired (T-2618, AP-18)", async () => {
    const user = userEvent.setup();
    const fetchMock = renderCatalog([built(app({ name: "bikes", title: { en: "Bikes" } }, { lifecycle: "published" }))]);

    await choose(user, "Bikes", en.apps.retireAction);
    const dialog = await screen.findByRole("dialog");
    expect(writes(fetchMock)).toHaveLength(0);
    await user.click(within(dialog).getByRole("button", { name: en.apps.retire.confirm }));

    await waitFor(() => {
      expect(writes(fetchMock)).toHaveLength(2);
    });
    const body = JSON.parse(await writes(fetchMock)[1].text()) as { spec: Record<string, unknown> };
    expect(body.spec.lifecycle).toBe("retired");
  });

  it("asks in a dialog that holds the focus, and Escape leaves the app as it is (T-3281)", async () => {
    const user = userEvent.setup();
    const fetchMock = renderCatalog([built(app({ name: "bikes", title: { en: "Bikes" } }, { lifecycle: "published" }))]);

    await choose(user, "Bikes", en.apps.retireAction);
    const dialog = await screen.findByRole("dialog", { name: en.apps.retire.title.replace("{name}", "Bikes") });
    expect(dialog).toHaveAttribute("aria-describedby");
    expect(dialog.contains(document.activeElement)).toBe(true);
    // The page behind is out of reach while it asks.
    for (let step = 0; step < 6; step += 1) {
      await user.tab();
      expect(dialog.contains(document.activeElement)).toBe(true);
    }
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(writes(fetchMock)).toHaveLength(0);
  });

  // AP-86, AP-87: the host serves a published App only from a build the lane published or the
  // bundle the Portal image ships; one with neither answers 404, so its card opens its page.
  it("opens the app from the card only when something serves it (AP-86, AP-87, T-3038)", async () => {
    renderCatalog([
      app({ name: "allerts", title: { en: "Alerts" } }, { lifecycle: "published" }),
      app(
        {
          name: "ukazovatele",
          title: { en: "Indicators" },
          annotations: { "joinedcontext.com/shipped-with": "portal" },
        },
        { lifecycle: "published" },
      ),
      app(
        {
          name: "unshipped",
          title: { en: "Claims" },
          annotations: { "joinedcontext.com/shipped-with": "gitea" },
        },
        { lifecycle: "published" },
      ),
      built(app({ name: "bikes", title: { en: "Bikes" } }, { lifecycle: "published" })),
      built(app({ name: "old", title: { en: "Old" } }, { lifecycle: "retired" })),
    ]);

    const linkOf = async (title: string) =>
      within(await cardOf(title)).getByRole("link", { name: title });
    await waitFor(async () => {
      expect(await linkOf("Bikes")).toHaveAttribute("href", "/projects/banskabystrica/apps/bikes/open");
    });
    expect(await linkOf("Indicators")).toHaveAttribute("href", "/projects/banskabystrica/apps/ukazovatele/open");
    expect(await linkOf("Alerts")).toHaveAttribute("href", "/projects/banskabystrica/apps/allerts");
    expect(await linkOf("Claims")).toHaveAttribute("href", "/projects/banskabystrica/apps/unshipped");
    // A retired app is gone: its card opens its page, never a frame of nothing.
    expect(await linkOf("Old")).toHaveAttribute("href", "/projects/banskabystrica/apps/old");
  });

  // AP-86, AP-87: a published card says whether its newest run is building, failed or served,
  // and one with no repository of its own that nothing ships says why it has no Open.
  it("shows each published app's build state from the forge's runs (AP-86)", async () => {
    const FORGE = "https://forge.example/user/login?redirect_to=";
    const run = (status: string, conclusion: string | null, commit: string) => ({
      status,
      conclusion,
      commit,
      url: `${FORGE}%2Fjoinedcontext%2Fruns%2F${commit}`,
    });
    const onForge = (runState: unknown) => ({
      repositoryUrl: `${FORGE}%2Fjoinedcontext%2Frepo`,
      run: runState,
      rebuild: { allowed: true },
    });
    renderCatalog(
      [
        app({ name: "building", title: { en: "Building one" } }, { lifecycle: "published" }),
        built(app({ name: "failing", title: { en: "Failing one" } }, { lifecycle: "published" })),
        built(app({ name: "bikes", title: { en: "Bikes" } }, { lifecycle: "published" })),
        app({ name: "allerts", title: { en: "Alerts" } }, { lifecycle: "published" }),
      ],
      undefined,
      [],
      GREEN,
      {
        building: onForge(run("running", null, "abcdef0123")),
        failing: onForge(run("completed", "failure", "0123456789")),
        bikes: onForge(run("completed", "success", "9f1c2ab")),
        allerts: { repositoryUrl: null, run: null, rebuild: { allowed: false } },
      },
    );

    const card = async (title: string) => (await screen.findByText(title)).closest("li") as HTMLElement;
    // The state in words on the card; the commit only in the tooltip (T-2759, AP-86).
    const building = await within(await card("Building one")).findByText(en.apps.build.state.building);
    expect(building.closest("[title]")).toHaveAttribute("title", "Built from commit abcdef0");
    const failing = await card("Failing one");
    expect(within(failing).queryByText(en.apps.build.state.failed)).toBeNull();
    const served = await within(await card("Bikes")).findByText(en.apps.build.state.served);
    expect(served.closest("[title]")).toHaveAttribute("title", "Built from commit 9f1c2ab");
    expect(within(await card("Bikes")).queryByText(/9f1c2ab/)).toBeNull();
    expect(await within(await card("Alerts")).findByText(en.apps.build.state.noRepository)).toBeInTheDocument();
  });

  it("frames the preview in an opaque origin, never same-origin with the Portal (AP-19)", async () => {
    const user = userEvent.setup();
    renderCatalog([app()]);

    await choose(user, "Air quality map", en.apps.previewAction);

    const frame = await screen.findByTitle("Preview of Air quality map");
    // An app is served from the Portal's own origin (AP-14), so `allow-same-origin` would let
    // the framed app act as the signed-in reviewer. `allow-scripts` alone is the whole point.
    const sandbox = frame.getAttribute("sandbox") ?? "";
    expect(sandbox.split(/\s+/)).toContain("allow-scripts");
    expect(sandbox).not.toContain("allow-same-origin");
    expect(sandbox).not.toContain("allow-top-navigation");
    // The commit the preview was built from, so a reviewer never reads a stale build.
    expect(frame).toHaveAttribute("src", "/apps/mapa-ovzdusia/?preview=9f1c2ab");
  });

  it("says why an app that refuses framing has no preview (AP-12)", async () => {
    const user = userEvent.setup();
    renderCatalog([app({}, { embeddable: false })]);

    await choose(user, "Air quality map", en.apps.previewAction);

    expect(await screen.findByRole("alert")).toHaveTextContent(en.apps.preview.notEmbeddable);
    expect(screen.queryByTitle("Preview of Air quality map")).toBeNull();
  });

  it("publishing asks first, then proposes a change rather than flipping a switch (AP-20)", async () => {
    const user = userEvent.setup();
    const fetchMock = renderCatalog([app()]);

    await choose(user, "Air quality map", en.apps.publishAction);

    // The confirmation says what publishing does, and nothing has been sent yet.
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent("reachable by members of this project");
    expect(writes(fetchMock)).toHaveLength(0);

    await user.click(within(dialog).getByRole("button", { name: en.apps.publish.confirm }));

    // Two: the check the verdict gate wants for this manifest, then the write it lets through
    // (PF-57, T-2264).
    await waitFor(() => {
      expect(writes(fetchMock)).toHaveLength(2);
    });
    expect(new URL(writes(fetchMock)[0].url).searchParams.get("dryRun")).toBe("All");
    const request = writes(fetchMock)[1];
    expect(request.method).toBe("PUT");
    expect(new URL(request.url).searchParams.get("dryRun")).toBe(null);
    expect(new URL(request.url).pathname).toBe(
      "/api/v1/projects/banskabystrica/apps/mapa-ovzdusia",
    );
    const body = JSON.parse(await request.text()) as { spec: Record<string, unknown> };
    expect(body.spec.lifecycle).toBe("published");
    // MF-04: the status is the API's own, and a write never sends it back.
    expect(body).not.toHaveProperty("status");

    // The answer is a merge request, not a saved record (CC-32).
    expect(await screen.findByText("chg-0000a1b2")).toBeInTheDocument();
    expect(screen.getByText(en.changes.accepted)).toBeInTheDocument();
  });

  it("cancelling the publication sends nothing", async () => {
    const user = userEvent.setup();
    const fetchMock = renderCatalog([app()]);

    await choose(user, "Air quality map", en.apps.publishAction);
    const dialog = await screen.findByRole("dialog");
    await user.click(within(dialog).getByRole("button", { name: en.apps.publish.cancel }));

    await waitFor(() => {
      expect(screen.queryByRole("dialog")).toBeNull();
    });
    expect(writes(fetchMock)).toHaveLength(0);
  });

  it("shows the server's own reason when the publication is refused", async () => {
    const user = userEvent.setup();
    renderCatalog([app()], {
      status: 403,
      body: {
        type: "https://joinedcontext.com/errors/forbidden",
        title: "Forbidden",
        status: 403,
        detail: "publishing a public app needs the org-admin role",
      },
    });

    await choose(user, "Air quality map", en.apps.publishAction);
    const dialog = await screen.findByRole("dialog");
    await user.click(within(dialog).getByRole("button", { name: en.apps.publish.confirm }));

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "publishing a public app needs the org-admin role",
    );
  });

  it("an empty catalogue says so instead of showing an empty grid", async () => {
    renderCatalog([]);
    expect(await screen.findByText(en.apps.empty)).toBeInTheDocument();
  });

  it("keeps a published App closed until its host has its certificate (AP-133, T-2838)", () => {
    const t = i18n.getFixedT("en");
    const app = (conditions: object[]): Manifest =>
      ({
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "App",
        metadata: { name: "bikes", annotations: { "joinedcontext.com/shipped-with": "portal" } },
        spec: { lifecycle: "published" },
        status: { conditions },
      }) as unknown as Manifest;
    const ready = (status: string, reason: string) => [{ type: "Ready", status, reason }];
    expect(openBlockedReason(app(ready("False", "CertificatePending")), null, t)).toBe(en.apps.openDisabled.certificate);
    expect(openBlockedReason(app(ready("False", "HostRefused")), null, t)).toBe(en.apps.openDisabled.host);
    // Issued, or no word yet, or another reason: the build decides, as before.
    expect(openBlockedReason(app(ready("True", "CertificatePending")), null, t)).toBeUndefined();
    expect(openBlockedReason(app([]), null, t)).toBeUndefined();
    expect(openBlockedReason(app(ready("False", "BuildMissing")), null, t)).toBeUndefined();
  });

  it("draftState maps lifecycle statuses to draft categories (AP-70)", () => {
    expect(draftState("queued")).toBe("building");
    expect(draftState("starting")).toBe("building");
    expect(draftState("building")).toBe("building");
    expect(draftState("testing")).toBe("building");
    expect(draftState("previewing")).toBe("readyToPublish");
    expect(draftState("interviewing")).toBe("needsYou");
    expect(draftState("awaiting_approval")).toBe("waitingApproval");
    expect(draftState("awaitingApproval")).toBe("waitingApproval");
    expect(draftState("failed")).toBe("failed");
    expect(draftState("cancelled")).toBe("failed");
    expect(draftState("expired")).toBe("failed");
    expect(draftState("published")).toBeNull();
  });

  /// T-2772: a build whose change waits for an approver says so on its tile and once above the
  /// grid, with the way to the approvals; nothing is said when none waits.
  it("says which builds wait for approval and links the approvals", async () => {
    const run = (id: string, appName: string, status: string) => ({
      id,
      project: "banskabystrica",
      appName,
      status,
      createdAt: "2026-09-12T08:00:00Z",
    });
    renderCatalog([], undefined, [
      run("r1", "mapa-vystavby", "awaiting_approval"),
      run("r2", "ovzdusie-dnes", "previewing"),
    ]);
    expect(await screen.findByText(en.apps.drafts.state.waitingApproval)).toBeInTheDocument();
    expect(screen.getByText(en.apps.drafts.state.readyToPublish)).toBeInTheDocument();
    expect(screen.getByText(/One application waits for its change to be approved/)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: en.apps.drafts.openApprovals })).toHaveAttribute(
      "href",
      "/projects/banskabystrica/approvals",
    );
  });

  it("says nothing about approvals when no build waits for one", async () => {
    renderCatalog([], undefined, [
      { id: "r3", project: "banskabystrica", appName: "mapa", status: "building", createdAt: "2026-09-12T08:00:00Z" },
    ]);
    expect(await screen.findByText(en.apps.drafts.state.building)).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: en.apps.drafts.openApprovals })).toBeNull();
  });

  it("shows an ended build as nothing: no tile, no builds list, only the apps and the drafts still going", async () => {
    const run = (id: string, appName: string, status: string) => ({
      id,
      project: "banskabystrica",
      appName,
      status,
      createdAt: "2026-09-12T08:00:00Z",
    });
    renderCatalog([], undefined, [
      run("r1", "stare-mapa", "expired"),
      run("r2", "zrusena", "cancelled"),
      run("r3", "mapa-vystavby", "building"),
    ]);

    expect(await screen.findByRole("heading", { name: "Mapa vystavby" })).toBeInTheDocument();
    expect(screen.queryByText("stare-mapa")).toBeNull();
    expect(screen.queryByText("zrusena")).toBeNull();
    expect(screen.queryByRole("heading", { name: en.apps.builds.title })).toBeNull();
  });

  it("opens the builder in the assistant instead of on the page", async () => {
    const user = userEvent.setup();
    const intents: unknown[] = [];
    const listener = (event: Event) => {
      intents.push((event as CustomEvent).detail);
    };
    window.addEventListener("jc:assistant-open", listener);
    renderCatalog([]);

    // An empty catalogue offers the same action in its empty state (T-3246): both open the builder.
    const buttons = await screen.findAllByRole("button", { name: en.apps.newAction });
    expect(buttons).toHaveLength(2);
    for (const button of buttons) {
      await user.click(button);
    }

    window.removeEventListener("jc:assistant-open", listener);
    expect(intents).toEqual(["build", "build"]);
    expect(screen.queryByLabelText(en.apps.generate.prompt, { exact: false })).toBeNull();
  });

  it("names a draft by its run's title, else its name as words, never the id", async () => {
    renderCatalog([], undefined, [
      {
        id: "r1",
        project: "banskabystrica",
        appName: "map-visualization",
        title: "Helsinki Traffic Alerts Map",
        status: "building",
        createdAt: "2026-09-12T08:00:00Z",
      },
      { id: "r2", project: "banskabystrica", appName: "kpi_board", status: "building", createdAt: "2026-09-12T08:00:00Z" },
    ]);
    expect(await screen.findByRole("heading", { name: "Helsinki Traffic Alerts Map" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Kpi board" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "map-visualization" })).toBeNull();
  });

  it("lists draft applications with their status label and no embed or preview link (AP-65, AP-70)", async () => {
    const draftRun = {
      id: "run-draft-1",
      project: "banskabystrica",
      appName: "mapa-vystavby",
      status: "building",
      createdAt: "2026-09-12T08:00:00Z",
    };
    renderCatalog([], undefined, [draftRun]);

    const heading = await screen.findByRole("heading", { name: "Mapa vystavby" });
    const card = heading.closest("li");
    if (!card) throw new Error("the draft is not a card");
    expect(within(card).getByText(en.apps.drafts.state.building)).toBeInTheDocument();
    expect(within(card).getByRole("link", { name: en.apps.drafts.open })).toHaveAttribute(
      "href",
      "/projects/banskabystrica/apps/mapa-vystavby",
    );
    expect(within(card).getAllByRole("link")).toHaveLength(1);
    expect(within(card).queryByRole("button")).toBeNull();
  });

  // T-3267, AP-140: publishing lists what the app lacks, each with where it is fixed, and a
  // privacy problem holds it; a warning does not.
  it("lists the gaps before publishing, links each, and holds a public app that would show unpublished data", async () => {
    const user = userEvent.setup();
    const fetchMock = renderCatalog([app({ description: undefined }, { visibility: "public" })]);
    await choose(user, "Air quality map", en.apps.publishAction);
    const dialog = await screen.findByRole("dialog");
    const list = await within(dialog).findByRole("region", { name: en.apps.checklist.heading });
    expect(list).toHaveTextContent(en.apps.checklist.description.warning);
    expect(within(list).getByRole("link", { name: en.apps.checklist.fix.description })).toHaveAttribute(
      "href",
      "/projects/banskabystrica/apps/mapa-ovzdusia/edit",
    );
    await waitFor(() => expect(list).toHaveTextContent("which no public endpoint publishes"));
    const confirm = within(dialog).getByRole("button", { name: en.apps.publish.confirm });
    expect(confirm).toHaveAttribute("aria-disabled", "true");
    await user.click(confirm);
    expect(writes(fetchMock)).toHaveLength(0);
  });

  it("publishes nothing when the check is red, and says what it found (PF-57, T-2264)", async () => {
    const user = userEvent.setup();
    const fetchMock = renderCatalog([app()], { body: CHANGE, status: 202 }, [], {
      valid: false,
      verdict: { ok: false, findings: [{ message: "the endpoint it reads is not public yet" }] },
    });

    await choose(user, "Air quality map", en.apps.publishAction);
    const dialog = await screen.findByRole("dialog");
    await user.click(within(dialog).getByRole("button", { name: en.apps.publish.confirm }));

    expect(await screen.findByText(/the endpoint it reads is not public yet/)).toBeInTheDocument();
    // The check alone was sent: a red check publishes nothing.
    expect(writes(fetchMock)).toHaveLength(1);
    expect(new URL(writes(fetchMock)[0].url).searchParams.get("dryRun")).toBe("All");
  });
});

// T-2750: `/apps/new` said "there is nothing called apps here"; it is what "New app" does.
describe("the apps page's create address", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    window.history.pushState({}, "", "/");
  });

  it("opens the assistant's builder over the list and leaves the address for the list", async () => {
    await renderRoute({ path: "/projects/helsinki/apps/new" });
    // The dock opened on its builder: the way back to the chat is what only that view shows.
    const dock = await screen.findByRole("complementary", { name: en.agentRun.conversation.title });
    expect(await within(dock).findByRole("button", { name: en.assistant.backToChat })).toBeInTheDocument();
    await waitFor(() => expect(window.location.pathname).toBe("/projects/helsinki/apps"));
    expect(screen.queryByText(en.form.notOpenNew)).toBeNull();
  });

  it("opens nothing for a role that may not propose an App, and says why without an API word", async () => {
    await renderRoute({
      path: "/projects/helsinki/apps/new",
      permissions: { project: "helsinki", bootstrap: false, grants: [] },
    });
    // The role, not a red warning that the address might be wrong (T-4263).
    await waitFor(() =>
      expect(screen.getAllByRole("status").some((status) => status.textContent?.includes(en.form.mayNotCreate))).toBe(true),
    );
    expect(screen.queryByText(en.form.notOpenNew)).toBeNull();
    expect(screen.queryByRole("button", { name: en.assistant.backToChat })).toBeNull();
    expect(screen.queryByText(/called apps/)).toBeNull();
  });
});
