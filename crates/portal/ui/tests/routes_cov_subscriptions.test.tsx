/**
 * SubscriptionsPage: the grid read by every column (sort, search), the cells for ids, a paused
 * subscription and one that watches nothing, a subscription proposed from a draft (accepted,
 * refused, and refused before the round trip for watching nothing) and edited through its row menu.
 */
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { digestOf } from "../src/api/digest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { SubscriptionsPage, toSubscriptionEnvelope } from "../src/routes/SubscriptionsPage";
import type { SubscriptionForm } from "../src/routes/SubscriptionsPage";
import { json, list, problem, renderPage } from "./page_contract";
import { sortEachColumn } from "./routes_cov_grid";

const PROJECT = "helsinki";

const FORM: SubscriptionForm = {
  name: "air-alert",
  contextSpaceRef: "air",
  entities: [{ type: "AirQualityObserved" }],
  watchedAttributes: ["pm10"],
  notification: { endpoint: { uri: "https://hook.example.org/air", accept: "application/json" } },
  throttling: 60,
  expiresAt: "2027-01-01T00:00:00Z",
  isActive: true,
};

const envelope = (form: SubscriptionForm, status: unknown) => ({
  ...(toSubscriptionEnvelope(PROJECT, form) as object),
  status,
});

const ALERT = envelope(FORM, {
  phase: "Live",
  notifications: { status: "ok", timesSent: 3, timesFailed: 0, lastSuccess: "2026-10-21T03:20:11Z" },
});
const PAUSED = envelope(
  {
    ...FORM,
    name: "ids-only",
    contextSpaceRef: "buses",
    entities: [{ id: "urn:ngsi-ld:Bus:12" }, { idPattern: "urn:ngsi-ld:Bus:.*" }],
    watchedAttributes: [],
    notification: { endpoint: { uri: "https://hook.example.org/bus", secretRef: { name: "bus-hook", key: "token" } } },
    throttling: 5,
    expiresAt: "2028-02-02T00:00:00Z",
    isActive: false,
  },
  { phase: "Live" },
);
const NOTHING = envelope(
  { ...FORM, name: "no-watch", contextSpaceRef: "parking", notification: { endpoint: { uri: "https://hook.example.org/parking" } }, entities: [], watchedAttributes: undefined, throttling: undefined, expiresAt: undefined },
  { phase: "Error" },
);
const SPACES = ["air", "buses", "parking"].map((name) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ContextSpace",
  metadata: { name, namespace: PROJECT },
  spec: {},
}));

const propose = () =>
  json({ grants: [{ rule: { kinds: ["Subscription"], verbs: ["propose"] } }] });

