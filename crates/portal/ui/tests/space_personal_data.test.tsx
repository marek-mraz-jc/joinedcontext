/**
 * T-4870, T-5356 (DM-124, DM-126, DM-127, DM-142, API/01 §46): the "A person's data" section of a
 * space. An owner exports what the space holds about one person to a file, or proposes erasing it
 * after a preview of what goes, each with a reason; another owner approves the proposal with the
 * identifier typed again, and the proposer cannot; the fields say what the server would refuse before anything is sent,
 * a refusal is shown in words, and a person who does not own the space sees the buttons off.
 */
// covers (T-2137, the module gate in gate_modules.test.ts): src/pages/spaces/SpacePersonalData.tsx
import { cleanup, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { SpacePersonalData } from "../src/pages/spaces/SpacePersonalData";
import { expectNoAxeViolations, json, renderPage } from "./page_contract";

const BASE = "/api/v1/projects/helsinki/spaces/riders";
const T = en.spaces.personal;

type Sent = { method: string; path: string; search: string; body: unknown };

function renderSection(
  answer: (
    url: URL,
    request: Request,
  ) => Response | undefined | Promise<Response | undefined>,
) {
  const sent: Sent[] = [];
  const result = renderPage(
    <SpacePersonalData project="helsinki" space="riders" />,
    {
      path: "/projects/helsinki/spaces/riders",
      answer: async (url, request) => {
        if (url.pathname.startsWith(BASE) && request.method === "POST") {
          const text = await request.clone().text();
          sent.push({
            method: request.method,
            path: url.pathname,
            search: url.search,
            body: text ? JSON.parse(text) : null,
          });
        }
        return answer(url, request);
      },
    },
  );
  return { ...result, sent };
}

async function fill(subject: string, reason: string) {
  const user = userEvent.setup();
  await user.type(await screen.findByLabelText(new RegExp(T.subject)), subject);
  await user.type(screen.getByLabelText(new RegExp(T.reason)), reason);
  return user;
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("a person's data in a space (DM-124, DM-126)", () => {
  it("names a missing subject and reason on the fields and sends nothing", async () => {
    const { sent, container } = renderSection(() => undefined);
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: T.export }));
    expect(
      await screen.findByText(T.subjectError.required),
    ).toBeInTheDocument();
    expect(screen.getByText(T.reasonError.required)).toBeInTheDocument();
    expect(screen.getByLabelText(new RegExp(T.subject))).toHaveAttribute(
      "aria-invalid",
      "true",
    );
    await user.type(screen.getByLabelText(new RegExp(T.subject)), 'r"42');
    await user.click(screen.getByRole("button", { name: T.export }));
    expect(
      await screen.findByText(T.subjectError.characters),
    ).toBeInTheDocument();
    expect(sent).toHaveLength(0);
    // Each field carries its hint with an example.
    expect(screen.getByText(T.hint.subject)).toBeInTheDocument();
    expect(screen.getByText(T.hint.reason)).toBeInTheDocument();
    await expectNoAxeViolations(container);
  });

  it("exports the person's records to a file, the subject in the body only", async () => {
    const created = vi.fn((_: Blob) => "blob:export");
    vi.stubGlobal(
      "URL",
      Object.assign(URL, {
        createObjectURL: created,
        revokeObjectURL: vi.fn(),
      }),
    );
    // a link outside the document saves nothing in some browsers
    const attached: boolean[] = [];
    const click = vi
      .spyOn(HTMLAnchorElement.prototype, "click")
      .mockImplementation(function (this: HTMLAnchorElement) {
        attached.push(this.isConnected);
      });
    const { sent } = renderSection((url) =>
      url.pathname === `${BASE}/subjects/export`
        ? json({
            space: "riders",
            subject: "hash",
            entities: [{ class: "Rider" }, { class: "Rider" }],
          })
        : undefined,
    );
    const user = await fill("r-42", "Art. 15 request 7");
    await user.click(screen.getByRole("button", { name: T.export }));
    expect(
      await screen.findByText(
        "Exported 2 records to a file in your downloads.",
      ),
    ).toBeInTheDocument();
    expect(sent).toEqual([
      {
        method: "POST",
        path: `${BASE}/subjects/export`,
        search: "",
        body: { subject: "r-42", reason: "Art. 15 request 7" },
      },
    ]);
    expect(click).toHaveBeenCalledTimes(1);
    expect(attached).toEqual([true]);
    const file = JSON.parse(await created.mock.calls[0][0].text()) as {
      entities: unknown[];
    };
    expect(file.entities).toHaveLength(2);
  });

  it("previews an erasure and proposes it on confirm, erasing nothing", async () => {
    const PROPOSED = {
      id: "er-20261021-0a1b2c3d4e5f",
      state: "proposed",
      requestedAt: "2026-10-21T08:00:00Z",
      completedAt: null,
      proposedBy: "user:jana.kovacova",
      expiresAt: "2026-10-28T08:00:00Z",
      entities: 3,
      counts: {},
      stores: {
        broker: { state: "pending", count: 0 },
        portal: { state: "pending", count: 0 },
      },
    };
    let proposed = false;
    const { sent, container } = renderSection((url, request) => {
      if (
        url.pathname === `${BASE}/erasures` &&
        url.searchParams.get("dryRun") === "All"
      ) {
        return json({
          dryRun: true,
          entities: 3,
          groups: [
            {
              class: "Rider",
              entities: ["urn:ngsi-ld:Rider:1", "urn:ngsi-ld:Rider:2"],
              attrs: ["riderName"],
            },
            { class: "Trip", entities: ["urn:ngsi-ld:Trip:9"], attrs: null },
          ],
        });
      }
      if (url.pathname === `${BASE}/erasures` && request.method === "POST") {
        proposed = true;
        return json(PROPOSED, 202);
      }
      if (url.pathname === `${BASE}/erasures`)
        return json({ items: proposed ? [PROPOSED] : [] });
      if (url.pathname === `${BASE}/erasures/${PROPOSED.id}`)
        return json(PROPOSED);
      return undefined;
    });
    expect(await screen.findByText(T.proposals.empty)).toBeInTheDocument();
    const user = await fill("r-42", "Art. 17 request 7");
    await user.click(screen.getByRole("button", { name: T.erase }));
    const dialog = await screen.findByRole("alertdialog");
    expect(
      within(dialog).getByText(
        "Rider: the personal attributes (riderName) of 2 entities",
      ),
    ).toBeInTheDocument();
    expect(
      within(dialog).getByText("Trip: 1 whole entity"),
    ).toBeInTheDocument();
    // The preview erased nothing: one dry run so far.
    expect(sent.map((s) => s.search)).toEqual(["?dryRun=All"]);
    await user.click(within(dialog).getByRole("button", { name: T.confirm }));
    expect(
      await screen.findByText(
        /is proposed \(3 entities found\)\. Nothing is erased/,
      ),
    ).toBeInTheDocument();
    expect(sent[1]).toEqual({
      method: "POST",
      path: `${BASE}/erasures`,
      search: "",
      body: { subject: "r-42", reason: "Art. 17 request 7" },
    });
    // The value does not stay on screen, and the proposer cannot approve their own proposal.
    expect(screen.getByLabelText(new RegExp(T.subject))).toHaveValue("");
    expect(
      await screen.findByRole("button", { name: T.proposals.approve }),
    ).toHaveAttribute("aria-disabled", "true");
    await expectNoAxeViolations(container);
  });

  it("lets another owner approve a proposal with the identifier typed again and follows it", async () => {
    const PROPOSED = {
      id: "er-20261021-0a1b2c3d4e5f",
      state: "proposed",
      requestedAt: "2026-10-21T08:00:00Z",
      proposedBy: "user:olli.virtanen",
      expiresAt: "2026-10-28T08:00:00Z",
      entities: 3,
      counts: {},
      stores: {},
    };
    let approved = false;
    const DONE = {
      ...PROPOSED,
      state: "done",
      stores: { broker: { state: "done", count: 3, versions: 12 } },
    };
    const { sent, container } = renderSection((url, request) => {
      if (url.pathname === `${BASE}/erasures/${PROPOSED.id}/approve`) {
        approved = true;
        return json(DONE);
      }
      if (url.pathname === `${BASE}/erasures`)
        return json({ items: approved ? [] : [PROPOSED] });
      if (
        url.pathname === `${BASE}/erasures/${PROPOSED.id}` &&
        request.method === "GET"
      )
        return json(DONE);
      return undefined;
    });
    const user = userEvent.setup();
    await user.click(
      await screen.findByRole("button", { name: T.proposals.approve }),
    );
    const dialog = await screen.findByRole("alertdialog");
    await user.click(
      within(dialog).getByRole("button", { name: T.approveConfirm }),
    );
    expect(
      await within(dialog).findByText(T.subjectError.required),
    ).toBeInTheDocument();
    expect(sent).toHaveLength(0);
    expect(within(dialog).getByText(T.hint.approveSubject)).toBeInTheDocument();
    await user.type(
      within(dialog).getByLabelText(new RegExp(T.subject)),
      "r-42",
    );
    await user.click(
      within(dialog).getByRole("button", { name: T.approveConfirm }),
    );
    expect(
      await screen.findByText(/every store reports its entities erased/),
    ).toBeInTheDocument();
    expect(sent).toEqual([
      {
        method: "POST",
        path: `${BASE}/erasures/${PROPOSED.id}/approve`,
        search: "",
        body: { subject: "r-42" },
      },
    ]);
    expect(await screen.findByText(T.proposals.empty)).toBeInTheDocument();
    await expectNoAxeViolations(container);
  });

  it("shows the server's refusal in words and erases nothing", async () => {
    const { sent } = renderSection((url, request) =>
      url.pathname === `${BASE}/erasures` && request.method === "POST"
        ? json(
            {
              type: "about:blank",
              title: "Forbidden",
              status: 403,
              detail:
                "only an owner of context space 'riders' runs a data subject's export and erasure (DM-124)",
            },
            403,
          )
        : undefined,
    );
    const user = await fill("r-42", "Art. 17 request 7");
    await user.click(screen.getByRole("button", { name: T.erase }));
    expect(
      await screen.findByText(/only an owner of context space 'riders'/),
    ).toBeInTheDocument();
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
    expect(sent).toHaveLength(1);
  });

  it("keeps both buttons off for a person who does not own the space", async () => {
    renderSection((url) =>
      url.pathname.endsWith("/permissions/me")
        ? json({
            project: "helsinki",
            bootstrap: false,
            grants: [{ rule: { kinds: ["ContextSpace"], verbs: ["read"] } }],
          })
        : undefined,
    );
    await waitFor(async () =>
      expect(
        await screen.findByRole("button", { name: T.export }),
      ).toHaveAttribute("aria-disabled", "true"),
    );
    expect(screen.getByRole("button", { name: T.erase })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
  });

  it("says the erasure list is for owners in a quiet line, not a red alert, on a 403", async () => {
    renderSection((url, request) =>
      url.pathname === `${BASE}/erasures` && request.method === "GET"
        ? json(
            {
              type: "about:blank",
              title: "Forbidden",
              status: 403,
              detail: "only an owner of context space 'riders' runs a data subject's export and erasure",
            },
            403,
          )
        : undefined,
    );
    expect(await screen.findByText(T.proposals.ownersOnly)).toBeInTheDocument();
    expect(screen.queryByText(/only an owner of context space 'riders'/)).toBeNull();
    expect(screen.queryByText(T.proposals.empty)).toBeNull();
  });
});
