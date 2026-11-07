/**
 * T-1782: "Sync sources" against the page contract (UI-11, UI-15, UI-16, UI-44, MF-27, MF-28).
 *
 * No test named this page before. What is asserted here: the H1 and the tab, the four states of
 * the list, that a source's own status can fail on its own without taking the page with it, that
 * detaching a source — the one destructive action on the page — asks in the shared dialog with
 * the source named, and that both links it draws come from the source's own status and are
 * checked before they are links.
 */
import { cleanup, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { SyncSourcesPage, intervalWords } from "../src/pages/sync/SyncSourcesPage";
import { expectDenied, expectOpen } from "./checks";
import {
  expectHeadingOutline,
  expectNoAxeViolations,
  inEveryLocale,
  json,
  list,
  problem,
  renderPage,
  tabOrder,
} from "./page_contract";

const PROJECT = "helsinki";

function source(index: number) {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "SyncSource",
    metadata: { name: `upstream-${index}`, namespace: PROJECT },
    spec: {
      source: { git: { url: "https://git.example.sk/city/org", ref: "main", path: "projects" } },
      schedule: { interval: "6h" },
      mode: "mirror",
    },
  };
}

const STATUS = {
  phase: "Live",
  observedRevision: "9c1f0ab2233445566778899aabbccddeeff00112",
  durable: true,
};

interface World {
  rows?: number;
  fails?: { status: number; detail: string };
  pending?: boolean;
  status?: Record<string, unknown> | "fails";
  /** What `permissions/me` answers; left out, the page has no document and the API decides. */
  permissions?: unknown;
}

function renderSync(world: World = {}) {
  const { rows = 1, fails, pending = false, status = STATUS, permissions } = world;
  return renderPage(<SyncSourcesPage project={PROJECT} />, {
    path: `/projects/${PROJECT}/syncsources`,
    answer: (url, request) => {
      if (permissions !== undefined && url.pathname.endsWith("/permissions/me")) {
        return json(permissions);
      }
      if (url.pathname.endsWith("/status")) {
        return status === "fails"
          ? problem(502, "The source's repository did not answer.")
          : json(status);
      }
      if (!url.pathname.endsWith("/syncsources") || request.method !== "GET") return undefined;
      if (pending) return new Promise<Response>(() => undefined);
      if (fails) return problem(fails.status, fails.detail);
      return json(list(Array.from({ length: rows }, (_, index) => source(index))));
    },
  });
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  document.title = "";
});

