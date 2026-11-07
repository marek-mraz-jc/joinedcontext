/**
 * T-1777, T-2606: Project settings against the page contract (UI-11, UI-15, UI-16, UI-44, UI-76).
 *
 * Project → Access became Project settings (Architecture/09 §14.3): its panels are tabs now, each
 * at its own address, so what this file owns is the frame and each tab's states — the H1 and the
 * document title, one outline per tab, the four states a list can be in, the keyboard, the four
 * languages, axe — and that what the Access page gave a person still reaches them: the grant form
 * from Members, the endpoint matrix from Your access. Each panel's own cases stay in its own test
 * (`role_bindings`, `access_groups`, `service_accounts_view`, `part_effective_permissions`).
 */
import { cleanup, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { ProjectSettingsPage } from "../src/pages/projectSettings/ProjectSettingsPage";
import type { ProjectSettingsTab } from "../src/pages/projectSettings/ProjectSettingsPage";
import {
  expectHeadingOutline,
  expectNoAxeViolations,
  inEveryLocale,
  json,
  list,
  problem,
  renderPage,
  tabOrder,
} from "./page_contract";

const PROJECT = "helsinki";

function binding(name: string, user: string, role: string) {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "RoleBinding",
    metadata: { name, namespace: "org" },
    spec: { subjects: [{ user }], role, scope: { project: PROJECT } },
  };
}

const ENDPOINT = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Endpoint",
  metadata: { name: "bikes-public", namespace: PROJECT, title: { en: "Bikes, open data" } },
  spec: { contextSpaceRef: "mobility", slug: "k7m2qz4tv6xh3n5jb2ryd3wcfa", audience: "public" },
};

const GRANTS = {
  subject: { type: "role", id: "public" },
  resource: { type: "endpoint", id: ENDPOINT.spec.slug, space: PROJECT },
  permissions: [
    {
      resource: { type: "BikeHireDockingStation" },
      actions: ["retrieveEntity", "queryEntity"],
      attributes: ["availableBikeNumber", "name"],
      constraints: { scopeQ: "/helsinki/#" },
    },
  ],
  prohibitions: [],
};

/** How many role bindings the project holds; `pending` keeps every request on its way. */
interface World {
  rows?: number;
  /** A fresh answer per request: a `Response` body can be read once, and panels retry. */
  fails?: { status: number; detail: string };
  pending?: boolean;
  grants?: "ok" | "forbidden";
  tab?: ProjectSettingsTab;
}

function renderSettings(world: World = {}) {
  const { rows = 1, fails, pending = false, grants = "ok", tab = "members" } = world;
  return renderPage(<ProjectSettingsPage project={PROJECT} tab={tab} />, {
    path: `/projects/${PROJECT}/settings/${tab}`,
    answer: async (url) => {
      if (pending) return new Promise<Response>(() => undefined);
      if (url.pathname.endsWith("/rolebindings")) {
        if (fails) return problem(fails.status, fails.detail);
        return json(
          list(
            Array.from({ length: rows }, (_, index) =>
              binding(`grant-${index}`, `person.${index}@helsinki.fi`, "domain-editor"),
            ),
          ),
        );
      }
      if (url.pathname.endsWith("/endpoints")) return json(list([ENDPOINT]));
      if (url.pathname.includes("/api/endpoint/")) {
        return grants === "forbidden"
          ? problem(403, "You may not read the grants of this endpoint.")
          : json(GRANTS);
      }
      return undefined;
    },
  });
}

const MEMBERS_CAPTION = en.access.roles.caption.replace("{project}", PROJECT);

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  document.title = "";
});

