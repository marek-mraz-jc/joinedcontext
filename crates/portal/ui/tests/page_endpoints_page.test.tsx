/**
 * T-1791: the endpoints page meets the UI contract (UI-01, UI-11, UI-15, UI-16, UI-44, PF-50).
 *
 * The survey measured five `href`s built from values. All five are `SourceLink` or the page's own
 * `EndpointLink`, and both go through `safeHref`; the cases below are what holds them there
 * rather than a change nobody needed. The one suppressed check that stays is `?edit=<name>`
 * opening the editor from an effect — deriving it during render would mint a fresh slug on every
 * render for an endpoint that has none, so the rule's remedy would be the defect.
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
} from "./pageHarness";
import { findFormPage, queryFormPage } from "./formPage";

const PATH = "/projects/helsinki/endpoints";

const endpoint = (name: string, over: Record<string, unknown> = {}) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Endpoint",
  metadata: { name, namespace: "helsinki" },
  spec: {
    contextSpaceRef: "air",
    slug: "k7m2qz4tv6xh3n5jb2ryd3wcfa",
    audience: "public",
    enabledRepresentations: ["ngsi-ld"],
  },
  status: { phase: "Live" },
  ...over,
});

const answering =
  (items: unknown[], extra?: (path: string) => Response | undefined) =>
  (path: string) => {
    if (path.endsWith("/endpoints")) return jsonResponse(list(items));
    if (path.endsWith("/spaces")) {
      return jsonResponse(
        list([
          {
            apiVersion: "joinedcontext.com/v1alpha1",
            kind: "ContextSpace",
            metadata: { name: "air", namespace: "helsinki" },
            spec: {},
            status: { phase: "Live" },
          },
        ]),
      );
    }
    return extra?.(path);
  };

describe("the endpoints page", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("opens_under_one_h1_with_its_lead_and_no_axe_violation", async () => {
    const { container } = await renderRoute({ path: PATH, answer: answering([endpoint("public-air")]) });
    await expectOneH1(en.endpoints.title);
    expect(screen.getByText(en.endpoints.lead)).toBeInTheDocument();
    await screen.findAllByRole("table");
    await expectAxeClean(container);
  });

  // T-2759: an App's endpoint showed `app-bikes` and nothing else.
  it("says_which_app_an_apps_endpoint_serves", async () => {
    await renderRoute({
      path: PATH,
      answer: answering([
        endpoint("app-bikes", {
          metadata: {
            name: "app-bikes",
            namespace: "helsinki",
            title: { en: "City bikes" },
            annotations: { "joinedcontext.com/generated-by": "portal/app-reconciler" },
          },
        }),
        endpoint("public-air"),
      ]),
    });
    expect(await screen.findByRole("link", { name: "City bikes" })).toBeInTheDocument();
    expect(screen.getByText("The endpoint of the app bikes")).toBeInTheDocument();
    expect(screen.getAllByText(/The endpoint of the app/)).toHaveLength(1);
  });

  it("holds_its_layout_while_the_endpoints_are_on_their_way", async () => {
    await renderRoute({ path: PATH, pending: true });
    const tables = await screen.findAllByRole("table");
    expect(tables[0]).toHaveAttribute("aria-busy", "true");
    expect(screen.queryByText(en.endpoints.empty)).toBeNull();
  });

  it("says_why_the_endpoints_could_not_be_read_and_offers_one_more_try", async () => {
    const { calls } = await renderRoute({
      path: PATH,
      answer: (path) =>
        path.endsWith("/endpoints")
          ? problem(503, "The endpoints could not be read right now.")
          : undefined,
    });
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("The endpoints could not be read right now.");
    expect(screen.queryByText(en.endpoints.empty)).toBeNull();

    const before = calls().filter((call) => call.endsWith("/endpoints")).length;
    await userEvent.click(within(alert).getByRole("button", { name: en.app.error.retry }));
    await waitFor(() =>
      expect(calls().filter((call) => call.endsWith("/endpoints")).length).toBeGreaterThan(before),
    );
  });

  // T-2490, EP-87: one connector URL for all of them, beside the per-endpoint ones.
  it("offers_one_connector_url_for_every_endpoint_and_copies_it", async () => {
    const writeText = vi.fn(() => Promise.resolve());
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    await renderRoute({ path: PATH, answer: answering([endpoint("public-air")]) });
    // Behind the header's button since T-6095, so the list starts right under the title.
    await userEvent.click(await screen.findByRole("button", { name: en.endpoints.hub.button }));
    const section = await screen.findByRole("dialog", { name: en.endpoints.hub.title });
    // On the platform host the gateway names as the hub's resource (the harness's branding domain),
    // not the Portal's own: an MCP client refuses metadata naming another URL (RFC 9728, T-3019).
    const hub = "https://api.portal.hel.fi/api/mcp";
    expect(within(section as HTMLElement).getByText(hub)).toBeInTheDocument();
    expect(within(section as HTMLElement).getByText(en.endpoints.hub.lead)).toBeInTheDocument();
    await userEvent.click(within(section as HTMLElement).getByRole("button", { name: en.endpoints.copyUrl }));
    expect(writeText).toHaveBeenCalledWith(hub);
    expect(await within(section as HTMLElement).findByRole("button", { name: en.endpoints.copied })).toBeInTheDocument();
  });

  it("offers_no_connector_url_while_nothing_is_published", async () => {
    await renderRoute({ path: PATH, answer: answering([]) });
    await screen.findByText(en.endpoints.empty);
    expect(screen.queryByRole("heading", { name: en.endpoints.hub.title })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: en.endpoints.hub.button })).not.toBeInTheDocument();
  });

  it("nothing_published_yet_offers_the_way_to_the_first_endpoint", async () => {
    await renderRoute({ path: PATH, answer: answering([]) });
    expect(await screen.findByText(en.endpoints.empty)).toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: en.endpoints.add }).length).toBeGreaterThan(0);
  });

  it("survives_0_1_and_500_rows", async () => {
    const sent = new Map<number, string[]>();
    for (const count of [0, 1, 500]) {
      const items = Array.from({ length: count }, (_, index) => endpoint(`endpoint-${index}`));
      const { calls, container, unmount } = await renderRoute({ path: PATH, answer: answering(items) });
      // The row count is the check. A role query over 500 rows computes the accessible role of
      // every cell on every poll, seconds of CPU that a loaded host turns into a timeout (T-3538).
      // 500 rows mount one page of 50 (T-4545).
      // An empty list is the empty state at the frame's width, no table (T-6115 judge: clipped at 400 px).
      await waitFor(() =>
        count === 0
          ? expect(container.querySelector("[data-manifest-grid] table")).toBeNull()
          : expect(container.querySelector("table")?.rows.length).toBe(Math.min(count, 50) + 1),
      );
      if (count === 0) await screen.findByText(en.endpoints.empty);
      sent.set(count, [...new Set(calls())].sort());
      unmount();
      vi.restoreAllMocks();
    }
    // Counted, not timed (T-3538): a row asks the server nothing, so 500 rows send what one does.
    expect(sent.get(500)).toEqual(sent.get(1));
  });

  it("a_source_address_out_of_a_manifest_is_a_link_only_when_it_could_navigate", async () => {
    const { container } = await renderRoute({
      path: PATH,
      answer: answering([
        endpoint("public-air", {
          status: { phase: "Live", sourceUrl: "https://git.example.sk/hel/e.yaml" },
        }),
        endpoint("nasty", { status: { phase: "Live", sourceUrl: "javascript:alert(1)" } }),
      ]),
    });
    await screen.findAllByRole("table");
    // Every href on the page at once: not one of them may carry a scheme that runs code (PF-50).
    const schemes = [...container.querySelectorAll("a[href]")]
      .map((link) => link.getAttribute("href") ?? "")
      .filter((href) => /^[a-z][a-z0-9+.-]*:/i.test(href))
      .map((href) => href.slice(0, href.indexOf(":") + 1).toLowerCase());
    // `vscode:` and `cursor:` are the connector's two install links (T-4114, EP-113): constants
    // ConnectAssistant builds from the hub's URL, never a value out of a manifest; they open an
    // installed editor and run nothing in this page.
    const allowed = ["http:", "https:", "mailto:", "vscode:", "cursor:"];
    expect([...new Set(schemes)].filter((scheme) => !allowed.includes(scheme))).toEqual([]);
    expect(container.querySelector('a[href^="javascript:"]')).toBeNull();
  });

  it("opens_the_editor_on_the_endpoint_the_url_names_and_only_once_the_list_has_it", async () => {
    await renderRoute({
      path: `${PATH}?edit=public-air`,
      answer: answering([endpoint("public-air")]),
    });
    const dialog = await findFormPage();
    expect(dialog).toHaveTextContent("public-air");
  });

  // T-4545: 10 000 rows mounted at once showed the first after 18 s.
  const many = Array.from({ length: 120 }, (_, i) => endpoint(`ep-${String(i).padStart(4, "0")}`));
  // Anchors by text, not a role query: computing the role of every cell of 120 rows on every
  // poll is what a loaded CI host turned into 26 s and a timeout (T-3538, T-4553).
  const rowLinks = () =>
    Array.from(document.querySelectorAll("a[href]")).filter((link) => /^ep-\d{4}$/.test(link.textContent ?? ""));
  // One change per search, not one render of 120 rows per typed character.
  const search = async (text: string) => {
    const box = screen.getByRole("searchbox", { name: en.endpoints.find });
    await userEvent.clear(box);
    await userEvent.click(box);
    await userEvent.paste(text);
  };

  it("shows_a_page_of_rows_then_more_on_request_and_finds_one_by_name", async () => {
    await renderRoute({ path: PATH, answer: answering(many) });
    await screen.findByRole("link", { name: "ep-0000" });
    expect(rowLinks()).toHaveLength(50);
    expect(screen.getByText("Showing 50 of 120")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Show 50 more" }));
    expect(rowLinks()).toHaveLength(100);
    await userEvent.click(screen.getByRole("button", { name: "Show 20 more" }));
    expect(rowLinks()).toHaveLength(120);
    expect(screen.queryByRole("button", { name: /^Show \d+ more$/ })).toBeNull();

    await search("EP-0119");
    expect(rowLinks().map((link) => link.textContent)).toEqual(["ep-0119"]);
    expect(screen.getByText("Showing 1 of 1 matching, 120 in total")).toBeInTheDocument();
    // A search matching nothing names the text and clears back to the first page (T-6095).
    await search("nothing-like-it");
    expect(screen.queryByText(/^Showing/)).toBeNull();
    expect(screen.getByText(i18n.t("endpoints.searchNoMatch", { text: "nothing-like-it" }))).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: en.spaces.inside.clearSearch }));
    expect(screen.getByRole("searchbox", { name: en.endpoints.find })).toHaveValue("");
    expect(rowLinks()).toHaveLength(50);
  });

  it("nothing_shared_with_the_project_adds_no_second_table", async () => {
    await renderRoute({ path: PATH, answer: answering([endpoint("public-air")]) });
    await screen.findByRole("link", { name: "public-air" });
    // Own and shared Endpoints are one grid (T-6169): no section, no second table.
    expect(screen.getAllByRole("table")).toHaveLength(1);
    expect(screen.getAllByText(en.endpoints.source.own).length).toBeGreaterThan(0);
  });

  it("finds_by_title_ignores_blank_search_and_starts_a_new_search_on_page_one", async () => {
    const titled = many.map((one, i) =>
      i === 77 ? { ...one, metadata: { ...one.metadata, title: { en: "Noise in Zvolen" } } } : one,
    );
    await renderRoute({ path: PATH, answer: answering(titled) });
    await screen.findByRole("link", { name: "ep-0000" });
    await userEvent.click(screen.getByRole("button", { name: "Show 50 more" }));
    await search("   ");
    expect(rowLinks()).toHaveLength(50);
    expect(screen.getByText("Showing 50 of 120")).toBeInTheDocument();
    await search("zvolen");
    expect(screen.getByRole("link", { name: "Noise in Zvolen" })).toBeInTheDocument();
    expect(screen.getByText("Showing 1 of 1 matching, 120 in total")).toBeInTheDocument();
  });

  it("opens_an_endpoint_the_url_names_beyond_the_first_page", async () => {
    await renderRoute({ path: `${PATH}?edit=ep-0110`, answer: answering(many) });
    const dialog = await findFormPage();
    expect(dialog).toHaveTextContent("ep-0110");
  });

  it("a_short_list_has_the_header_search_and_no_paging", async () => {
    await renderRoute({ path: PATH, answer: answering([endpoint("public-air")]) });
    await screen.findByRole("link", { name: "public-air" });
    // The search sits right of the title on every list (T-6095), short or long.
    expect(screen.getByRole("searchbox", { name: en.endpoints.find })).toBeInTheDocument();
    expect(screen.queryByText(/^Showing/)).toBeNull();
  });

  it("a_name_the_url_asks_for_that_nobody_publishes_opens_nothing", async () => {
    // Not an empty form that would propose a new endpoint under a name off the address bar.
    await renderRoute({
      path: `${PATH}?edit=no-such-endpoint`,
      answer: answering([endpoint("public-air")]),
    });
    await screen.findAllByRole("table");
    expect(queryFormPage()).toBeNull();
  });

  it("says_everything_it_says_in_all_four_languages", async () => {
    for (const locale of LOCALES) {
      const { unmount } = await renderRoute({ path: PATH, locale, answer: answering([]) });
      await expectOneH1(i18n.t("endpoints.title"));
      expect(await screen.findByText(i18n.t("endpoints.empty"))).toBeInTheDocument();
      if (locale !== "en") {
        expect(screen.queryByText(en.endpoints.lead)).toBeNull();
      }
      unmount();
      vi.restoreAllMocks();
    }
  });

  it("paints_in_the_installations_own_colours_not_a_literal", async () => {
    const { container } = await renderRoute({
      path: PATH,
      brand: OTHER_BRAND,
      answer: answering([endpoint("public-air")]),
    });
    await screen.findAllByRole("table");
    const literal = [...container.querySelectorAll<HTMLElement>("[style]")].filter((element) =>
      /#[0-9a-f]{3,8}|\brgba?\(/i.test(element.getAttribute("style") ?? ""),
    );
    expect(
      literal.filter((element) => element.getAttribute("aria-hidden") !== "true").map((e) => e.outerHTML.slice(0, 120)),
    ).toEqual([]);
  });
});
