/**
 * T-2288: the card headers that still painted Edit and Delete loose (UI-26, UI-44, UI-71).
 *
 * T-2287 put every table row's four manifest actions behind one menu; these three surfaces are
 * not table rows, so they kept their pair in the open — the dashboard header beside a chooser
 * and two "new" buttons, a sync source's header, an app card under the actions the card exists
 * for. Each one keeps what it is for in the open and nothing else, and a role that may not
 * propose or delete still reads why, because the item stays in the menu with its reason.
 */
import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { LOCALES, jsonResponse, list, renderRoute } from "./pageHarness";
import { findFormPage } from "./formPage";

const PROJECT = "helsinki";

function manifest(kind: string, name: string, spec: Record<string, unknown> = {}, extra: Record<string, unknown> = {}) {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind,
    metadata: { name, namespace: PROJECT },
    spec,
    ...extra,
  };
}

const DASHBOARD = manifest("Dashboard", "air", {
  title: { en: "Air quality" },
  pages: [{ title: "Map", widgets: [] }],
});

const SYNC_SOURCE = manifest("SyncSource", "regional-datamodels", {
  source: { git: { url: "https://git.region.sk/udp/datamodels.git", ref: "main", path: "models" } },
  schedule: { interval: "30m" },
  mode: "mirror",
});

const APP = manifest(
  "App",
  "air-map",
  { title: { en: "Air map" }, lifecycle: "preview" },
  { status: { phase: "Live" } },
);

/** Everything the three pages read, answered from one table. */
function answer(path: string): Response | undefined {
  if (path.endsWith("/dashboards")) return jsonResponse(list([DASHBOARD]));
  if (path.endsWith("/syncsources")) return jsonResponse(list([SYNC_SOURCE]));
  if (path.endsWith("/apps")) return jsonResponse(list([APP]));
  if (path.includes("/syncsources/") && path.endsWith("/status")) {
    return jsonResponse({
      project: PROJECT,
      name: "regional-datamodels",
      phase: "Live",
      observedRevision: "abc1234",
      lastRunAt: 1757000000,
      mergeRequest: null,
      lastError: null,
      paused: false,
      durable: true,
    });
  }
  return undefined;
}

/** A role that may read and nothing else, so every menu item names its refusal. */
const READ_ONLY = { project: PROJECT, bootstrap: false, grants: [] };

const MENU = /More actions for/;

/** The four the shared menu carries, in the order it lists them. */
const MENU_ITEMS = [
  en.resourceEdit.button,
  en.saveAs.button,
  en.workspaces.open.action,
  en.resourceDelete.button,
];

async function openMenu(scope: HTMLElement): Promise<HTMLElement> {
  await userEvent.click(within(scope).getByRole("button", { name: MENU }));
  return await screen.findByRole("menu");
}

