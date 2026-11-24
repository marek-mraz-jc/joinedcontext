/**
 * RegistrationsPage and RegistrationPage, the parts the other tests leave unread: the grid sorted by
 * every column, a registration proposed (and refused), a registration edited through the row menu,
 * and the one registration's page with its Retry and its Edit.
 */
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { digestOf } from "../src/api/digest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { RegistrationsPage, toRegistrationEnvelope } from "../src/routes/RegistrationsPage";
import type { RegistrationForm } from "../src/routes/RegistrationsPage";
import { RegistrationPage } from "../src/routes/RegistrationPage";
import { json, list, problem, renderPage } from "./page_contract";
import { sortEachColumn } from "./routes_cov_grid";

const PROJECT = "helsinki";

const FORM: RegistrationForm = {
  name: "hub-meteo",
  contextSpaceRef: "weather",
  endpointRef: "meteo",
  information: [{ entities: [{ type: "WeatherObserved" }], propertyNames: ["temperature"] }],
  operations: ["retrieveOps"],
  federation: { identity: "caller" },
};

const ONE = {
  ...(toRegistrationEnvelope(PROJECT, FORM) as object),
  status: { phase: "Live" },
};
const TWO = {
  ...(toRegistrationEnvelope(PROJECT, {
    ...FORM,
    name: "air-hub",
    contextSpaceRef: "air",
    endpointRef: undefined,
    endpoint: "https://ngsi.example.org/air",
    information: [{ entities: [{ type: "AirQualityObserved" }] }],
  }) as object),
  status: { phase: "Error" },
};

const propose = (verbs = ["propose"]) =>
  json({ grants: [{ rule: { kinds: ["ContextSourceRegistration"], verbs } }] });

