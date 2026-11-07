// covers (T-2137, the module gate in gate_modules.test.ts): src/pages/projectSettings/settingsIndex.ts.
/**
 * T-3277: every setting of a project is findable by what it is called or what it changes, in the
 * person's language, and each result names the tab that holds it.
 */
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { PROJECT_SETTINGS_TABS, ProjectSettingsPage } from "../src/pages/projectSettings/ProjectSettingsPage";
import { findSettings, settingsIndex } from "../src/pages/projectSettings/settingsIndex";
import { QUOTA_DIMENSIONS } from "../src/schemas/kinds";
import { InRouter } from "./pageHarness";

beforeEach(async () => {
  await i18n.changeLanguage("en");
});
afterEach(() => {
  vi.unstubAllGlobals();
});

describe("the settings index (T-3277)", () => {
  it("holds every tab and every quota, each saying what it changes, in every language", async () => {
    for (const language of ["en", "sk", "cs", "de"]) {
      await i18n.changeLanguage(language);
      const index = settingsIndex(i18n.t.bind(i18n), language);
      expect(new Set(index.map((s) => s.tab))).toEqual(new Set(PROJECT_SETTINGS_TABS));
      for (const dimension of QUOTA_DIMENSIONS) {
        expect(index.some((s) => s.key === `quotas.${dimension}`), dimension).toBe(true);
      }
      for (const setting of index) {
        expect(setting.label, `${language} ${setting.key}`).not.toBe("");
        expect(setting.about, `${language} ${setting.key} says what it changes`).not.toBe("");
      }
    }
  });

  it("finds by every word, without accents, and nothing for an empty query", async () => {
    await i18n.changeLanguage("sk");
    const index = settingsIndex(i18n.t.bind(i18n), "sk");
    expect(findSettings(index, "odovzdat").map((s) => s.key)).toEqual(["handover"]);
    expect(findSettings(index, "   ")).toEqual([]);
    await i18n.changeLanguage("en");
    const english = settingsIndex(i18n.t.bind(i18n), "en");
    expect(findSettings(english, "delete project").map((s) => s.key)).toContain("delete");
    expect(findSettings(english, "nothing like this at all")).toEqual([]);
  });
});

describe("the settings search on the page (T-3277)", () => {
  function show() {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => new Response(JSON.stringify({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] }), { headers: { "Content-Type": "application/json" } })),
    );
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <I18nextProvider i18n={i18n}>
          <InRouter>
            <ProjectSettingsPage project="helsinki" tab="general" />
          </InRouter>
        </I18nextProvider>
      </QueryClientProvider>,
    );
  }

  it("lists what it finds with the tab that holds each, and says so when nothing matches", async () => {
    show();
    const search = await screen.findByRole("searchbox", { name: en.projectSettings.search.label });
    await userEvent.type(search, "hand over");
    const results = screen.getByRole("list", { name: en.projectSettings.search.results });
    const link = within(results).getByRole("link", { name: en.projectSettings.danger.handoverTitle });
    expect(link).toHaveAttribute("href", "/projects/helsinki/settings/danger");
    expect(results).toHaveTextContent(en.projectSettings.tab.danger);
    await userEvent.clear(search);
    await userEvent.type(search, "zzzz");
    expect(screen.getByRole("status")).toHaveTextContent(en.projectSettings.search.nothing);
  });
});