describe("the subscriptions grid", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  function grid(rows: unknown[]) {
    renderPage(<SubscriptionsPage project={PROJECT} />, {
      path: `/projects/${PROJECT}/subscriptions`,
      answer: (url) => {
        if (url.pathname.endsWith("/permissions/me")) return propose();
        if (url.pathname.endsWith("/subscriptions")) return json(list(rows));
        if (url.pathname.endsWith("/spaces")) return json(list(SPACES));
        return undefined;
      },
    });
  }

  it("sorts by each visible column and orders the rows by what the column reads", async () => {
    const user = userEvent.setup();
    grid([PAUSED, NOTHING, ALERT]);
    const table = await screen.findByRole("table", { name: en.subscriptions.title });
    await screen.findByText("no-watch");
    const sorted = await sortEachColumn(user, table);
    expect(sorted[en.subscriptions.field.name]).toEqual(["air-alert", "ids-only", "no-watch"]);
    expect(sorted[en.subscriptions.field.space]).toEqual(["air-alert", "ids-only", "no-watch"]);
    // Error, Live, Paused: the paused row reads Paused, not Live.
    expect(sorted[en.resourceList.phase]).toEqual(["no-watch", "air-alert", "ids-only"]);
    expect(sorted[en.subscriptions.field.receiver]).toEqual(["air-alert", "ids-only", "no-watch"]);
  });

  it("says what a row watches, whether by type, id, pattern or nothing at all", async () => {
    grid([ALERT, PAUSED, NOTHING]);
    const table = await screen.findByRole("table", { name: en.subscriptions.title });
    await screen.findByText("no-watch");
    const rows = within(table).getAllByRole("row");
    const of = (name: string) => rows.find((row) => within(row).queryByText(name)) as HTMLElement;
    expect(of("air-alert").textContent).toContain("AirQualityObserved, pm10");
    expect(of("ids-only").textContent).toContain("urn:ngsi-ld:Bus:12, urn:ngsi-ld:Bus:.*");
    expect(within(of("ids-only")).getByText(en.subscriptions.paused)).toBeInTheDocument();
    expect(of("no-watch").textContent).toContain("—");
  });

  it("reads every column, the hidden ones too, when the search matches none of them", async () => {
    const user = userEvent.setup();
    grid([ALERT, PAUSED, NOTHING]);
    await screen.findByRole("table", { name: en.subscriptions.title });
    await user.type(screen.getByRole("searchbox"), "60");
    // Only the row whose throttling is 60 (a hidden column) is left.
    await waitFor(() => expect(screen.queryByText("ids-only")).toBeNull());
    expect(screen.getByText("air-alert")).toBeInTheDocument();
    await user.clear(screen.getByRole("searchbox"));
    await user.type(screen.getByRole("searchbox"), "2027-01-01");
    await waitFor(() => expect(screen.queryByText("ids-only")).toBeNull());
    expect(screen.getByText("air-alert")).toBeInTheDocument();
    await user.clear(screen.getByRole("searchbox"));
    await user.type(screen.getByRole("searchbox"), "nothing matches this");
    expect(await screen.findByText(en.grid.noMatch)).toBeInTheDocument();
  });

  it("offers Retry when the list cannot be read", async () => {
    const user = userEvent.setup();
    let reads = 0;
    renderPage(<SubscriptionsPage project={PROJECT} />, {
      path: `/projects/${PROJECT}/subscriptions`,
      answer: (url) => {
        if (url.pathname.endsWith("/permissions/me")) return propose();
        if (url.pathname.endsWith("/subscriptions")) {
          reads += 1;
          return reads === 1 ? problem(503, "The space did not answer.") : json(list([ALERT]));
        }
        return undefined;
      },
    });
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("The space did not answer.");
    await user.click(within(alert).getByRole("button", { name: en.app.error.retry }));
    expect(await screen.findByText("air-alert")).toBeInTheDocument();
  });

  it("edits a subscription through the row menu, with the stored manifest in the form", async () => {
    const user = userEvent.setup();
    renderPage(<SubscriptionsPage project={PROJECT} />, {
      path: `/projects/${PROJECT}/subscriptions`,
      answer: (url) => {
        if (url.pathname.endsWith("/permissions/me")) return propose();
        if (url.pathname.endsWith("/subscriptions/air-alert")) return json(ALERT);
        if (url.pathname.endsWith("/subscriptions")) return json(list([ALERT]));
        if (url.pathname.endsWith("/spaces")) return json(list(SPACES));
        return undefined;
      },
    });
    const table = await screen.findByRole("table", { name: en.subscriptions.title });
    await screen.findByText("air-alert");
    const row = within(table).getAllByRole("row")[1];
    await user.click(within(row).getByRole("button", { name: /air-alert/ }));
    await user.click(await screen.findByRole("menuitem", { name: en.resourceEdit.button }));
    const dialog = await screen.findByRole("dialog");
    await waitFor(() => expect(within(dialog).getByDisplayValue("air-alert")).toBeInTheDocument());
    expect(within(dialog).getByDisplayValue("https://hook.example.org/air")).toBeInTheDocument();
  });
});

