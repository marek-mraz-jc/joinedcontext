/**
 * T-1840: how an application goes live, against the UI contract (UI-11, UI-16, UI-44, AP-71,
 * CC-34, CC-39, PF-50, PF-58).
 *
 * No test named this file, and it carries the one button on the run page that changes the world.
 * What it owns: it draws nothing when there is nothing to show, an address a manifest wrote is
 * checked before it is a link, approving is offered only to somebody the approval rules allow
 * and a red-lane change is never approved here, a refusal is the API's own sentence, and axe is
 * clean in each of those.
 */
import { cleanup, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { RunPublication } from "../src/pages/apps/RunPublication";
import {
  expectNoAxeViolations,
  inEveryLocale,
  json,
  problem,
  renderPage,
  tabOrder,
} from "./page_contract";

const PROJECT = "helsinki";
const CHANGE_ID = "chg-00000069";
const SOURCE = "https://git.example.sk/city/org/src/branch/main/apps/air.yaml";

let me: { email: string; username: string; roles: string[] } = {
  email: "erik@hel.fi",
  username: "erik",
  roles: ["portal-editor"],
};
vi.mock("../src/auth/AuthProvider", () => ({ useAuthStatus: () => "authenticated",
  useIdentity: () => me,
  useAuth: () => ({ identity: me, status: "authenticated" }),
}));

/** What `usePermissions` answers: every verb allowed unless a case says otherwise. */
let can: (kind: string, verb: string) => boolean = () => true;
/** An effective-permissions document, in the shape `allows` reads: one grant per rule. */
function granting(...verbs: string[]): Record<string, unknown> {
  return { bootstrap: false, grants: [{ rule: { kinds: ["App"], verbs } }] };
}

let effective: Record<string, unknown> = granting("approve", "delete");
// Only the hook: `approvalStanding` reads `allows` out of the same module, so the rest of it
// has to stay.
vi.mock("../src/api/permissions", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../src/api/permissions")>()),
  usePermissions: () => ({ data: effective, can: (kind: string, verb: string) => can(kind, verb) }),
}));

function change(overrides: Record<string, unknown> = {}) {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Change",
    metadata: { name: CHANGE_ID, namespace: PROJECT },
    summary: { key: "change.summary.create", params: { kind: "App", name: "air" } },
    author: { name: "Jana", email: "jana@hel.fi" },
    createdAt: "2026-09-12T08:05:00Z",
    status: { lane: "yellow", phase: "PendingApproval", plan: { create: 1 } },
    ...overrides,
  };
}

interface World {
  sourceUrl?: string;
  mirrorUrl?: string;
  changeId?: string;
  body?: Record<string, unknown>;
  approveFails?: { status: number; detail: string };
}

