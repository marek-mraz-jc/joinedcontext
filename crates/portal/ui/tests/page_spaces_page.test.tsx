/**
 * T-1795: the Context Spaces page meets the UI contract (UI-01, UI-11, UI-15, UI-16, UI-44, PF-50).
 *
 * The survey measured one `href` out of a manifest; it already goes through `SourceLink`, which
 * is `safeHref`, so the case below holds that rather than changing it. What the checklist found
 * instead: the Add button went hard-disabled when the project's quota was full, with the reason
 * only in a banner above it — out of reach of anybody who cannot see the button go grey.
 */
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { USAGE_TIMEOUT_MS } from "../src/api/spaceUsage";
import { expectDenied } from "./checks";
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

const PATH = "/projects/helsinki/spaces";

const space = (name: string, over: Record<string, unknown> = {}) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ContextSpace",
  metadata: { name, namespace: "helsinki" },
  spec: { dataModelRef: "AirQualityObserved" },
  status: { phase: "Live", ...(over.status as object) },
  ...over,
});

/** `GET /api/v1/projects/{project}` is where the counts come from (PF-75). */
const project = (used: number, limit?: number) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Project",
  metadata: { name: "helsinki" },
  status: {
    usage: {
      contextSpaces: { used, limit },
      residentPipelines: { used: 1, limit: 4 },
      publicEndpoints: { used: 1, limit: 3 },
    },
  },
});

const answering =
  (items: unknown[], detail = project(1)) =>
  (path: string) => {
    if (path.endsWith("/spaces")) return jsonResponse(list(items));
    if (path === "/api/v1/projects/helsinki") return jsonResponse(detail);
    return undefined;
  };

