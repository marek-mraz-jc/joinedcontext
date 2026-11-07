/**
 * T-1788: the change under review meets the UI contract (UI-01, UI-11, UI-15, UI-16, UI-44, PF-50).
 *
 * The page carried three things the contract refuses: a hand-made button for each file in the
 * bundle, `autoFocus` on the red-lane confirmation — which dropped an approver past the very
 * diff they are there to read — and one word in the corner while the change was loading.
 * `approval_actions.test.tsx` holds the approve and reject behaviour; these are the rest.
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
  LOCALES,
  OTHER_BRAND,
  problem,
  renderRoute,
} from "./pageHarness";

const ROUTE = "/projects/helsinki/approvals/chg-1a2b3c4d";

const proposal = (over: Record<string, unknown> = {}) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ChangeProposal",
  metadata: { name: "chg-1a2b3c4d", namespace: "helsinki" },
  summary: { key: "change.summary.update", params: { kind: "Endpoint", name: "public-air", fields: 2 } },
  author: { name: "Marek Mráz", email: "marek@banskabystrica.sk" },
  createdAt: "2026-03-03T12:00:00Z",
  status: { lane: "yellow", phase: "PendingApproval", plan: { update: 1 } },
  planFields: [{ path: "spec.audience", from: "public", to: "internal" }],
  ...over,
});

const FILES = [
  {
    path: "projects/helsinki/endpoints/public-air.yaml",
    kind: "Endpoint",
    lane: "yellow",
    operation: "Update",
    fields: [{ path: "spec.audience", from: "public", to: "internal" }],
  },
  {
    path: "projects/helsinki/spaces/air.yaml",
    kind: "ContextSpace",
    lane: "green",
    operation: "Update",
    fields: [{ path: "spec.retention", from: "30d", to: "90d" }],
  },
];

const answering = (change: unknown) => (path: string) =>
  path.endsWith("/changes/chg-1a2b3c4d") ? jsonResponse(change) : undefined;

describe("the change under review", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("opens_under_one_h1_that_is_the_summary_and_is_axe_clean", async () => {
    const { container } = await renderRoute({ path: ROUTE, answer: answering(proposal()) });
    await expectOneH1(/public-air/);
    await expectAxeClean(container);
  });

  it("keeps_the_shape_of_the_change_while_it_is_being_read", async () => {
    await renderRoute({ path: ROUTE, pending: true });
    const waiting = await screen.findByRole("status", { name: en.app.loading });
    expect(waiting).toHaveAttribute("aria-busy", "true");
    // A block the size of the change, not one word in the corner: nothing moves when it lands.
    expect(waiting.querySelectorAll("span[aria-hidden='true']").length).toBeGreaterThan(2);
  });

  it("says_why_the_change_could_not_be_read_and_offers_one_more_try", async () => {
    const { calls } = await renderRoute({
      path: ROUTE,
      answer: (path) =>
        path.endsWith("/changes/chg-1a2b3c4d")
          ? problem(503, "The forge is not answering.")
          : undefined,
    });
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("The forge is not answering.");

    const before = calls().filter((call) => call.includes("/changes/chg-")).length;
    await userEvent.click(within(alert).getByRole("button", { name: en.app.error.retry }));
    await waitFor(() =>
      expect(calls().filter((call) => call.includes("/changes/chg-")).length).toBeGreaterThan(before),
    );
  });

  it("each_file_of_a_bundle_is_a_shared_control_reached_and_toggled_from_the_keyboard", async () => {
    await renderRoute({ path: ROUTE, answer: answering(proposal({ files: FILES, fileCount: 2 })) });

    const space = await screen.findByRole("button", { name: "projects/helsinki/spaces/air.yaml" });
    // The shared Button: one focus ring, one size, one disabled rule for the whole Portal.
    expect(space).toHaveClass("focus-ring");
    expect(space).toHaveAttribute("aria-pressed", "false");

    space.focus();
    expect(space).toHaveFocus();
    await userEvent.keyboard("{Enter}");
    await waitFor(() => expect(space).toHaveAttribute("aria-pressed", "true"));
    await userEvent.keyboard("{Enter}");
    await waitFor(() => expect(space).toHaveAttribute("aria-pressed", "false"));
  });

  it("does_not_move_the_focus_on_arrival_even_on_a_red_lane_change", async () => {
    // What `autoFocus` used to do here: land in the confirmation and skip the diff above it.
    await renderRoute({
      path: ROUTE,
      answer: answering(
        proposal({ status: { lane: "red", phase: "PendingApproval", plan: { delete: 1 } } }),
      ),
    });
    const confirm = await screen.findByRole("textbox", { name: en.approvals.confirmLabel });
    expect(confirm).not.toHaveFocus();
    expect(document.activeElement).toBe(document.body);
    // And it is still labelled and described, so a person who tabs to it knows what to type.
    expect(confirm).toHaveAccessibleDescription(/public-air/);
  });

  it("a_merge_request_that_is_not_a_web_address_is_read_as_text_and_never_linked", async () => {
    await renderRoute({
      path: ROUTE,
      answer: answering(
        proposal({
          status: {
            lane: "yellow",
            phase: "PendingApproval",
            plan: { update: 1 },
            // What a forge that has been tampered with could put in the field (PF-50).
            mergeRequest: "javascript:alert(document.cookie)",
          },
        }),
      ),
    });
    expect(await screen.findByText("javascript:alert(document.cookie)")).toBeInTheDocument();
    expect(
      screen.queryByRole("link", { name: /javascript:/ }),
      "a scheme that is not http, https or mailto never becomes an href",
    ).toBeNull();
  });

  it("an_author_name_out_of_the_api_is_rendered_as_text", async () => {
    const { container } = await renderRoute({
      path: ROUTE,
      answer: answering(proposal({ author: { name: "<script>alert(1)</script>", email: "a@b.c" } })),
    });
    expect(await screen.findByText("<script>alert(1)</script>")).toBeInTheDocument();
    expect(container.querySelector("script")).toBeNull();
  });

  it("says_everything_it_says_in_all_four_languages", async () => {
    for (const locale of LOCALES) {
      const { unmount } = await renderRoute({ path: ROUTE, locale, answer: answering(proposal()) });
      expect(
        await screen.findByRole("button", { name: i18n.t("approvals.approve") }),
      ).toBeInTheDocument();
      if (locale !== "en") {
        expect(screen.queryByRole("button", { name: en.approvals.approve })).toBeNull();
      }
      unmount();
      vi.restoreAllMocks();
    }
  });

  it("paints_in_the_installations_own_colours_not_a_literal", async () => {
    const { container } = await renderRoute({
      path: ROUTE,
      brand: OTHER_BRAND,
      answer: answering(proposal({ files: FILES, fileCount: 2 })),
    });
    await screen.findByRole("button", { name: en.approvals.approve });
    const literal = [...container.querySelectorAll<HTMLElement>("[style]")].filter((element) =>
      /#[0-9a-f]{3,8}|\brgba?\(/i.test(element.getAttribute("style") ?? ""),
    );
    expect(literal.map((element) => element.outerHTML.slice(0, 120))).toEqual([]);
  });
});