describe("the sync sources page", () => {
  it("has one H1 and names the page and the project in the tab", async () => {
    const { container } = renderSync();
    expect(
      await screen.findByRole("heading", { level: 1, name: en.syncSources.title }),
    ).toBeInTheDocument();
    expect(screen.getByText(en.syncSources.intro)).toBeInTheDocument();
    expectHeadingOutline(container);
    await waitFor(() => {
      expect(document.title).toBe(`${en.syncSources.title} · ${PROJECT} · Helsinki Region Context`);
    });
  });

  // The one primary action of the page, and the origin it will create.
  it("offers one primary action and the origin it creates", async () => {
    renderSync();
    const add = await screen.findByRole("button", { name: en.syncSources.add });
    expect(add).toBeInTheDocument();
    // The choice is a labelled control, not a bare select under a line of text (UI-04).
    expect(screen.getByLabelText(en.syncSources.origin)).toHaveValue("git");
    const user = userEvent.setup();
    await user.click(add);
    expect(
      await screen.findByRole("dialog", { name: en.syncSources.dialog.title }),
    ).toBeInTheDocument();
  });

  // T-3219: a person who never set one up learns at each field what to type and what it does.
  it("tells a new user at each field what to type, and refuses a wrong address in words", async () => {
    renderSync();
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: en.syncSources.add }));
    const dialog = await screen.findByRole("dialog", { name: en.syncSources.dialog.title });
    const url = within(dialog).getByLabelText(new RegExp(`^${en.syncSources.field.url}`));
    expect(url).toHaveAccessibleDescription(/A private repository also needs the credential below/);
    await user.type(url, "git.example.org/city");
    await user.tab();
    expect(await within(dialog).findByText(en.form.gitUrl)).toBeInTheDocument();
  });

  it("says the list is loading rather than showing it empty", async () => {
    renderSync({ pending: true });
    expect(await screen.findByRole("status")).toHaveTextContent(en.app.loading);
    expect(screen.queryByText(en.syncSources.empty)).not.toBeInTheDocument();
  });

  it("says what a project with no source looks like", async () => {
    const { container } = renderSync({ rows: 0 });
    expect(await screen.findByText(en.syncSources.empty)).toBeInTheDocument();
    expect(screen.getByText(en.syncSources.emptyHint)).toBeInTheDocument();
    await expectNoAxeViolations(container);
  });

  // A list that failed is not "this project syncs from nothing".
  it("shows the reason the list failed, with a retry", async () => {
    renderSync({ fails: { status: 503, detail: "The configuration store is not answering." } });
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "The configuration store is not answering.",
    );
    expect(screen.getByRole("button", { name: en.app.error.retry })).toBeInTheDocument();
    expect(screen.queryByText(en.syncSources.empty)).not.toBeInTheDocument();
  });

  it("says a refusal in the API's own words", async () => {
    renderSync({ fails: { status: 403, detail: "You may not read this project's sync sources." } });
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "You may not read this project's sync sources.",
    );
  });

  // One source's status failing leaves the rest of the page standing, and says which.
  it("keeps the page when one source's status cannot be read", async () => {
    const { container } = renderSync({ rows: 2, status: "fails" });
    const failures = await screen.findAllByRole("alert");
    expect(failures.length).toBeGreaterThan(0);
    expect(failures[0]).toHaveTextContent("The source's repository did not answer.");
    expect(screen.getByRole("heading", { level: 1, name: en.syncSources.title })).toBeInTheDocument();
    expect(container.querySelectorAll("article")).toHaveLength(2);
  });

  // UI-44: detaching is destructive, so it is asked in the shared dialog, with the source named
  // and in the person's own language — never `window.confirm`.
  it("asks before detaching a source, naming it", async () => {
    renderSync({ rows: 1 });
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: en.syncSources.detach }));
    const dialog = await screen.findByRole("alertdialog", { name: en.syncSources.detach });
    expect(
      within(dialog).getByText(en.syncSources.detachConfirm.replace("{name}", "upstream-0")),
    ).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: en.syncSources.detach })).toBeInTheDocument();
  });

  // UI-44: every write control asks for the verb its route checks. A viewer meets each one closed
  // with the reason; a person who may propose but not delete runs the loop but cannot detach.
  it("closes each write control to a role without its verb, and says which verb", async () => {
    renderSync({ rows: 1, permissions: { project: PROJECT, bootstrap: false, grants: [] } });
    const reason = (verb: string) =>
      `Disabled: your role does not permit '${verb}' on 'SyncSource' in this project`;
    await waitFor(() => {
      expectDenied(screen.getByRole("button", { name: en.syncSources.add }), reason("propose"));
    });
    expectDenied(await screen.findByRole("button", { name: en.syncSources.syncNow }), reason("propose"));
    expectDenied(screen.getByRole("button", { name: en.syncSources.pause }), reason("propose"));
    expectDenied(screen.getByRole("button", { name: en.syncSources.detach }), reason("delete"));

    cleanup();
    renderSync({
      rows: 1,
      permissions: {
        project: PROJECT,
        bootstrap: false,
        grants: [{ role: "steward", binding: "stewards", rule: { kinds: ["SyncSource"], verbs: ["propose"] } }],
      },
    });
    await waitFor(() => {
      expectDenied(screen.getByRole("button", { name: en.syncSources.detach }), reason("delete"));
    });
    expectOpen(screen.getByRole("button", { name: en.syncSources.add }));
    expectOpen(screen.getByRole("button", { name: en.syncSources.syncNow }));
    expectOpen(screen.getByRole("button", { name: en.syncSources.pause }));
  });

  // A merge request the source reported is a link only when it is an address; the review link
  // for a detached source is the same value through the same check (PF-50).
  it("links a merge request the source reported, and shows a non-address as words", async () => {
    renderSync({
      rows: 1,
      status: { ...STATUS, mergeRequest: "https://git.example.sk/city/org/pulls/7" },
    });
    expect(await screen.findByRole("link", { name: new RegExp(en.syncSources.review) })).toHaveAttribute(
      "href",
      "https://git.example.sk/city/org/pulls/7",
    );

    cleanup();
    renderSync({ rows: 1, status: { ...STATUS, mergeRequest: "javascript:alert(1)" } });
    expect(await screen.findByText(en.syncSources.review)).toBeInTheDocument();
    expect(document.querySelector('a[href^="javascript:"]')).toBeNull();
  });

  it.each([0, 1, 40])("draws %i sources without changing how the page reads", async (rows) => {
    const { container } = renderSync({ rows });
    await screen.findByRole("heading", { level: 1, name: en.syncSources.title });
    if (rows === 0) {
      expect(await screen.findByText(en.syncSources.empty)).toBeInTheDocument();
    } else {
      await waitFor(() => {
        expect(container.querySelectorAll("article")).toHaveLength(rows);
      });
    }
    expectHeadingOutline(container);
  });

  // UI-16: the keyboard reaches the page's own action first, then each source's.
  it("is reachable by keyboard and takes no focus on arrival", async () => {
    const { container } = renderSync({ rows: 1 });
    await screen.findByRole("button", { name: en.syncSources.syncNow });
    expect(document.activeElement).toBe(document.body);
    const reached = await tabOrder(container, 60);
    expect(reached).toContain(screen.getByLabelText(en.syncSources.origin));
    expect(reached).toContain(screen.getByRole("button", { name: en.syncSources.add }));
    expect(reached).toContain(screen.getByRole("button", { name: en.syncSources.syncNow }));
  });

  it("says the same things in every language", async () => {
    await inEveryLocale(async (locale) => {
      cleanup();
      renderSync({ rows: 0 });
      expect(
        await screen.findByRole("heading", { level: 1, name: i18n.t("syncSources.title") }),
        `the title is missing in ${locale}`,
      ).toBeInTheDocument();
      expect(await screen.findByText(i18n.t("syncSources.emptyHint"))).toBeInTheDocument();
    });
  });

  // PF-50: the last error a run reported is read as text.
  it("renders a source's last error that arrived as markup as text", async () => {
    renderSync({ rows: 1, status: { ...STATUS, lastError: "<img src=x onerror=alert(1)>" } });
    expect(await screen.findByText("<img src=x onerror=alert(1)>")).toBeInTheDocument();
    expect(document.querySelector("img")).toBeNull();
  });

  it("has no axe violations with a source listed", async () => {
    const { container } = renderSync({ rows: 2 });
    await screen.findAllByRole("button", { name: en.syncSources.syncNow });
    await expectNoAxeViolations(container);
  });

  it("has no axe violations in the failed state", async () => {
    const { container } = renderSync({ fails: { status: 503, detail: "Not answering." } });
    await screen.findByRole("alert");
    await expectNoAxeViolations(container);
  });
});

describe("a sync source's interval", () => {
  it("is said in the reader's words, anything unknown as written", () => {
    expect(intervalWords("1d", "en")).toBe("1 day");
    expect(intervalWords("6h", "en")).toBe("6 hours");
    expect(intervalWords("30m", "de")).toBe("30 Minuten");
    expect(intervalWords("P1D", "en")).toBe("P1D");
  });
});
