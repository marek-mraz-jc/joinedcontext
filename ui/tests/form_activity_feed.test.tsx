/**
 * T-1750: the ActivityFeed against the UI contract (UI-04, UI-15, UI-16, UI-44, UI-48).
 *
 * The feed renders what the server says happened, and one of those fields — `details.object` —
 * was interpolated straight into the `href` of the link beside each row. `objectOf` asked only
 * whether the string held a `/`, so `endpoints/x?next=…`, `endpoints/x#…` or a name with a space
 * in it built a link to a route nobody meant (PF-50). The first case here is that link.
 *
 * `checkForm`, the shared form-contract helper, is T-1730's and lands with it; what this file
 * asserts by hand is the same list — reachable controls in DOM order, axe, four locales.
 */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { ActivityFeed } from "../src/components/ActivityFeed";
import { objectOf } from "../src/api/activity";
import type { ActivityEvent } from "../src/api/activity";
import en from "../src/locales/en.json";
import { expectNoRawKeys, expectNoViolations, expectTabOrder } from "./checks";

// The links are the router's; its address is what a test reads (T-6230).
vi.mock("@tanstack/react-router", () => ({
  Link: ({ to, params, children, ...rest }: { to: string; params: Record<string, string>; children: React.ReactNode }) => (
    <a href={to.replace(/\$(\w+)/g, (_, key: string) => params[key])} {...rest}>
      {children}
    </a>
  ),
}));

const PROJECT = "helsinki";
const LOCALES = ["en", "sk", "cs", "de"] as const;

function event(overrides: Partial<ActivityEvent> = {}): ActivityEvent {
  return {
    time: "2026-09-16T16:21:03Z",
    project: PROJECT,
    space: "air-quality",
    kind: "access.denied",
    source: "gateway",
    summary: "An anonymous caller was refused a write.",
    severity: "warning",
    details: { object: "endpoints/public-air" },
    ...overrides,
  } as ActivityEvent;
}

class SilentEventSource {
  close() {}
  addEventListener() {}
  removeEventListener() {}
}

function renderFeed(items: ActivityEvent[]) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const url = input instanceof Request ? input.url : String(input);
      const body = url.includes("/activity")
        ? { apiVersion: "joinedcontext.com/v1alpha1", kind: "ActivityList", items }
        : {};
      return new Response(JSON.stringify(body), {
        status: 200,
        headers: { "Content-Type": "application/json" },
      });
    }),
  );
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <ActivityFeed project={PROJECT} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

