/**
 * T-1796: the workspaces page meets the UI contract (UI-01, UI-11, UI-15, UI-16, UI-44, PF-50).
 *
 * A failed list was drawn as one red word with no reason and nothing to press, beside an empty
 * table — "you have no copies" and "nobody could ask" read the same, and they mean opposite
 * things. The `window.confirm` the survey measured had already gone to `ConfirmDialog`; what was
 * left was the error state, the wait, and a link styled by hand where `buttonClass` exists.
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
import { narrowsAndOrders } from "./grid_checks";

const PATH = "/projects/helsinki/workspaces";

const workspace = (over: Record<string, unknown> = {}) => ({
  name: "ws-air-quality",
  title: "Air quality rework",
  owner: VIEWER.email,
  scope: { kind: "project" },
  expiresAt: "2026-04-01T00:00:00Z",
  previewState: "running",
  ...over,
});

const answering = (items: unknown[]) => (path: string) =>
  path.endsWith("/workspaces") ? jsonResponse(list(items)) : undefined;

describe("the workspaces page", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("opens_under_one_h1_with_its_lead_and_no_axe_violation", async () => {
    const { container } = await renderRoute({ path: PATH, answer: answering([workspace()]) });
    await expectOneH1(en.workspaces.title);
    await screen.findByRole("table");
    await expectAxeClean(container);
  });

  it("a_list_that_failed_says_the_apis_own_sentence_and_offers_one_more_try", async () => {
    const { calls } = await renderRoute({
      path: PATH,
      answer: (path) =>
        path.endsWith("/workspaces")
          ? problem(503, "The workspace store is not answering.")
          : undefined,
    });

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("The workspace store is not answering.");
    // Not "you have no copies", and not the generic sentence that says nothing.
    expect(screen.queryByText(en.workspaces.empty)).toBeNull();
    expect(screen.queryByText(en.app.error.generic)).toBeNull();

    const before = calls().filter((call) => call.endsWith("/workspaces")).length;
    await userEvent.click(within(alert).getByRole("button", { name: en.app.error.retry }));
    await waitFor(() =>
      expect(calls().filter((call) => call.endsWith("/workspaces")).length).toBeGreaterThan(before),
    );
  });

  it("holds_its_layout_while_the_copies_are_on_their_way", async () => {
    await renderRoute({ path: PATH, pending: true });
    const waiting = await screen.findByRole("status", { name: en.app.loading });
    expect(waiting).toHaveAttribute("aria-busy", "true");
    expect(waiting.querySelectorAll("span[aria-hidden='true']").length).toBeGreaterThan(1);
    // Neither the empty state nor a table: the answer has not come yet.
    expect(screen.queryByText(en.workspaces.empty)).toBeNull();
  });

  it("no_copies_yet_says_what_would_make_one", async () => {
    await renderRoute({ path: PATH, answer: answering([]) });
    expect(await screen.findByText(en.workspaces.empty)).toBeInTheDocument();
    expect(screen.getByText(en.workspaces.emptyHint)).toBeInTheDocument();
  });

  it("separates_the_callers_own_copies_from_everyone_elses", async () => {
    await renderRoute({
      path: PATH,
      answer: answering([
        workspace(),
        workspace({ name: "ws-theirs", title: "Traffic", owner: "someone@else.example" }),
      ]),
    });
    expect(await screen.findByRole("heading", { name: en.workspaces.mine })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: en.workspaces.othersTitle })).toBeInTheDocument();
    // Only the caller's own copy offers to be discarded; someone else's is read-only here.
    expect(screen.getAllByRole("button", { name: en.workspaces.discard })).toHaveLength(1);
  });

  // T-6206: the copies are on the shared grid: search, sort by expiry, filter by preview state.
  it("searches_sorts_by_expiry_and_filters_by_preview_state", async () => {
    await renderRoute({
      path: PATH,
      answer: answering([
        workspace(),
        workspace({ name: "ws-noise", title: "Noise", expiresAt: "2026-03-01T00:00:00Z", previewState: "error" }),
        workspace({ name: "ws-parking", title: "Parking", expiresAt: "2026-05-01T00:00:00Z" }),
      ]),
    });
    const table = await screen.findByRole("table");
    await screen.findByText("Parking");
    await narrowsAndOrders(table, {
      search: { label: en.grid.searchIn.replace("{list}", en.workspaces.tableCaption), text: "noise", kept: ["Noise"] },
      sort: { column: en.workspaces.expiresColumn, ascending: ["Noise", "Air quality rework", "Parking"] },
      filter: { value: new RegExp(`^${en.workspaces.previewStates.error}`), kept: ["Noise"] },
    });
  });

  it("discarding_names_the_copy_asks_first_and_is_driven_from_the_keyboard", async () => {
    const { calls } = await renderRoute({ path: PATH, answer: answering([workspace()]) });
    const discard = await screen.findByRole("button", { name: en.workspaces.discard });

    discard.focus();
    await userEvent.keyboard("{Enter}");
    const dialog = await screen.findByRole("alertdialog");
    // The dialog names what is destroyed, rather than asking "are you sure?".
    expect(dialog).toHaveTextContent("ws-air-quality");

    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(calls().filter((call) => call.startsWith("DELETE"))).toEqual([]);
  });

  it("survives_0_1_and_500_copies", async () => {
    for (const count of [0, 1, 500]) {
      const items = Array.from({ length: count }, (_, index) =>
        workspace({ name: `ws-${index}`, title: `Copy ${index}` }),
      );
      const { unmount } = await renderRoute({ path: PATH, answer: answering(items) });
      if (count === 0) {
        expect(await screen.findByText(en.workspaces.empty)).toBeInTheDocument();
      } else {
        const table = await screen.findByRole("table");
        await waitFor(() => expect(within(table).getAllByRole("row").length).toBe(Math.min(count, 50) + 1));
      }
      unmount();
      vi.restoreAllMocks();
    }
  });

  it("every_row_action_is_a_shared_control_in_dom_order", async () => {
    await renderRoute({ path: PATH, answer: answering([workspace()]) });
    const table = await screen.findByRole("table");
    const row = within(table).getAllByRole("row")[1];
    const actions = [
      within(row).getByRole("button", { name: en.workspaces.openAction }),
      within(row).getByRole("link", { name: en.workspaces.compareAction }),
      within(row).getByRole("button", { name: en.workspaces.discard }),
    ];
    // The compare link used to be styled by hand; all three now carry the shared focus ring.
    for (const action of actions) {
      expect(action).toHaveClass("focus-ring");
    }
    actions[0].focus();
    await userEvent.tab();
    expect(actions[1]).toHaveFocus();
    await userEvent.tab();
    expect(actions[2]).toHaveFocus();
  });

  it("says_everything_it_says_in_all_four_languages", async () => {
    for (const locale of LOCALES) {
      const { unmount } = await renderRoute({ path: PATH, locale, answer: answering([]) });
      await expectOneH1(i18n.t("workspaces.title"));
      expect(await screen.findByText(i18n.t("workspaces.empty"))).toBeInTheDocument();
      if (locale !== "en") {
        expect(screen.queryByText(en.workspaces.emptyHint)).toBeNull();
      }
      unmount();
      vi.restoreAllMocks();
    }
  });

  it("paints_in_the_installations_own_colours_and_renders_a_title_as_text", async () => {
    const { container } = await renderRoute({
      path: PATH,
      brand: OTHER_BRAND,
      answer: answering([workspace({ title: "<img src=x onerror=alert(1)>" })]),
    });
    const table = await screen.findByRole("table");
    expect(await within(table).findByText("<img src=x onerror=alert(1)>")).toBeInTheDocument();
    expect(table.querySelector("img")).toBeNull();
    const literal = [...container.querySelectorAll<HTMLElement>("[style]")].filter((element) =>
      /#[0-9a-f]{3,8}|\brgba?\(/i.test(element.getAttribute("style") ?? ""),
    );
    expect(literal.map((element) => element.outerHTML.slice(0, 120))).toEqual([]);
  });

  // T-2749: the address the sidebar's "new" pattern suggests crashed on 'reading title'.
  it("its_new_address_opens_the_work_on_a_copy_dialog_and_closing_returns_to_the_list", async () => {
    const user = userEvent.setup();
    await renderRoute({ path: `${PATH}/new`, answer: answering([workspace()]) });
    const dialog = await screen.findByRole("dialog", { name: en.workspaces.open.title });
    expect(within(dialog).getByText(en.workspaces.open.scopeProject)).toBeInTheDocument();
    await user.keyboard("{Escape}");
    await waitFor(() => expect(window.location.pathname).toBe(PATH));
    expect(screen.queryByRole("dialog")).toBeNull();
    // The list was under the dialog all along, not a page of its own.
    expect(screen.getByRole("table")).toBeInTheDocument();
  });

  it("its_new_address_opens_no_dialog_for_a_role_that_may_not_propose", async () => {
    await renderRoute({
      path: `${PATH}/new`,
      answer: answering([workspace()]),
      permissions: { project: "helsinki", bootstrap: false, grants: [] },
    });
    await screen.findByRole("table");
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(screen.getByRole("button", { name: en.workspaces.new })).toHaveAttribute("aria-disabled", "true");
  });
});