describe("the Context Spaces page", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("opens_under_one_h1_with_its_lead_and_no_axe_violation", async () => {
    const { container } = await renderRoute({
      path: PATH,
      answer: answering([space("air")]),
    });
    await expectOneH1(en.spaces.title);
    expect(screen.getByText(en.spaces.lead)).toBeInTheDocument();
    await screen.findByRole("table");
    await expectAxeClean(container);
  });

  it("a_full_quota_refuses_the_add_button_on_the_button_and_keeps_it_reachable", async () => {
    await renderRoute({
      path: PATH,
      answer: answering([space("air")], project(3, 3)),
    });
    const add = await screen.findByRole("button", { name: en.spaces.add });
    await waitFor(() =>
      expectDenied(add, i18n.t("quota.exceeded", { limit: 3 })),
    );
    // The banner stays too, for whoever reads the page rather than the control.
    expect(
      screen
        .getAllByRole("status")
        .some((node) => node.textContent?.includes("3")),
    ).toBe(true);
  });

  it("a_quota_with_room_leaves_the_add_button_alone", async () => {
    await renderRoute({
      path: PATH,
      answer: answering([space("air")], project(1, 3)),
    });
    const add = await screen.findByRole("button", { name: en.spaces.add });
    await waitFor(() => expect(add).toBeEnabled());
    expect(add).not.toHaveAttribute("aria-disabled");
  });

  it("the_list_header_offers_only_adding_a_space_not_per_space_or_import_actions", async () => {
    // Completing acts on one space and an upload is an import (T-6084): each has its own page.
    await renderRoute({ path: PATH, answer: answering([space("air")], project(1, 3)) });
    await screen.findByRole("button", { name: en.spaces.add });
    expect(screen.queryByRole("link", { name: en.spaces.complete.title })).toBeNull();
    expect(screen.queryByRole("link", { name: en.spaces.upload.open })).toBeNull();
  });

  it("holds_its_layout_while_the_spaces_are_on_their_way", async () => {
    await renderRoute({ path: PATH, pending: true });
    const table = await screen.findByRole("table");
    expect(table).toHaveAttribute("aria-busy", "true");
    expect(screen.queryByText(en.spaces.empty)).toBeNull();
  });

  it("says_why_the_spaces_could_not_be_read_and_offers_one_more_try", async () => {
    const { calls } = await renderRoute({
      path: PATH,
      answer: (path) =>
        path.endsWith("/spaces")
          ? problem(503, "The spaces could not be read right now.")
          : undefined,
    });
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("The spaces could not be read right now.");
    expect(screen.queryByText(en.spaces.empty)).toBeNull();

    const before = calls().filter((call) => call.endsWith("/spaces")).length;
    await userEvent.click(
      within(alert).getByRole("button", { name: en.app.error.retry }),
    );
    await waitFor(() =>
      expect(
        calls().filter((call) => call.endsWith("/spaces")).length,
      ).toBeGreaterThan(before),
    );
  });

  it("no_space_yet_offers_the_same_add_action_as_the_header", async () => {
    await renderRoute({ path: PATH, answer: answering([]) });
    expect(await screen.findByText(en.spaces.empty)).toBeInTheDocument();
    // The same control twice, so the first one is where a person is looking (UI-01).
    expect(screen.getAllByRole("button", { name: en.spaces.add })).toHaveLength(
      2,
    );
  });

  // T-2889: how big each space is, from the broker's count (API/01 §28), sortable by it.
  it("shows_each_spaces_entity_count_and_sorts_the_biggest_first", async () => {
    const counts: Record<string, number> = {
      air: 1200,
      bikes: 14232,
      noise: 0,
    };
    await renderRoute({
      path: PATH,
      answer: (path) => {
        const usage = /\/spaces\/([a-z-]+)\/usage$/.exec(path);
        if (usage)
          return jsonResponse({
            entities: counts[usage[1]],
            observedAt: "2026-09-25T11:40:00Z",
          });
        return answering([space("air"), space("bikes"), space("noise")])(path);
      },
    });
    const table = await screen.findByRole("table");
    await waitFor(() =>
      expect(within(table).getAllByTestId("space-entities")).toHaveLength(3),
    );
    const shown = () =>
      within(table)
        .getAllByTestId("space-entities")
        .map((cell) => cell.textContent);
    expect(shown()).toEqual(["1,200", "14,232", "0"]);

    // The grid's own sort (T-6169): ascending, then descending, then the list order.
    const sort = within(table).getByRole("button", {
      name: en.grid.sortBy.replace("{column}", en.spaces.field.entities),
    });
    const header = sort.closest("th") as HTMLElement;
    expect(header).not.toHaveAttribute("aria-sort");
    await userEvent.click(sort);
    expect(header).toHaveAttribute("aria-sort", "ascending");
    expect(shown()).toEqual(["0", "1,200", "14,232"]);
    await userEvent.click(sort);
    expect(header).toHaveAttribute("aria-sort", "descending");
    expect(shown()).toEqual(["14,232", "1,200", "0"]);
    await userEvent.click(sort);
    expect(shown()).toEqual(["1,200", "14,232", "0"]);
  });

  // T-2890: what each space takes in the broker's store, in binary units, exact in the tooltip.
  it("shows_each_spaces_size_in_binary_units_and_a_dash_where_the_broker_measured_none", async () => {
    const bytes: Record<string, unknown> = {
      air: {
        entities: 1_048_576,
        attrInstances: 524_288,
        byType: { "https://x/Air": 1_048_576 },
      },
      bikes: { entities: 900, attrInstances: 0, byType: {} },
    };
    await renderRoute({
      path: PATH,
      answer: (path) => {
        const usage = /\/spaces\/([a-z-]+)\/usage$/.exec(path);
        if (usage) {
          return jsonResponse({
            entities: 3,
            broker: "main",
            observedAt: "2026-09-25T11:40:00Z",
            bytes: bytes[usage[1]],
          });
        }
        return answering([space("air"), space("bikes"), space("noise")])(path);
      },
    });
    const table = await screen.findByRole("table");
    // Owner 2026-10-30: the current state and the history each show their size, under their own count.
    await waitFor(() => expect(within(table).getAllByTestId("space-size")).toHaveLength(3));
    const sizes = within(table).getAllByTestId("space-size");
    expect(sizes.slice(0, 2).map((cell) => cell.textContent)).toEqual(["1 MiB", "900 B"]);
    expect(sizes[0]).toHaveAttribute("title", `${en.spaces.field.size}: 1,048,576 B`);
    expect(sizes[2]).toHaveAttribute("title", en.spaces.sizeMissing);
    const history = within(table).getAllByTestId("space-history-size");
    expect(history.slice(0, 2).map((cell) => cell.textContent)).toEqual(["512 KiB", "0 B"]);
    expect(history[2]).toHaveAttribute("title", en.spaces.sizeMissing);
    expect(within(table).getByRole("columnheader", { name: en.spaces.field.history })).toBeInTheDocument();
  });

  it("says_why_a_count_is_missing_and_never_shows_a_zero_for_it", async () => {
    await renderRoute({
      path: PATH,
      answer: (path) =>
        path.endsWith("/usage")
          ? problem(503, "the broker did not answer; try again in a minute")
          : answering([space("air")])(path),
    });
    const table = await screen.findByRole("table");
    const reason = i18n.t("spaces.entitiesUnknown", {
      reason: "the broker did not answer; try again in a minute",
    });
    expect(await within(table).findByText(reason)).toBeInTheDocument();
    expect(within(table).queryByTestId("space-entities")).toBeNull();
  });

  it("counts_with_a_quiet_bar_and_gives_up_on_a_count_that_never_comes", async () => {
    // T-4225 (photo audit T-4163 #9): "Loading..." stood in monospace in the column for 20 s.
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      await renderRoute({
        path: PATH,
        answer: (path, request) =>
          path.endsWith("/usage")
            ? new Promise<Response>((_, reject) =>
                request.signal.addEventListener("abort", () =>
                  reject(request.signal.reason),
                ),
              )
            : answering([space("air")])(path),
      });
      const table = await screen.findByRole("table");
      await waitFor(() =>
        expect(
          within(table).getByText(en.spaces.entitiesCounting),
        ).toBeInTheDocument(),
      );
      expect(within(table).queryByText(en.app.loading)).toBeNull();
      await vi.advanceTimersByTimeAsync(USAGE_TIMEOUT_MS + 10);
      expect(
        await within(table).findByText(i18n.t("spaces.entitiesSlow")),
      ).toBeInTheDocument();
    } finally {
      vi.useRealTimers();
    }
  });

  it("shows_a_count_the_portal_gives_after_its_20_s_wait_for_the_broker", async () => {
    // T-4825: the page gave up at 15 s while the Portal waited 20 s for a slow broker count.
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      await renderRoute({
        path: PATH,
        answer: (path, request) =>
          path.endsWith("/usage")
            ? new Promise<Response>((resolve, reject) => {
                request.signal.addEventListener("abort", () =>
                  reject(request.signal.reason),
                );
                setTimeout(
                  () =>
                    resolve(
                      jsonResponse({
                        entities: 1234,
                        broker: "main",
                        observedAt: "2026-10-20T19:00:00Z",
                      }),
                    ),
                  19_500,
                );
              })
            : answering([space("air")])(path),
      });
      const table = await screen.findByRole("table");
      await vi.advanceTimersByTimeAsync(19_600);
      expect(
        await within(table).findByTestId("space-entities"),
      ).toHaveTextContent("1,234");
    } finally {
      vi.useRealTimers();
    }
  });

  it("survives_0_1_and_500_rows", async () => {
    for (const count of [0, 1, 500]) {
      const items = Array.from({ length: count }, (_, index) =>
        space(`space-${index}`),
      );
      const { unmount } = await renderRoute({
        path: PATH,
        answer: answering(items),
      });
      if (count === 0) {
        // An empty list is one card with the way to the first space, no table (T-6115).
        expect(await screen.findByText(en.spaces.empty)).toBeInTheDocument();
        expect(screen.queryByRole("table")).toBeNull();
      } else {
        const table = await screen.findByRole("table");
        // The grid shows 50 rows and offers the rest (T-6115).
        await waitFor(() =>
          expect(within(table).getAllByRole("row").length).toBe(Math.min(count, 50) + 1),
        );
      }
      unmount();
      vi.restoreAllMocks();
    }
  });

  it("a_source_address_out_of_a_manifest_is_a_link_only_when_it_could_navigate", async () => {
    await renderRoute({
      path: PATH,
      answer: answering([
        space("air", {
          status: {
            phase: "Live",
            sourceUrl: "https://git.example.sk/hel/x.yaml",
          },
        }),
        space("traffic", {
          status: { phase: "Live", sourceUrl: "javascript:alert(1)" },
        }),
      ]),
    });
    const table = await screen.findByRole("table");
    await within(table).findByText("air");
    // http and https navigate; anything else is data a forge could have written (PF-50).
    expect(
      within(table)
        .getAllByRole("link")
        .map((link) => link.getAttribute("href"))
        .filter((href) => href?.startsWith("javascript:")),
    ).toEqual([]);
  });

  it("says_everything_it_says_in_all_four_languages", async () => {
    for (const locale of LOCALES) {
      const { unmount } = await renderRoute({
        path: PATH,
        locale,
        answer: answering([]),
      });
      await expectOneH1(i18n.t("spaces.title"));
      expect(
        await screen.findByText(i18n.t("spaces.empty")),
      ).toBeInTheDocument();
      if (locale !== "en") {
        expect(screen.queryByText(en.spaces.lead)).toBeNull();
      }
      unmount();
      vi.restoreAllMocks();
    }
  });

  it("paints_in_the_installations_own_colours_not_a_literal", async () => {
    const { container } = await renderRoute({
      path: PATH,
      brand: OTHER_BRAND,
      answer: answering([space("air")]),
    });
    await screen.findByRole("table");
    const literal = [
      ...container.querySelectorAll<HTMLElement>("[style]"),
    ].filter((element) =>
      /#[0-9a-f]{3,8}|\brgba?\(/i.test(element.getAttribute("style") ?? ""),
    );
    expect(
      literal
        .filter((element) => element.getAttribute("aria-hidden") !== "true")
        .map((e) => e.outerHTML.slice(0, 120)),
    ).toEqual([]);
  });
});