async function rows() {
  return await waitFor(() => {
    const found = within(screen.getByRole("table")).getAllByRole("row").slice(1);
    expect(found.length).toBeGreaterThan(0);
    return found;
  });
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
  vi.stubGlobal("EventSource", SilentEventSource);
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("the object an activity row links to (PF-50, UI-16)", () => {
  // `{plural}/{name}`, both DNS-1123 labels, is the whole of what the route accepts; a value the
  // server wrote that is not that shape is text in the row, not a link.
  const NOT_AN_OBJECT = [
    "no-slash",
    "endpoints/public-air?next=https://elsewhere.example",
    "endpoints/public-air#fragment",
    "endpoints/public air",
    "endpoints/../../admin",
    "endpoints//public-air",
    "endpoints/public-air/extra",
    "/endpoints/public-air",
    "Endpoints/Public-Air",
    "endpoints/-leading-dash",
    `endpoints/${"a".repeat(64)}`,
    "endpoints/public%2Fair",
    "javascript:alert(1)/x",
    "",
  ];

  it.each(NOT_AN_OBJECT)("an_object_that_is_not_two_labels_is_no_link: %s", (object) => {
    expect(objectOf(event({ details: { object } }))).toBeUndefined();
  });

  it("two_labels_are_the_object_page_and_nothing_is_added_to_the_path", async () => {
    renderFeed([event({ details: { object: "context-spaces/air-quality" } })]);
    const shown = await rows();
    expect(
      within(shown[0]).getByRole("link", { name: "context-spaces/air-quality" }),
      // The record's own page, as RecordLink opens it everywhere: a kind without a detail page
      // opens on its edit form, where the router's redirect took the bare address anyway.
    ).toHaveAttribute("href", "/projects/helsinki/context-spaces/air-quality/edit");
  });

  it("a_row_whose_object_is_refused_still_shows_its_summary_without_a_link", async () => {
    renderFeed([
      event({ summary: "Something happened.", details: { object: "endpoints/x?to=evil" } }),
    ]);
    const shown = await rows();
    expect(shown[0]).toHaveTextContent("Something happened.");
    expect(within(shown[0]).queryByRole("link")).toBeNull();
  });

  it("an_object_that_is_not_a_string_is_no_link", () => {
    expect(objectOf(event({ details: { object: 42 } }))).toBeUndefined();
    expect(objectOf(event({ details: { object: null } }))).toBeUndefined();
    expect(objectOf(event({ details: undefined }))).toBeUndefined();
  });
});

describe("the feed meets the UI contract", () => {
  it("has_no_axe_violation_with_rows_on_screen", async () => {
    const { container } = renderFeed([event(), event({ time: "2026-09-16T17:00:00Z" })]);
    await rows();
    await expectNoViolations(container);
  });

  it("has_no_axe_violation_when_it_is_empty", async () => {
    const { container } = renderFeed([]);
    await waitFor(() => expect(screen.queryByRole("table")).toBeNull());
    await expectNoViolations(container);
  });

  it("every_control_is_reached_by_keyboard_in_dom_order", async () => {
    const user = userEvent.setup();
    const { container } = renderFeed([event()]);
    await rows();
    await expectTabOrder(user, container);
  });

  it.each(LOCALES)("says_everything_in_%s_with_no_raw_key", async (locale) => {
    await i18n.changeLanguage(locale);
    const { container } = renderFeed([event()]);
    await rows();
    // The kind badge and the kind/source filter options render the API's own identifiers
    // (`access.denied`, `gateway`), which no bundle translates and which look exactly like keys.
    expectNoRawKeys(container, ["[data-testid=activity-kind]", "option"]);
  });
});

describe("what happened, in words (T-2756)", () => {
  it("names the kind and the source in words, and a failure as one", async () => {
    renderFeed([
      event({ kind: "catalogue.published", source: "ckan", severity: "error", summary: "The catalogue refused the dataset." }),
      event({ kind: "access.denied", severity: "error" }),
    ]);
    const [publication, refusal] = await rows();
    expect(within(publication).getByTestId("activity-kind")).toHaveTextContent(
      i18n.t("activity.failed", { what: en.activity.kinds.catalogue.published }),
    );
    expect(publication).toHaveTextContent(en.activity.sources.ckan);
    expect(publication).not.toHaveTextContent("catalogue.published");
    // A kind that already says it failed is not said twice.
    expect(within(refusal).getByTestId("activity-kind")).toHaveTextContent(en.activity.kinds.access.denied);
    expect(within(refusal).getByTestId("activity-kind")).not.toHaveTextContent("failed");
  });

  // T-2760: a publisher failing every minute wrote sixty identical rows an hour.
  it("shows identical events as one row that says how often and since when", async () => {
    const failing = (minute: number) =>
      event({
        time: `2026-09-16T16:${String(minute).padStart(2, "0")}:00Z`,
        kind: "catalogue.published",
        source: "ckan",
        severity: "error",
        summary: "The catalogue refused the dataset.",
      });
    renderFeed([failing(59), failing(58), event({ time: "2026-09-16T16:30:00Z" }), failing(0)]);
    const listed = await rows();
    expect(listed).toHaveLength(2);
    const repeats = within(listed[0]).getByTestId("activity-repeats");
    expect(repeats).toHaveTextContent(/^3 times since /);
    expect(within(listed[1]).queryByTestId("activity-repeats")).toBeNull();
  });

  it("offers the filters in words, and a kind it does not know as the platform wrote it", async () => {
    renderFeed([event({ kind: "brand.new", severity: "info" })]);
    const [row] = await rows();
    expect(within(row).getByTestId("activity-kind")).toHaveTextContent("brand.new");
    expect(screen.getByRole("option", { name: en.activity.kinds.config.drifted })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: en.activity.sources.reconciler })).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: "config.drifted" })).not.toBeInTheDocument();
  });
});
