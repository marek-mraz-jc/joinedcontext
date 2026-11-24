/**
 * T-7588: Organization → People, the cases people_page and people_invite leave out: the pager's
 * Previous, unticking a person, a refused bulk delete, dialogs dismissed, an invitation sent again
 * with a temporary password (copied, refused by the clipboard), a revoke answered by a Change,
 * enabling a person, a delete nothing named, a failed person read and its Retry, the steps and
 * link expiry of a person not signed in yet, a role in one Context Space, and an edit's body.
 */
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { changedFields, People, PersonPage } from "../src/pages/organization/People";
import { renderPage } from "./page_contract";

const P = en.organization.people;
const PASSWORD = "Tq7#vR2m!Kx9$pL4wZ8@nB3e";

const person = (id: string, first: string, last: string, extra: Record<string, unknown> = {}) => ({
  id,
  email: `${first.toLowerCase()}@hel.fi`,
  firstName: first,
  lastName: last,
  enabled: true,
  emailVerified: true,
  requiredActions: [] as string[],
  createdAt: "2026-09-01T08:00:00Z",
  lastSeen: null,
  locale: "sk",
  pendingDeletion: null,
  ...extra,
});

const JANA = person("jana-id", "Jana", "Nováková", { lastSeen: "2026-09-24T10:30:00Z" });
const EVA = person("eva-id", "Eva", "Kováčová", { enabled: false });
const NORA = person("nora-id", "Nora", "Nová", { requiredActions: ["VERIFY_EMAIL"], invitationExpires: "2099-01-01T00:00:00Z" });
const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-0000077", namespace: "org" },
  status: { lane: "red", phase: "PendingApproval", plan: { create: 0, update: 1, delete: 0 } },
};

const reply = (status: number, body?: unknown) =>
  new Response(body === undefined ? null : JSON.stringify(body), {
    status,
    headers: { "Content-Type": status < 300 ? "application/json" : "application/problem+json" },
  });

interface Sent {
  method: string;
  path: string;
  search: string;
}

interface Options {
  items?: unknown[];
  next?: number;
  detail?: Record<string, unknown>;
  /** Answers for the first reads of the person, then the detail. */
  detailFirst?: Response[];
  remove?: () => Response;
  resend?: () => Response;
  deleted?: () => Response;
}

function answer(sent: Sent[], options: Options = {}) {
  const firsts = [...(options.detailFirst ?? [])];
  return async (url: URL, request: Request) => {
    const path = url.pathname;
    if (path.endsWith("/permissions/me")) {
      return reply(200, { project: "org", bootstrap: false, grants: [{ rule: { kinds: ["Person"], verbs: ["read", "create", "update", "disable", "delete"] } }] });
    }
    if (!path.startsWith("/api/v1/organization/people")) return undefined;
    sent.push({ method: request.method, path, search: url.search });
    if (request.method === "GET" && path === "/api/v1/organization/people") {
      return reply(200, { items: options.items ?? [JANA, EVA], ...(options.next === undefined ? {} : { next: options.next }) });
    }
    if (request.method === "GET") {
      const first = firsts.shift();
      if (first) return first;
      return reply(200, { person: JANA, groups: [], platformRoles: [], appRoles: [], ...options.detail });
    }
    if (path.endsWith("/people/remove")) return options.remove ? options.remove() : reply(202, CHANGE);
    if (path.endsWith("/resend-invitation")) return options.resend ? options.resend() : reply(202, { emailSent: true });
    if (request.method === "DELETE") return options.deleted ? options.deleted() : reply(202, CHANGE);
    return reply(204);
  };
}

