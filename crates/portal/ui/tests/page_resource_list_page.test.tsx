/**
 * T-1794: the generic kind page meets the UI contract (UI-01, UI-11, UI-15, UI-16, UI-44, PF-50).
 *
 * `/projects/{project}/{plural}` for a kind with no page of its own used to render a bare table
 * whose caption was the URL segment: no heading, nothing that said which kind was on screen, and
 * nothing translated. These cases are what it owes a person instead.
 */
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import sk from "../src/locales/sk.json";
import {
  expectAxeClean,
  expectOneH1,
  jsonResponse,
  list,
  LOCALES,
  OTHER_BRAND,
  problem,
  renderRoute,
} from "./pageHarness";

const PATH = "/projects/helsinki/blueprints";

const blueprint = (name: string) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Blueprint",
  metadata: { name, title: { en: `Blueprint ${name}`, sk: `Vzor ${name}` } },
  spec: { version: "1.0.0" },
  status: { phase: "Live" },
});

const answering = (items: unknown[]) => (path: string) =>
  path.endsWith("/blueprints") ? jsonResponse(list(items)) : undefined;

describe("a kind with no page of its own", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("names_the_kind_in_one_h1_and_in_the_table_caption_rather_than_the_url_segment", async () => {
    const { container } = await renderRoute({ path: PATH, answer: answering([blueprint("alerting")]) });

    await expectOneH1("Blueprints");
    const table = await screen.findByRole("table");
    // `caption` is what a screen reader announces when it enters the table; "blueprints" out of
    // the address bar is not a sentence anybody wrote.
    expect(within(table).getByText("Blueprints")).toBeInTheDocument();
    expect(screen.queryByText("blueprints")).toBeNull();
    await expectAxeClean(container);
  });

  it("reads_a_kind_the_navigation_has_no_word_for_as_words", async () => {
    await renderRoute({
      path: "/projects/helsinki/service-accounts",
      answer: (path) => (path.endsWith("/service-accounts") ? jsonResponse(list([])) : undefined),
    });
    await expectOneH1("Service accounts");
  });

  it("holds_its_layout_while_the_list_is_on_its_way", async () => {
    await renderRoute({ path: PATH, pending: true });
    // The table is already there with its head, so nothing jumps when the rows land.
    const table = await screen.findByRole("table");
    expect(table).toHaveAttribute("aria-busy", "true");
    expect(within(table).getByRole("columnheader", { name: en.resourceList.name })).toBeInTheDocument();
  });

  it("says_why_the_list_could_not_be_read_and_offers_one_more_try", async () => {
    const { calls } = await renderRoute({
      path: PATH,
      answer: (path) =>
        path.endsWith("/blueprints")
          ? problem(503, "The blueprints could not be read right now.")
          : undefined,
    });

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("The blueprints could not be read right now.");
    // Not the empty state: a list that failed is not "there is nothing here" (T-1763).
    expect(screen.queryByText(en.resourceList.empty)).toBeNull();

    const before = calls().filter((call) => call.endsWith("/blueprints")).length;
    await userEvent.click(within(alert).getByRole("button", { name: en.app.error.retry }));
    await waitFor(() =>
      expect(calls().filter((call) => call.endsWith("/blueprints")).length).toBeGreaterThan(before),
    );
  });

  it("an_empty_kind_says_what_would_fill_it", async () => {
    await renderRoute({ path: PATH, answer: answering([]) });
    expect(await screen.findByText(en.resourceList.empty)).toBeInTheDocument();
    // Blueprint has a form since T-1547, so its empty list names the New it offers.
    expect(screen.getByText(i18n.t("resourceList.emptyHintCreate", { new: "New blueprint" }))).toBeInTheDocument();
  });

  it("survives_0_1_and_500_rows_and_keeps_one_row_action_each", async () => {
    for (const count of [0, 1, 500]) {
      const items = Array.from({ length: count }, (_, index) => blueprint(`bp-${index}`));
      const { unmount } = await renderRoute({ path: PATH, answer: answering(items) });
      if (count === 0) {
        // An empty list is one card saying what to do next, no table (T-6115, T-6169).
        expect(await screen.findByText(i18n.t("resourceList.empty"))).toBeInTheDocument();
        expect(screen.queryByRole("table")).toBeNull();
      } else {
        const table = await screen.findByRole("table");
        // The head is a row too; the grid shows 50 rows and offers the rest.
        await waitFor(() => expect(within(table).getAllByRole("row").length).toBe(Math.min(count, 50) + 1));
        // One menu per row beside the grid's Details button.
        expect(within(table).getAllByRole("button", { name: /More actions/ })).toHaveLength(Math.min(count, 50));
      }
      unmount();
      vi.restoreAllMocks();
    }
  });

  it("reads_its_rows_page_by_page_from_the_server_and_searches_there", async () => {
    // T-6175, API/13 section 2: the server pages (limit, cursor), searches (q) and counts
    // (totalCount); the grid neither loads the whole kind nor filters it in the browser.
    const all = Array.from({ length: 120 }, (_, index) => blueprint(`bp-${String(index).padStart(3, "0")}`));
    const asked: URL[] = [];
    const { calls } = await renderRoute({
      path: PATH,
      answer: (path, request) => {
        if (!path.endsWith("/blueprints")) return undefined;
        const url = new URL(request.url, window.location.origin);
        asked.push(url);
        const q = url.searchParams.get("q") ?? "";
        const kept = all.filter((each) => each.metadata.name.includes(q));
        const from = Number(url.searchParams.get("cursor") ?? 0);
        const limit = Number(url.searchParams.get("limit"));
        const next = from + limit < kept.length ? String(from + limit) : undefined;
        return jsonResponse({ ...list(kept.slice(from, from + limit)), nextCursor: next, totalCount: kept.length });
      },
    });
    const table = await screen.findByRole("table");
    await waitFor(() => expect(within(table).getAllByRole("row")).toHaveLength(51));
    expect(asked[0]?.searchParams.get("limit")).toBe("50");
    expect(asked[0]?.searchParams.get("count")).toBe("true");
    expect(asked[0]?.searchParams.has("cursor")).toBe(false);
    expect(screen.getByText(i18n.t("grid.footer", { shown: 50, count: 120 }))).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: i18n.t("grid.showMore", { count: 50 }) }));
    await waitFor(() => expect(within(table).getAllByRole("row")).toHaveLength(101));
    expect(asked.at(-1)?.searchParams.get("cursor")).toBe("50");

    await userEvent.type(screen.getByRole("searchbox"), "bp-11");
    await waitFor(() => expect(asked.at(-1)?.searchParams.get("q")).toBe("bp-11"));
    await waitFor(() => expect(within(screen.getByRole("table")).getAllByRole("row")).toHaveLength(11));
    expect(calls().filter((call) => call.endsWith("/blueprints")).length).toBe(asked.length);
  });

  it("every_row_action_is_reached_and_opened_from_the_keyboard", async () => {
    await renderRoute({ path: PATH, answer: answering([blueprint("alerting")]) });
    const table = await screen.findByRole("table");
    const menu = await within(table).findByRole("button", { expanded: false });

    menu.focus();
    expect(menu).toHaveFocus();
    await userEvent.keyboard("{Enter}");
    await waitFor(() => expect(menu).toHaveAttribute("aria-expanded", "true"));
    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(menu).toHaveAttribute("aria-expanded", "false"));
    expect(menu).toHaveFocus();
  });

  it("says_everything_it_says_in_all_four_languages", async () => {
    for (const locale of LOCALES) {
      const { unmount } = await renderRoute({ path: PATH, locale, answer: answering([]) });
      const heading = await expectOneH1();
      expect(heading.textContent?.trim()).not.toBe("");
      expect(await screen.findByText(i18n.t("resourceList.empty"))).toBeInTheDocument();
      if (locale !== "en") {
        // The lead is written per language; the English one showing through means nobody wrote it.
        expect(screen.queryByText(en.resourceList.emptyHint)).toBeNull();
      }
      unmount();
      vi.restoreAllMocks();
    }
    expect(sk.resourceList.lead).not.toBe(en.resourceList.lead);
  });

  it("paints_in_the_installations_own_colours_not_a_literal", async () => {
    const { container } = await renderRoute({
      path: PATH,
      brand: OTHER_BRAND,
      answer: answering([blueprint("alerting")]),
    });
    await screen.findByRole("table");
    // Nothing this page renders may carry a colour of its own: the brand is the only source.
    const literal = [...container.querySelectorAll<HTMLElement>("[style]")].filter((element) =>
      /#[0-9a-f]{3,8}|\brgba?\(/i.test(element.getAttribute("style") ?? ""),
    );
    expect(literal.map((element) => element.outerHTML.slice(0, 120))).toEqual([]);
  });

  it("renders_a_title_out_of_a_manifest_as_text_and_never_as_markup", async () => {
    const nasty = blueprint("alerting");
    nasty.metadata.title = { en: "<img src=x onerror=alert(1)>", sk: "x" };
    await renderRoute({ path: PATH, answer: answering([nasty]) });
    const table = await screen.findByRole("table");
    expect(await within(table).findByText("<img src=x onerror=alert(1)>")).toBeInTheDocument();
    expect(table.querySelector("img")).toBeNull();
  });
});
