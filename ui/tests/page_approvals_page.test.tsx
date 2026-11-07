/**
 * T-1789: the approvals queue meets the UI contract (UI-01, UI-11, UI-15, UI-16, UI-44, PF-50).
 *
 * The queue's "only mine" filter was a raw `<input type="checkbox">`: no shared focus ring, no
 * shared tick, and a label tied to it only by wrapping. These cases hold the whole page to the
 * contract — the four states, the filters, the keyboard and the four locales.
 */
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import {
  expectAxeClean,
  expectOneH1,
  jsonResponse,
  list,
  LOCALES,
  OTHER_BRAND,
  problem,
  renderRoute,
  VIEWER,
} from "./pageHarness";

const PATH = "/projects/helsinki/approvals";

const change = (over: Record<string, unknown> = {}) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ChangeProposal",
  metadata: { name: "chg-1a2b3c4d", namespace: "helsinki" },
  summary: { key: "change.summary.update", params: { kind: "Endpoint", name: "public-air", fields: 2 } },
  author: { name: "Marek Mráz", email: "marek@banskabystrica.sk" },
  createdAt: "2026-03-03T12:00:00Z",
  status: { lane: "yellow", phase: "PendingApproval", plan: { update: 1 } },
  ...over,
});

const answering = (items: unknown[]) => (path: string) =>
  path.endsWith("/changes") ? jsonResponse(list(items)) : undefined;

describe("the approvals queue", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("opens_under_one_h1_with_its_lead_and_no_axe_violation", async () => {
    const { container } = await renderRoute({ path: PATH, answer: answering([change()]) });
    await expectOneH1(en.approvals.title);
    expect(screen.getByText(en.approvals.lead)).toBeInTheDocument();
    await screen.findByRole("table");
    await expectAxeClean(container);
  });

  it("the_only_mine_filter_is_the_shared_checkbox_and_its_label_ticks_it", async () => {
    await renderRoute({
      path: PATH,
      answer: answering([change(), change({ metadata: { name: "chg-mine" }, author: VIEWER })]),
    });
    const mine = await screen.findByRole("checkbox", { name: en.approvals.filterMine });
    // The shared control, not a bare input: the tick and the focus ring come from one place.
    expect(mine).toHaveClass("size-[1rem]");
    expect(mine).not.toBeChecked();

    // Clicking the words ticks the box, which is the whole point of the label being tied to it.
    await userEvent.click(screen.getByText(en.approvals.filterMine));
    expect(mine).toBeChecked();
    await waitFor(() =>
      expect(within(screen.getByRole("table")).queryByText(/public-air/)).not.toBeNull(),
    );
  });

  it("the_filter_is_reached_and_ticked_from_the_keyboard", async () => {
    await renderRoute({ path: PATH, answer: answering([change()]) });
    const mine = await screen.findByRole("checkbox", { name: en.approvals.filterMine });
    mine.focus();
    expect(mine).toHaveFocus();
    await userEvent.keyboard(" ");
    expect(mine).toBeChecked();
    await userEvent.keyboard(" ");
    expect(mine).not.toBeChecked();
  });

  it("narrowing_to_nothing_says_so_rather_than_that_the_queue_is_empty", async () => {
    await renderRoute({ path: PATH, answer: answering([change()]) });
    await screen.findByRole("table");
    await userEvent.click(await screen.findByRole("checkbox", { name: en.approvals.filterMine }));
    // Somebody else's change is the only one there: "nothing matches" and not "nothing proposed".
    expect(await screen.findByText(en.approvals.noneMatch)).toBeInTheDocument();
    expect(screen.queryByText(en.approvals.empty)).toBeNull();
  });

  it("an_empty_queue_offers_the_way_a_first_change_is_made", async () => {
    await renderRoute({ path: PATH, answer: answering([]) });
    expect(await screen.findByText(en.approvals.empty)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: en.approvals.emptyAction })).toHaveAttribute(
      "href",
      "/projects/helsinki/assistant",
    );
    // No filters over an empty queue: there is nothing to narrow.
    expect(screen.queryByRole("checkbox", { name: en.approvals.filterMine })).toBeNull();
  });

  it("says_why_the_queue_could_not_be_read_and_offers_one_more_try", async () => {
    const { calls } = await renderRoute({
      path: PATH,
      answer: (path) =>
        path.endsWith("/changes") ? problem(503, "The changes could not be read right now.") : undefined,
    });
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("The changes could not be read right now.");
    expect(screen.queryByText(en.approvals.empty)).toBeNull();

    const before = calls().filter((call) => call.endsWith("/changes")).length;
    await userEvent.click(within(alert).getByRole("button", { name: en.app.error.retry }));
    await waitFor(() =>
      expect(calls().filter((call) => call.endsWith("/changes")).length).toBeGreaterThan(before),
    );
  });

  it("holds_its_layout_while_the_queue_is_on_its_way", async () => {
    await renderRoute({ path: PATH, pending: true });
    const table = await screen.findByRole("table");
    expect(table).toHaveAttribute("aria-busy", "true");
    expect(within(table).getByRole("columnheader", { name: en.approvals.summary })).toBeInTheDocument();
  });

  it("survives_0_1_and_500_rows", async () => {
    for (const count of [0, 1, 500]) {
      const items = Array.from({ length: count }, (_, index) =>
        change({ metadata: { name: `chg-${index}` } }),
      );
      const { unmount } = await renderRoute({ path: PATH, answer: answering(items) });
      const table = await screen.findByRole("table");
      await waitFor(() =>
        expect(within(table).getAllByRole("row").length).toBe(count === 0 ? 2 : count + 1),
      );
      unmount();
      vi.restoreAllMocks();
    }
  });

  it("says_everything_it_says_in_all_four_languages", async () => {
    for (const locale of LOCALES) {
      const { unmount } = await renderRoute({ path: PATH, locale, answer: answering([]) });
      await expectOneH1(i18n.t("approvals.title"));
      expect(await screen.findByText(i18n.t("approvals.empty"))).toBeInTheDocument();
      if (locale !== "en") {
        expect(screen.queryByText(en.approvals.emptyHint)).toBeNull();
      }
      unmount();
      vi.restoreAllMocks();
    }
  });

  it("paints_in_the_installations_own_colours_not_a_literal", async () => {
    const { container } = await renderRoute({
      path: PATH,
      brand: OTHER_BRAND,
      answer: answering([change()]),
    });
    await screen.findByRole("table");
    const literal = [...container.querySelectorAll<HTMLElement>("[style]")].filter((element) =>
      /#[0-9a-f]{3,8}|\brgba?\(/i.test(element.getAttribute("style") ?? ""),
    );
    expect(literal.map((element) => element.outerHTML.slice(0, 120))).toEqual([]);
  });

  it("renders_an_author_name_out_of_the_api_as_text_and_never_as_markup", async () => {
    await renderRoute({
      path: PATH,
      answer: answering([
        change({ author: { name: "<img src=x onerror=alert(1)>", email: "a@b.c" } }),
      ]),
    });
    const table = await screen.findByRole("table");
    expect(await within(table).findByText("<img src=x onerror=alert(1)>")).toBeInTheDocument();
    expect(table.querySelector("img")).toBeNull();
  });
});