describe("Project settings", () => {
  // UI-15: the page names itself once, to the screen and to the tab.
  it("has one H1 and names the page and the project in the tab", async () => {
    renderSettings();
    const heading = await screen.findByRole("heading", { level: 1, name: en.projectSettings.title });
    expect(heading).toBeInTheDocument();
    expect(screen.getByText(en.projectSettings.lead.replace("{project}", PROJECT))).toBeInTheDocument();
    await waitFor(() => {
      expect(document.title).toBe(`${en.projectSettings.title} · ${PROJECT} · Helsinki Region Context`);
    });
  });

  // UI-76: the six tabs of Architecture/09 §14.2, the address's one selected.
  it("offers the six tabs, the address's one selected", async () => {
    renderSettings({ tab: "roles" });
    const tabs = within(await screen.findByRole("tablist", { name: en.projectSettings.tabsLabel })).getAllByRole("tab");
    expect(tabs.map((tab) => tab.textContent)).toEqual([
      en.projectSettings.tab.general,
      en.projectSettings.tab.members,
      en.projectSettings.tab.roles,
      en.projectSettings.tab["service-accounts"],
      en.projectSettings.tab.access,
      en.projectSettings.tab.danger,
    ]);
    expect(screen.getByRole("tab", { name: en.projectSettings.tab.roles })).toHaveAttribute("aria-selected", "true");
  });

  // UI-16: one H1 and the tab's H2s, in the order a heading list reads them, nothing skipped.
  it.each(["members", "access", "danger"] as const)("reads as one outline on %s", async (tab) => {
    const { container } = renderSettings({ tab });
    await screen.findByRole("heading", { level: 1, name: en.projectSettings.title });
    await screen.findAllByRole("heading", { level: 2 });
    expectHeadingOutline(container);
  });

  // UI-16: every panel is a region a screen reader can jump to, named by its own heading.
  it("gives each panel of Your access a named region", async () => {
    const { container } = renderSettings({ tab: "access" });
    await screen.findByRole("heading", { level: 2, name: en.access.matrix.title });
    const named = [...container.querySelectorAll("section[aria-labelledby]")].map(
      (section) => section.querySelector("h2")?.textContent,
    );
    expect(named).toEqual([en.projectSettings.access.title, en.access.matrix.title]);
  });

  // The loading state: a skeleton, and a table that says it is still loading rather than
  // claiming the project holds nobody.
  it("says the lists are loading rather than showing them empty", async () => {
    renderSettings({ pending: true });
    const loading = await screen.findAllByText(en.app.loading);
    expect(loading.length).toBeGreaterThan(0);
    expect(screen.queryByText(en.access.roles.empty)).not.toBeInTheDocument();
  });

  // The empty state: what this is, and what to do first.
  it("offers the first action when nobody holds a role yet", async () => {
    renderSettings({ rows: 0 });
    const empty = await screen.findByText(en.access.roles.empty);
    // The empty list offers the first grant itself (T-3246), beside the page's own button.
    const state = empty.closest("[data-empty-state]") as HTMLElement;
    expect(within(state).getByRole("button", { name: en.access.roles.grant })).toBeInTheDocument();
  });

  // The error state: the API's own sentence, not "something went wrong", and never an empty
  // list — a list that failed is not "you have nothing".
  it("shows the reason a list failed instead of an empty list", async () => {
    renderSettings({ fails: { status: 503, detail: "The configuration store is not answering." } });
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("The configuration store is not answering.");
    expect(screen.queryByText(en.access.roles.empty)).not.toBeInTheDocument();
  });

  // UI-44: what a person may not read says so in words; the server stays the judge.
  it("says who may grant what when the grants are refused", async () => {
    renderSettings({ tab: "access", grants: "forbidden" });
    expect(await screen.findByText(en.access.matrix.forbidden)).toBeInTheDocument();
  });

  // A list that holds one row and a list that holds many read the same way.
  it.each([0, 1, 500])("draws %i rows without changing how the page reads", async (rows) => {
    const { container } = renderSettings({ rows });
    const table = await screen.findByRole("table", { name: MEMBERS_CAPTION });
    // The skeleton says it is busy without the word: the rows come after the permissions do.
    await waitFor(() => {
      expect(within(table).queryByText(en.app.loading)).not.toBeInTheDocument();
      expect(table).not.toHaveAttribute("aria-busy", "true");
    });
    const body = table.querySelector("tbody");
    expect(body?.querySelectorAll("tr")).toHaveLength(Math.max(rows, 1));
    expectHeadingOutline(container);
  });

  // UI-16: the keyboard reaches the tab list first, then the tab's own action, and nothing takes
  // focus on arrival.
  it("is reachable by keyboard: the settings search, the selected tab, then the tab's action", async () => {
    const { container } = renderSettings();
    await screen.findByRole("button", { name: en.access.roles.grant });
    expect(document.activeElement).toBe(document.body);
    const reached = await tabOrder(container);
    // T-3277: finding a setting comes first, above every tab.
    expect(reached[0]).toBe(screen.getByRole("searchbox", { name: en.projectSettings.search.label }));
    expect(reached[1]).toBe(screen.getByRole("tab", { name: en.projectSettings.tab.members }));
    expect(reached.indexOf(screen.getByRole("button", { name: en.access.roles.grant }))).toBe(2);
  });

  // UI-11: every string of the page comes from the bundle, in all four languages.
  it("says the same things in every language", async () => {
    await inEveryLocale(async (locale) => {
      cleanup();
      renderSettings({ rows: 0 });
      const title = i18n.t("projectSettings.title");
      expect(await screen.findByRole("heading", { level: 1, name: title })).toBeInTheDocument();
      expect(
        screen.getByText(i18n.t("projectSettings.lead", { project: PROJECT })),
        `the lead is missing in ${locale}`,
      ).toBeInTheDocument();
      if (locale !== "en") {
        expect(title, `${locale} still shows the English title`).not.toBe(en.projectSettings.title);
      }
    });
  });

  // PF-50: a name that arrived as markup is read as text, never parsed.
  it("renders a role holder that arrived as markup as text", async () => {
    renderPage(<ProjectSettingsPage project={PROJECT} tab="members" />, {
      path: `/projects/${PROJECT}/settings/members`,
      answer: (url) =>
        url.pathname.endsWith("/rolebindings")
          ? json(list([binding("x", "<img src=x onerror=alert(1)>", "domain-editor")]))
          : undefined,
    });
    expect(await screen.findByText("<img src=x onerror=alert(1)>")).toBeInTheDocument();
    expect(document.querySelector("img")).toBeNull();
  });

  it.each(["members", "access", "general", "danger"] as const)("has no axe violations on %s", async (tab) => {
    const { container } = renderSettings({ tab, rows: 3 });
    await screen.findAllByRole("heading", { level: 2 });
    await waitFor(() => {
      expect(screen.queryAllByText(en.app.loading)).toHaveLength(0);
    });
    await expectNoAxeViolations(container);
  });

  it("has no axe violations while Members is empty", async () => {
    const { container } = renderSettings({ rows: 0 });
    await screen.findByText(en.access.roles.empty);
    await expectNoAxeViolations(container);
  });

  // The grant form the Members tab's primary action opens is the same form the assistant's draft
  // opens; pressing it from here has to reach it (UI-44, the parity table).
  it("opens the grant form from the Members tab's own action", async () => {
    renderSettings({ rows: 0 });
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: en.access.roles.grant }));
    expect(await screen.findByRole("dialog", { name: en.access.roles.grantTitle })).toBeInTheDocument();
  });
});