describe("the card headers that still carried a loose pair", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("leaves the dashboard header its chooser and its new buttons, and nothing that acts on this dashboard", async () => {
    await renderRoute({ path: `/projects/${PROJECT}/dashboards`, answer });
    await screen.findByRole("heading", { name: "Air quality", level: 1 });
    const main = screen.getByRole("main");

    expect(within(main).queryByRole("button", { name: en.dashboards.edit })).toBeNull();
    expect(within(main).queryByRole("button", { name: en.resourceDelete.button })).toBeNull();
    // What makes something new is the page's own action and stays where it was.
    expect(within(main).getByRole("button", { name: en.dashboards.add })).toBeInTheDocument();
    expect(within(main).getByRole("button", { name: MENU })).toBeInTheDocument();
  });

  it("lists the dashboard's four actions in order and opens its editor from the menu", async () => {
    await renderRoute({ path: `/projects/${PROJECT}/dashboards`, answer });
    await screen.findByRole("heading", { name: "Air quality", level: 1 });
    const menu = await openMenu(document.body);
    expect(within(menu).getAllByRole("menuitem").map((item) => item.textContent?.trim())).toEqual(
      MENU_ITEMS,
    );

    await userEvent.click(within(menu).getByRole("menuitem", { name: en.resourceEdit.button }));
    expect(await findFormPage()).toBeInTheDocument();
  });

  it("keeps a sync source's header to one control, with its three own actions below", async () => {
    await renderRoute({ path: `/projects/${PROJECT}/syncsources`, answer });
    const heading = await screen.findByRole("heading", { name: "regional-datamodels", level: 2 });
    const header = heading.closest("header") as HTMLElement;

    expect(within(header).getAllByRole("button")).toHaveLength(1);
    expect(within(header).getByRole("button", { name: MENU })).toBeInTheDocument();
  });

  it("keeps no Open button on the app card and every action in its one menu (T-2618, T-3038)", async () => {
    await renderRoute({ path: `/projects/${PROJECT}/apps`, answer });
    const card = (await screen.findByRole("heading", { name: "Air map" })).closest(
      "li",
    ) as HTMLElement;

    expect(
      within(card).queryByRole("button", { name: new RegExp(`^${en.apps.openAction}`) }),
    ).toBeNull();
    expect(within(card).queryByRole("button", { name: en.apps.previewAction })).toBeNull();
    expect(within(card).queryByRole("button", { name: en.resourceEdit.button })).toBeNull();
    const menu = await openMenu(card);
    // The app's own actions first, then the manifest's four in the same menu.
    expect(
      within(menu)
        .getAllByRole("menuitem")
        .slice(-MENU_ITEMS.length)
        .map((item) => item.textContent?.trim()),
    ).toEqual(MENU_ITEMS);
    expect(
      within(menu).getByRole("menuitem", { name: new RegExp(`^${en.apps.previewAction}`) }),
    ).toBeInTheDocument();
  });

  it("tells a role that may not propose or delete why, rather than dropping the item", async () => {
    await renderRoute({ path: `/projects/${PROJECT}/apps`, answer, permissions: READ_ONLY });
    const card = (await screen.findByRole("heading", { name: "Air map" })).closest("li") as HTMLElement;
    const menu = await openMenu(card);

    for (const [label, verb] of [
      [en.resourceEdit.button, "propose"],
      [en.saveAs.button, "propose"],
      [en.resourceDelete.button, "delete"],
    ] as const) {
      const item = within(menu).getByRole("menuitem", { name: new RegExp(label) });
      const reason = en.permissions.denied
        .replace("{verb}", verb)
        .replace("{kind}", "App")
        .replaceAll("''", "'");
      expect(item).toHaveAttribute("aria-disabled", "true");
      expect(item).toHaveAttribute("title", reason);
      // And the sentence is in the item itself, so a screen reader reads it with the action.
      expect(item).toHaveTextContent(reason);
    }
  });

  it("names the menu and the manifest's four actions in every locale the organisation offers", async () => {
    for (const locale of LOCALES) {
      const view = await renderRoute({ path: `/projects/${PROJECT}/apps`, answer, locale });
      const card = (await screen.findByRole("heading", { name: "Air map" })).closest("li") as HTMLElement;
      const trigger = within(card).getByRole("button", {
        name: new RegExp(i18n.t("rowActions.more", { name: "" }).trim().split("{")[0]),
      });
      await userEvent.click(trigger);
      const menu = await screen.findByRole("menu");
      // The app's own actions come first (T-2618); the manifest's four close the menu.
      expect(
        within(menu)
          .getAllByRole("menuitem")
          .slice(-4)
          .map((item) => item.textContent?.trim()),
      ).toEqual([
        i18n.t("resourceEdit.button"),
        i18n.t("saveAs.button"),
        i18n.t("workspaces.open.action"),
        i18n.t("resourceDelete.button"),
      ]);
      view.unmount();
      vi.unstubAllGlobals();
    }
    await i18n.changeLanguage("en");
  });
});