describe("proposing a subscription", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  /** The page opened on a draft; the first dry run (Check) is green, what follows is `after`. */
  function draftPage(draft: SubscriptionForm, after: (dryRun: boolean) => Response) {
    const sent: { dryRun: boolean }[] = [];
    renderPage(<SubscriptionsPage project={PROJECT} />, {
      path: `/projects/${PROJECT}/subscriptions?draft=air-alert`,
      answer: async (url, request) => {
        if (url.pathname.endsWith("/permissions/me")) return propose();
        if (url.pathname.includes("/drafts/Subscription/air-alert")) {
          return json({ version: 1, manifest: toSubscriptionEnvelope(PROJECT, draft), verdict: null });
        }
        if (request.method === "POST" && url.pathname.endsWith("/subscriptions")) {
          const dryRun = url.searchParams.get("dryRun") === "All";
          const body = (await request.json()) as Record<string, unknown>;
          sent.push({ dryRun });
          if (sent.length === 1) {
            const judged = Object.fromEntries(Object.entries(body).filter(([key]) => key !== "draft"));
            return json({
              valid: true,
              verdict: { ok: true, findings: [], checkedAt: new Date().toISOString(), inputDigest: digestOf(judged) },
            });
          }
          return after(dryRun);
        }
        if (url.pathname.endsWith("/spaces")) return json(list(SPACES));
        if (url.pathname.endsWith("/subscriptions")) return json(list([PAUSED]));
        return undefined;
      },
    });
    return sent;
  }

  async function checked() {
    const user = userEvent.setup();
    const dialog = await screen.findByRole("dialog");
    await waitFor(() => expect(within(dialog).getByDisplayValue("air-alert")).toBeInTheDocument());
    const submit = within(dialog).getByRole("button", { name: en.subscriptions.propose });
    if (submit.getAttribute("aria-disabled") === "true") {
      await user.click(within(dialog).getByRole("button", { name: en.form.check }));
      await waitFor(() => expect(submit).not.toHaveAttribute("aria-disabled", "true"));
    }
    await user.click(submit);
    return dialog;
  }

  it("proposes it from a draft, closes the form and names the Change it opened", async () => {
    const sent = draftPage(FORM, (dryRun) =>
      dryRun
        ? json({ valid: true })
        : json(
            {
              apiVersion: "joinedcontext.com/v1alpha1",
              kind: "ChangeProposal",
              metadata: { name: "chg-3b9d2e41", namespace: PROJECT },
              summary: { key: "change.summary.create", params: { kind: "Subscription", name: "air-alert", fields: 0 } },
              author: { name: "Jana Kováčová", email: "jana@hel.fi" },
              createdAt: "2026-10-01T09:00:00Z",
              status: { lane: "green", phase: "PendingApproval", plan: { create: 1 } },
            },
            202,
          ),
    );
    await checked();
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(sent.at(-1)?.dryRun).toBe(false);
    expect(await screen.findByRole("link", { name: en.changes.review })).toHaveAttribute(
      "href",
      `/projects/${PROJECT}/approvals/chg-3b9d2e41`,
    );
  });

  it("keeps the dialog open and says why when the server refuses it", async () => {
    draftPage(FORM, () => problem(422, "The receiver address is not reachable."));
    const dialog = await checked();
    expect(await within(dialog).findByText(/The receiver address is not reachable/)).toBeInTheDocument();
    expect(screen.getByRole("dialog")).toBeInTheDocument();
  });

  it("refuses a subscription that watches nothing before asking the server again", async () => {
    const sent = draftPage({ ...FORM, entities: [], watchedAttributes: [] }, () => json({ valid: true }));
    const dialog = await checked();
    expect(await within(dialog).findByText(en.subscriptions.watchesNothing)).toBeInTheDocument();
    // Only the Check went out; nothing was proposed.
    expect(sent).toHaveLength(1);
  });
});

describe("the manifest a subscription form writes", () => {
  it("leaves out a member that is null, blank or empty and keeps a number", () => {
    const written = toSubscriptionEnvelope(PROJECT, {
      ...FORM,
      ...({ q: null, geoQ: "   ", description: undefined } as object),
      throttling: 0,
      watchedAttributes: [],
    }) as { spec: Record<string, unknown> };
    expect(written.spec).not.toHaveProperty("q");
    expect(written.spec).not.toHaveProperty("geoQ");
    expect(written.spec).not.toHaveProperty("watchedAttributes");
    expect(written.spec.throttling).toBe(0);
  });
});