const offsets = (sent: Sent[]) =>
  sent.filter((s) => s.method === "GET" && s.path === "/api/v1/organization/people").map((s) => new URLSearchParams(s.search).get("first") ?? "0");

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("Organization → People, the list's remaining cases", () => {
  it("pages back with Previous and drops the ticks of the page it leaves", async () => {
    const user = userEvent.setup();
    const sent: Sent[] = [];
    renderPage(<People />, { path: "/organization/people", answer: answer(sent, { next: 50 }) });
    await screen.findByRole("link", { name: "Jana Nováková" });
    const previous = screen.getByRole("button", { name: P.previous });
    expect(previous).toHaveAttribute("aria-disabled", "true");
    await user.click(screen.getByRole("button", { name: P.next }));
    await waitFor(() => expect(offsets(sent)).toContain("50"));
    await screen.findByRole("link", { name: "Jana Nováková" });
    await user.click(screen.getByRole("checkbox", { name: "Select Jana Nováková" }));
    await user.click(screen.getByRole("button", { name: P.previous }));
    await waitFor(() => expect(offsets(sent).at(-1)).toBe("0"));
    expect(await screen.findByRole("checkbox", { name: "Select Jana Nováková" })).not.toBeChecked();
  });

  it("unticks a person, and the bulk bar goes with the last tick", async () => {
    const user = userEvent.setup();
    renderPage(<People />, { path: "/organization/people", answer: answer([]) });
    const box = await screen.findByRole("checkbox", { name: "Select Jana Nováková" });
    await user.click(box);
    expect(screen.getByText("1 selected")).toBeInTheDocument();
    await user.click(box);
    expect(box).not.toBeChecked();
    expect(screen.queryByText("1 selected")).toBeNull();
  });

  it("closes the bulk dialog on Escape without asking, and on a refused delete shows the reason beside the ticks", async () => {
    const user = userEvent.setup();
    const sent: Sent[] = [];
    renderPage(<People />, {
      path: "/organization/people",
      answer: answer(sent, {
        remove: () => reply(403, { title: "Forbidden", status: 403, detail: "eva@hel.fi holds a right you do not" }),
      }),
    });
    await user.click(await screen.findByRole("checkbox", { name: "Select Eva Kováčová" }));
    await user.click(screen.getByRole("button", { name: P.bulk.delete }));
    await screen.findByRole("alertdialog", { name: "Delete 1 people?" });
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(sent.some((s) => s.path.endsWith("/people/remove"))).toBe(false);

    await user.click(screen.getByRole("button", { name: P.bulk.delete }));
    await user.click(within(await screen.findByRole("alertdialog")).getByTestId("confirm-accept"));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(await screen.findByText(/eva@hel\.fi holds a right you do not/)).toBeInTheDocument();
  });

  it("closes the new-person form on Cancel without sending anything", async () => {
    const user = userEvent.setup();
    const sent: Sent[] = [];
    renderPage(<People />, { path: "/organization/people", answer: answer(sent) });
    await user.click(await screen.findByRole("button", { name: P.new }));
    const dialog = await screen.findByRole("dialog", { name: P.newTitle });
    await user.click(within(dialog).getByRole("button", { name: en.app.cancel }));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: P.newTitle })).toBeNull());
    expect(sent.filter((s) => s.method === "POST")).toEqual([]);
  });

  it("sorts by e-mail and by last activity from the column menus", async () => {
    const user = userEvent.setup();
    renderPage(<People />, { path: "/organization/people", answer: answer([]) });
    await screen.findByRole("link", { name: "Jana Nováková" });
    const names = () => [...document.querySelectorAll(".jc-data-grid tbody tr")].map((tr) => tr.querySelector("a")?.textContent ?? "");
    await user.click(screen.getByRole("button", { name: `${en.entityGrid.columnMenu}: ${P.email}` }));
    await user.click(screen.getByRole("menuitemradio", { name: en.entityGrid.sortAsc }));
    expect(names()).toEqual(["Eva Kováčová", "Jana Nováková"]);
    await user.click(screen.getByRole("button", { name: `${en.entityGrid.columnMenu}: ${P.lastSeen}` }));
    await user.click(screen.getByRole("menuitemradio", { name: en.entityGrid.sortDescDate }));
    expect(names()).toEqual(["Jana Nováková", "Eva Kováčová"]);
  });
});

describe("Organization → People, invitations not accepted", () => {
  it("hands the temporary password of an invitation sent again, copies it, and drops it on close", async () => {
    const user = userEvent.setup();
    const writeText = vi.fn().mockResolvedValueOnce(undefined).mockRejectedValueOnce(new Error("denied"));
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
    renderPage(<People />, {
      path: "/organization/people",
      answer: answer([], { items: [JANA, NORA], resend: () => reply(200, { emailSent: false, temporaryPassword: PASSWORD }) }),
    });
    await user.click(await screen.findByRole("button", { name: "Send the invitation to nora@hel.fi again" }));
    await user.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: P.action["resend-invitation"] }));
    const shown = await screen.findByRole("dialog", { name: P.password.title });
    await user.click(within(shown).getByRole("button", { name: P.password.copy }));
    expect(await within(shown).findByText(P.password.copied)).toBeInTheDocument();
    expect(writeText).toHaveBeenCalledWith(PASSWORD);
    // The clipboard refused the second copy: the page no longer says it is copied.
    await user.click(within(shown).getByRole("button", { name: P.password.copy }));
    await waitFor(() => expect(within(shown).queryByText(P.password.copied)).toBeNull());
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog", { name: P.password.title })).toBeNull());
    expect(document.body.innerHTML).not.toContain(PASSWORD);
  });

  it("shows the Change a revoke waits for, and asks nothing when its dialog is dismissed", async () => {
    const user = userEvent.setup();
    const sent: Sent[] = [];
    renderPage(<People />, { path: "/organization/people", answer: answer(sent, { items: [JANA, NORA] }) });
    await user.click(await screen.findByRole("button", { name: "Revoke the invitation of nora@hel.fi" }));
    await screen.findByRole("alertdialog");
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(sent.filter((s) => s.method === "DELETE")).toEqual([]);
    await user.click(screen.getByRole("button", { name: "Revoke the invitation of nora@hel.fi" }));
    await user.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: P.pending.revoke }));
    expect(await screen.findByText(/chg-0000077/)).toBeInTheDocument();
  });
});