function renderPublication(world: World = {}) {
  const { sourceUrl = SOURCE, mirrorUrl, changeId = CHANGE_ID, body = change(), approveFails } = world;
  return renderPage(
    <RunPublication project={PROJECT} sourceUrl={sourceUrl} mirrorUrl={mirrorUrl} changeId={changeId} />,
    {
      path: `/projects/${PROJECT}/apps/air`,
      answer: (url) => {
        if (url.pathname.endsWith("/approve")) {
          return approveFails
            ? problem(approveFails.status, approveFails.detail)
            : json({ ...body, status: { ...body.status as object, phase: "Deploying" } });
        }
        if (url.pathname.includes(`/changes/${CHANGE_ID}`)) return json(body);
        return undefined;
      },
    },
  );
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
  me = { email: "erik@hel.fi", username: "erik", roles: ["portal-editor"] };
  can = () => true;
  effective = granting("approve", "delete");
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("how an application goes live", () => {
  // Nothing to say, nothing drawn: an empty bordered box under every run would be noise.
  it("draws nothing when there is no source and no change", () => {
    const { container } = renderPublication({ sourceUrl: undefined, changeId: undefined });
    expect(container).toBeEmptyDOMElement();
  });

  it("names itself and links the source in the forge", async () => {
    renderPublication({ changeId: undefined });
    const link = await screen.findByRole("link", { name: en.agentRun.publication.source });
    expect(link).toHaveAttribute("href", SOURCE);
    expect(link).toHaveAttribute("rel", expect.stringContaining("noreferrer"));
  });

  // AP-79: the copy on GitHub sits beside the forge's repository of record, named as a copy.
  it("links the application's copy on GitHub beside the forge, and only where there is one", async () => {
    const mirror = "https://github.com/hel-apps/helsinki_air/tree/agent/app-air/r1";
    renderPublication({ changeId: undefined, mirrorUrl: mirror });
    const copy = await screen.findByRole("link", { name: en.agentRun.publication.mirror });
    expect(copy).toHaveAttribute("href", mirror);
    expect(copy).toHaveAttribute("rel", expect.stringContaining("noreferrer"));
    expect(screen.getByRole("link", { name: en.agentRun.publication.source })).toHaveAttribute("href", SOURCE);
    cleanup();

    renderPublication({ changeId: undefined });
    await screen.findByRole("link", { name: en.agentRun.publication.source });
    expect(screen.queryByRole("link", { name: en.agentRun.publication.mirror })).not.toBeInTheDocument();
  });

  // PF-50: `status.sourceUrl` is written by whoever authored the manifest. An address that
  // cannot navigate draws no icon at all — an icon that does nothing is worse than none.
  it("draws no source link for an address that is not one", () => {
    renderPublication({ sourceUrl: "javascript:alert(1)", changeId: undefined });
    expect(screen.queryByRole("link", { name: en.agentRun.publication.source })).not.toBeInTheDocument();
    expect(document.querySelector('a[href^="javascript:"]')).toBeNull();
  });

  it("names the change and the state it is in", async () => {
    renderPublication();
    expect(await screen.findByRole("link", { name: en.changes.review })).toBeInTheDocument();
    expect(screen.getByText(CHANGE_ID)).toBeInTheDocument();
  });

  // CC-34: approving is offered to somebody the rules allow, and it sends the approval.
  it("approves the change in place", async () => {
    renderPublication();
    const user = userEvent.setup();
    const approve = await screen.findByRole("button", { name: en.agentRun.publication.approve });
    await user.click(approve);
    await screen.findByText(/Deploying/i);
  });

  // CC-39: a red-lane change is approved on its own page, where its name is typed back.
  it("never approves a red-lane change here", async () => {
    renderPublication({ body: change({ status: { lane: "red", phase: "PendingApproval" } }) });
    expect(await screen.findByRole("link", { name: en.changes.review })).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: en.agentRun.publication.approve }),
    ).not.toBeInTheDocument();
  });

  // UI-44: refused says which role is missing, and the page still offers the review.
  it("says which role is missing instead of only hiding the button", async () => {
    can = () => false;
    renderPublication();
    expect(await screen.findByText(en.approvals.needsRole)).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: en.agentRun.publication.approve }),
    ).not.toBeInTheDocument();
  });

  // PF-58: one's own change is approved only as an administrator of the kind, and the page says
  // that is what happened.
  it("says when somebody approves their own change as an administrator", async () => {
    me = { email: "jana@hel.fi", username: "jana", roles: ["portal-admin"] };
    renderPublication();
    expect(
      await screen.findByText(
        en.approvals.ownAsAdministrator.replace("{kind}", "App"),
      ),
    ).toBeInTheDocument();
  });

  it("refuses one's own change when one does not administer the kind", async () => {
    me = { email: "jana@hel.fi", username: "jana", roles: ["portal-editor"] };
    // Approve but not delete: an approver of the kind, not an administrator of it (PF-58).
    effective = granting("approve");
    renderPublication();
    expect(await screen.findByText(en.approvals.ownProposal)).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: en.agentRun.publication.approve }),
    ).not.toBeInTheDocument();
  });

  // A refusal of the approval itself is the API's own sentence, never "the Portal did not answer".
  it("shows the API's reason when the approval is refused", async () => {
    renderPublication({ approveFails: { status: 409, detail: "The change moved on while you read it." } });
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: en.agentRun.publication.approve }));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "The change moved on while you read it.",
    );
  });

  it("renders a refusal that arrived as markup as text", async () => {
    renderPublication({ approveFails: { status: 409, detail: "<img src=x onerror=alert(1)>" } });
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: en.agentRun.publication.approve }));
    expect(await screen.findByText("<img src=x onerror=alert(1)>")).toBeInTheDocument();
    expect(document.querySelector("img")).toBeNull();
  });

  // UI-16: both actions are reached by keyboard, and nothing takes focus on arrival.
  it("is reachable by keyboard and takes no focus on arrival", async () => {
    const { container } = renderPublication();
    await screen.findByRole("button", { name: en.agentRun.publication.approve });
    expect(document.activeElement).toBe(document.body);
    const reached = await tabOrder(container);
    expect(reached).toContain(screen.getByRole("button", { name: en.agentRun.publication.approve }));
    expect(reached).toContain(screen.getByRole("link", { name: en.changes.review }));
  });

  it("says the same things in every language", async () => {
    await inEveryLocale(async (locale) => {
      cleanup();
      renderPublication();
      expect(
        await screen.findByRole("button", { name: i18n.t("agentRun.publication.approve") }),
        `the approve action is missing in ${locale}`,
      ).toBeInTheDocument();
    });
  });

  it("has no axe violations with a change pending approval", async () => {
    const { container } = renderPublication();
    await screen.findByRole("button", { name: en.agentRun.publication.approve });
    await expectNoAxeViolations(container);
  });
});