describe("the registrations list", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("sorts by each column and reads the text of every one", async () => {
    const user = userEvent.setup();
    renderPage(<RegistrationsPage project={PROJECT} />, {
      path: `/projects/${PROJECT}/csrs`,
      answer: (url) => {
        if (url.pathname.endsWith("/permissions/me")) return propose();
        if (url.pathname.endsWith("/csrs")) return json(list([ONE, TWO]));
        return undefined;
      },
    });
    const table = await screen.findByRole("table", { name: en.registrations.title });
    const sorted = await sortEachColumn(user, table);
    expect(sorted[en.registrations.field.name]).toEqual(["air-hub", "hub-meteo"]);
    expect(sorted[en.registrations.field.space]).toEqual(["air-hub", "hub-meteo"]);
    expect(sorted[en.registrations.column.covers]).toEqual(["air-hub", "hub-meteo"]);
    expect(sorted[en.resourceList.phase]).toEqual(["air-hub", "hub-meteo"]);
    // The endpoint's name ("meteo") sorts before the host of an address ("ngsi.example.org/air").
    expect(sorted[en.registrations.column.source]).toEqual(["hub-meteo", "air-hub"]);
  });

  /** The page opened on a draft; the first dry run (Check) is green, what follows is `after`. */
  function draftPage(after: (dryRun: boolean) => Response) {
    const sent: { dryRun: boolean }[] = [];
    renderPage(<RegistrationsPage project={PROJECT} />, {
      path: `/projects/${PROJECT}/csrs?draft=hub-meteo`,
      answer: async (url, request) => {
        if (url.pathname.endsWith("/permissions/me")) return propose();
        if (url.pathname.includes("/drafts/ContextSourceRegistration/hub-meteo")) {
          return json({ version: 1, manifest: toRegistrationEnvelope(PROJECT, FORM), verdict: null });
        }
        if (request.method === "POST" && url.pathname.endsWith("/csrs")) {
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
        if (url.pathname.endsWith("/csrs")) return json(list([]));
        return undefined;
      },
    });
    return sent;
  }

  async function checked() {
    const user = userEvent.setup();
    const dialog = await screen.findByRole("dialog");
    await waitFor(() => expect(within(dialog).getByDisplayValue("hub-meteo")).toBeInTheDocument());
    const propose = within(dialog).getByRole("button", { name: en.registrations.propose });
    if (propose.getAttribute("aria-disabled") === "true") {
      await user.click(within(dialog).getByRole("button", { name: en.form.check }));
      await waitFor(() => expect(propose).not.toHaveAttribute("aria-disabled", "true"));
    }
    await user.click(propose);
    return dialog;
  }

  it("proposes a registration from a draft, closes the form and names the Change it opened", async () => {
    const sent = draftPage((dryRun) =>
      dryRun
        ? json({ valid: true })
        : json(
            {
              apiVersion: "joinedcontext.com/v1alpha1",
              kind: "ChangeProposal",
              metadata: { name: "chg-5d1e0a77", namespace: PROJECT },
              summary: { key: "change.summary.create", params: { kind: "ContextSourceRegistration", name: "hub-meteo", fields: 0 } },
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
    expect(await screen.findByRole("link", { name: en.changes.review })).toHaveAttribute("href", `/projects/${PROJECT}/approvals/chg-5d1e0a77`);
  });

  it("keeps the dialog open and says why when the check refuses the registration", async () => {
    draftPage(() => problem(422, "The Endpoint 'meteo' does not exist in helsinki."));
    const dialog = await checked();
    expect(await within(dialog).findByText(/The Endpoint 'meteo' does not exist/)).toBeInTheDocument();
    expect(screen.getByRole("dialog")).toBeInTheDocument();
  });

  it("edits a registration through the row menu, with the stored manifest in the form", async () => {
    const user = userEvent.setup();
    renderPage(<RegistrationsPage project={PROJECT} />, {
      path: `/projects/${PROJECT}/csrs`,
      answer: (url) => {
        if (url.pathname.endsWith("/permissions/me")) return propose();
        if (url.pathname.endsWith("/csrs/hub-meteo")) return json(ONE);
        if (url.pathname.endsWith("/csrs")) return json(list([ONE]));
        return undefined;
      },
    });
    await screen.findByRole("table", { name: en.registrations.title });
    const row = (await screen.findAllByText("hub-meteo"))[0].closest("tr") as HTMLElement;
    await user.click(within(row).getByRole("button", { name: /hub-meteo/ }));
    await user.click(await screen.findByRole("menuitem", { name: en.resourceEdit.button }));
    const dialog = await screen.findByRole("dialog");
    await waitFor(() => expect(within(dialog).getByDisplayValue("hub-meteo")).toBeInTheDocument());
    expect(within(dialog).getByDisplayValue("WeatherObserved")).toBeInTheDocument();
  });
});

describe("one registration's page", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("offers Retry when the read fails, and shows the registration once it answers", async () => {
    const user = userEvent.setup();
    let reads = 0;
    renderPage(<RegistrationPage project={PROJECT} name="hub-meteo" />, {
      path: `/projects/${PROJECT}/csrs/hub-meteo`,
      answer: (url) => {
        if (url.pathname.endsWith("/permissions/me")) return propose();
        if (url.pathname.endsWith("/csrs/hub-meteo")) {
          reads += 1;
          return reads === 1 ? problem(503, "The registry did not answer.") : json(ONE);
        }
        return undefined;
      },
    });
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("The registry did not answer.");
    await user.click(within(alert).getByRole("button", { name: en.app.error.retry }));
    expect(await screen.findByText("WeatherObserved")).toBeInTheDocument();
    expect(reads).toBe(2);
  });

  it("opens the edit form from its Edit button", async () => {
    const user = userEvent.setup();
    renderPage(<RegistrationPage project={PROJECT} name="hub-meteo" />, {
      path: `/projects/${PROJECT}/csrs/hub-meteo`,
      answer: (url) => {
        if (url.pathname.endsWith("/permissions/me")) return propose();
        if (url.pathname.endsWith("/csrs/hub-meteo")) return json(ONE);
        return undefined;
      },
    });
    await user.click(await screen.findByRole("button", { name: en.registrations.page.edit }));
    await waitFor(() => expect(window.location.pathname).toBe(`/projects/${PROJECT}/csrs/hub-meteo/edit`));
  });
});