describe("Organization → People, one person's page", () => {
  it("enables a disabled person", async () => {
    const user = userEvent.setup();
    const sent: Sent[] = [];
    renderPage(<PersonPage id="eva-id" />, { path: "/organization/people/eva-id", answer: answer(sent, { detail: { person: EVA } }) });
    await user.click(await screen.findByRole("button", { name: P.action.enable }));
    await user.click(within(await screen.findByRole("alertdialog")).getByTestId("confirm-accept"));
    await waitFor(() => expect(sent.some((s) => s.method === "POST" && s.path.endsWith("/eva-id/enable"))).toBe(true));
  });

  it("returns to the people list when a delete removed the person at once", async () => {
    const user = userEvent.setup();
    renderPage(<PersonPage id="jana-id" />, { path: "/organization/people/jana-id", answer: answer([], { deleted: () => reply(204) }) });
    await user.click(await screen.findByRole("button", { name: P.action.delete }));
    await user.click(within(await screen.findByRole("alertdialog")).getByTestId("confirm-accept"));
    await waitFor(() => expect(window.location.pathname).toBe("/organization/people"));
  });

  it("says why the person could not be read and reads again on Retry", async () => {
    const user = userEvent.setup();
    renderPage(<PersonPage id="jana-id" />, {
      path: "/organization/people/jana-id",
      answer: answer([], { detailFirst: [reply(503, { title: "Unavailable", status: 503, detail: "the realm is restarting" })] }),
    });
    expect(await screen.findByText(/the realm is restarting/)).toBeInTheDocument();
    expect(screen.queryByText(P.notFound)).toBeNull();
    await user.click(screen.getAllByRole("button", { name: en.app.error.retry }).at(-1) as HTMLElement);
    expect(await screen.findByRole("heading", { level: 1, name: "Jana Nováková" })).toBeInTheDocument();
  });

  it("says the realm has no such person on a 404", async () => {
    renderPage(<PersonPage id="gone" />, {
      path: "/organization/people/gone",
      answer: answer([], { detailFirst: [reply(404, { title: "Not Found", status: 404, detail: "no person gone" })] }),
    });
    expect(await screen.findByText(P.notFound)).toBeInTheDocument();
  });

  it("names the steps of a person not signed in yet and when the link expires, ran out or is unknown", async () => {
    const pending = (extra: Record<string, unknown>) =>
      renderPage(<PersonPage id="nora-id" />, {
        path: "/organization/people/nora-id",
        answer: answer([], { detail: { person: { ...NORA, requiredActions: ["VERIFY_EMAIL", "ACCEPT_TERMS"], ...extra } } }),
      });
    const first = pending({});
    expect(await screen.findByText("verify the e-mail, ACCEPT_TERMS")).toBeInTheDocument();
    expect(screen.getByText(/The link works until .*2099/)).toBeInTheDocument();
    first.unmount();
    const second = pending({ invitationExpires: "2026-10-01T00:00:00Z" });
    expect(await screen.findByText(/The link expired on Oct 1, 2026/)).toBeInTheDocument();
    second.unmount();
    pending({ invitationExpires: null });
    expect(await screen.findByText(P.pending.expiryUnknown)).toBeInTheDocument();
  });

  it("names a role held in one Context Space by that space", async () => {
    renderPage(<PersonPage id="jana-id" />, {
      path: "/organization/people/jana-id",
      answer: answer([], {
        detail: { platformRoles: [{ role: "viewer", binding: "air-readers", scope: { project: "helsinki", contextSpace: "air" }, via: { user: "jana@hel.fi" } }] },
      }),
    });
    expect(await screen.findByText(/Context space air/)).toBeInTheDocument();
  });

  it("closes the edit form on Cancel without sending anything", async () => {
    const user = userEvent.setup();
    const sent: Sent[] = [];
    renderPage(<PersonPage id="jana-id" />, { path: "/organization/people/jana-id", answer: answer(sent) });
    await waitFor(() => expect(screen.getByRole("button", { name: P.edit })).not.toHaveAttribute("aria-disabled", "true"));
    await user.click(screen.getByRole("button", { name: P.edit }));
    const dialog = await screen.findByRole("dialog", { name: "Edit Jana Nováková" });
    await user.click(within(dialog).getByRole("button", { name: en.app.cancel }));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Edit Jana Nováková" })).toBeNull());
    expect(sent.filter((s) => s.method === "PATCH")).toEqual([]);
  });
});

describe("an edit's body, each field", () => {
  const form = { email: "jana@hel.fi", firstName: "Jana", lastName: "Nováková", locale: "sk" };
  it("sends nothing when nothing changed", () => {
    expect(changedFields(JANA as never, form as never)).toEqual({});
  });
  it("sends the first name, trimmed, and a new language", () => {
    expect(changedFields(JANA as never, { ...form, firstName: " Janka ", locale: "de" } as never)).toEqual({ firstName: "Janka", locale: "de" });
  });
  it("keeps the realm's language when the form leaves it empty", () => {
    expect(changedFields(JANA as never, { ...form, locale: "" } as never)).toEqual({});
  });
});
