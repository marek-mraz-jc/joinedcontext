/**
 * T-1783: "Bring the copy back" against the page contract (UI-11, UI-15, UI-16, UI-44, UI-61).
 *
 * The page where a copy's changes come home: what it changed, what the project changed under it,
 * and the choice per field where the two disagree. What is asserted here is that it is a page in
 * each of its four states, that the choice is a real `radiogroup` the arrow keys walk rather than
 * a set of hand-made inputs, that neither action is offered to someone who is not the owner, and
 * that axe is clean with a conflict on screen.
 */
import { cleanup, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { BringBackPage } from "../src/pages/workspaces/BringBackPage";
import { expectDenied } from "./checks";
import {
  expectHeadingOutline,
  expectNoAxeViolations,
  inEveryLocale,
  json,
  problem,
  renderPage,
  tabOrder,
} from "./page_contract";

let me: { email: string; username: string } = { email: "jana@hel.fi", username: "jana" };
vi.mock("../src/auth/AuthProvider", () => ({ useAuthStatus: () => "authenticated",
  useAuth: () => ({ identity: me, status: "authenticated" }),
}));

const PROJECT = "helsinki";
const COPY = "air-v2";
const WORKSPACE = {
  name: COPY,
  title: "Air cleanup",
  project: PROJECT,
  owner: "jana@hel.fi",
  createdAt: "2026-09-12T08:00:00Z",
};

const FILE = {
  path: `projects/${PROJECT}/spaces/ovzdusie.yaml`,
  kind: "ContextSpace",
  operation: "Update",
  lane: "yellow",
  fields: [{ path: "spec.audience", from: "public", to: "internal" }],
};

const CONFLICT = {
  path: `projects/${PROJECT}/spaces/ovzdusie.yaml`,
  fields: [
    { path: "spec.audience", ours: "internal", theirs: "public" },
    { path: "spec.retention", ours: "P30D", theirs: "P90D" },
  ],
};

interface World {
  conflicts?: typeof CONFLICT[];
  files?: typeof FILE[];
  /** Which of the two requests fails, and with what. */
  fails?: { on: "workspace" | "compare"; status: number; detail: string };
  pending?: boolean;
}

function renderBringBack(world: World = {}) {
  const { conflicts = [], files = [FILE], fails, pending = false } = world;
  return renderPage(<BringBackPage project={PROJECT} name={COPY} />, {
    path: `/projects/${PROJECT}/workspaces/${COPY}/bring-back`,
    answer: (url) => {
      const isCompare = url.pathname.endsWith("/compare");
      const isWorkspace = url.pathname.endsWith(`/workspaces/${COPY}`);
      if (!isCompare && !isWorkspace) return undefined;
      if (pending) return new Promise<Response>(() => undefined);
      if (fails && ((fails.on === "compare") === isCompare)) {
        return problem(fails.status, fails.detail);
      }
      return isCompare ? json({ files, conflicts }) : json(WORKSPACE);
    },
  });
}

/**
 * The one choice per conflicting field: a `fieldset` that says it is a `radiogroup`, so a screen
 * reader reads "radio group" and counts the options rather than announcing a plain group
 * (T-1254). The diff table's frame is a `group` and is not one of these.
 */
async function conflictGroups(): Promise<HTMLElement[]> {
  return screen.findAllByRole("radiogroup");
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
  me = { email: "jana@hel.fi", username: "jana" };
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  document.title = "";
});

describe("bringing a copy back", () => {
  it("has one H1 and names the page and the project in the tab", async () => {
    const { container } = renderBringBack();
    expect(
      await screen.findByRole("heading", { level: 1, name: en.workspaces.bringBack.title }),
    ).toBeInTheDocument();
    expectHeadingOutline(container);
    await waitFor(() => {
      expect(document.title).toBe(
        `${en.workspaces.bringBack.title} · ${PROJECT} · Helsinki Region Context`,
      );
    });
  });

  it("keeps the heading while the copy and its comparison are read", async () => {
    renderBringBack({ pending: true });
    expect(
      await screen.findByRole("heading", { level: 1, name: en.workspaces.bringBack.title }),
    ).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent(en.app.loading);
  });

  // A refusal and an outage used to be the same red line of the same words.
  it.each([
    ["workspace", 403, "You may not read this copy."],
    ["compare", 503, "The comparison store is not answering."],
  ] as const)("says why %s could not be read, with a retry only for an outage", async (on, status, detail) => {
    renderBringBack({ fails: { on, status, detail } });
    expect(await screen.findByRole("alert")).toHaveTextContent(detail);
    // A refusal answers the same the second time (T-2834); the way back is there either way.
    expect(screen.queryAllByRole("button", { name: en.app.error.retry })).toHaveLength(status >= 500 ? 1 : 0);
    expect(screen.getByRole("link", { name: en.workspaces.compare.back })).toBeInTheDocument();
    expect(
      screen.getByRole("heading", { level: 1, name: en.workspaces.bringBack.title }),
    ).toBeInTheDocument();
  });

  it("says the copy changes nothing when it changes nothing", async () => {
    const { container } = renderBringBack({ files: [] });
    expect(await screen.findByText(en.workspaces.compare.empty)).toBeInTheDocument();
    await expectNoAxeViolations(container);
  });

  // UI-01, UI-16: the choice per field is a real radiogroup — a legend a screen reader reads
  // before the options, and one tab stop the arrow keys walk. It used to be two hand-made
  // `<input type="radio">` inside labels, so the group had no name and the tab key stopped at
  // every option.
  it("offers the choice per field as a named radio group", async () => {
    renderBringBack({ conflicts: [CONFLICT] });
    const groups = await conflictGroups();
    expect(groups).toHaveLength(2);
    expect(groups[0]).toHaveAccessibleName("spec.audience");
    expect(groups[1]).toHaveAccessibleName("spec.retention");
    const first = within(groups[0]);
    expect(first.getByRole("radio", { name: /Keep the copy's/ })).toBeInTheDocument();
    expect(first.getByRole("radio", { name: /Take the project's/ })).toBeInTheDocument();
  });

  it("walks the options of one field with the arrow keys, and each field is one group", async () => {
    renderBringBack({ conflicts: [CONFLICT] });
    const groups = await conflictGroups();
    const user = userEvent.setup();
    const ours = within(groups[0]).getByRole("radio", { name: /Keep the copy's/ });
    ours.focus();
    await user.keyboard("{ }");
    expect(ours).toBeChecked();
    await user.keyboard("{ArrowDown}");
    expect(within(groups[0]).getByRole("radio", { name: /Take the project's/ })).toBeChecked();
    // What makes the browser walk these two with the arrows and step over them as one tab stop
    // is the shared `name`, and that the next field's options carry a different one. jsdom does
    // not implement the roving tab stop itself, so this is the property that causes it.
    const names = (group: HTMLElement) =>
      new Set([...group.querySelectorAll<HTMLInputElement>("input[type=radio]")].map((r) => r.name));
    expect(names(groups[0]).size).toBe(1);
    expect(names(groups[1]).size).toBe(1);
    expect([...names(groups[0])][0]).not.toBe([...names(groups[1])][0]);
  });

  // Nothing is brought back until every disagreement has an answer.
  it("keeps the update refused until every field is answered", async () => {
    renderBringBack({ conflicts: [CONFLICT] });
    const update = await screen.findByRole("button", { name: en.workspaces.bringBack.update });
    // Refused, not removed from the keyboard: the reason has to be readable (T-1743, UI-44).
    expectDenied(update, en.workspaces.bringBack.updateBlocked);
    const user = userEvent.setup();
    const groups = screen.getAllByRole("radiogroup");
    for (const group of groups) {
      await user.click(within(group).getByRole("radio", { name: /Keep the copy's/ }));
    }
    expect(screen.getByRole("button", { name: en.workspaces.bringBack.update })).toBeEnabled();
  });

  // UI-16: nothing takes focus on arrival, and the keyboard reaches both actions.
  it("is reachable by keyboard and takes no focus on arrival", async () => {
    const { container } = renderBringBack({ conflicts: [CONFLICT] });
    await conflictGroups();
    expect(document.activeElement).toBe(document.body);
    const reached = await tabOrder(container, 60);
    expect(reached).toContain(screen.getByRole("button", { name: en.workspaces.bringBack.update }));
    expect(reached).toContain(screen.getByRole("button", { name: en.workspaces.bringBack.propose }));
  });

  // A conflict is never proposed as one change: it would bring back a half-resolved file.
  it("refuses to propose while a file is in conflict", async () => {
    renderBringBack({ conflicts: [CONFLICT] });
    expectDenied(
      await screen.findByRole("button", { name: en.workspaces.bringBack.propose }),
      en.workspaces.bringBack.proposeBlocked,
    );
  });

  // UI-44: someone who is not the owner is told why, and is offered neither action.
  it("offers someone else's copy nothing to press, with the reason", async () => {
    me = { email: "erik@hel.fi", username: "erik" };
    renderBringBack({ conflicts: [CONFLICT] });
    expect(await screen.findByText(en.workspaces.bringBack.notOwner)).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: en.workspaces.bringBack.update }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: en.workspaces.bringBack.propose }),
    ).not.toBeInTheDocument();
  });

  it("says the same things in every language", async () => {
    await inEveryLocale(async (locale) => {
      cleanup();
      renderBringBack({ conflicts: [CONFLICT] });
      expect(
        await screen.findByRole("heading", {
          level: 1,
          name: i18n.t("workspaces.bringBack.title"),
        }),
        `the title is missing in ${locale}`,
      ).toBeInTheDocument();
      expect(
        await screen.findByRole("button", { name: i18n.t("workspaces.bringBack.update") }),
      ).toBeInTheDocument();
    });
  });

  // PF-50: a field path and a value the copy carried are read as text.
  it("renders a conflicting value that arrived as markup as text", async () => {
    renderBringBack({
      conflicts: [
        {
          path: CONFLICT.path,
          fields: [{ path: "spec.audience", ours: "<img src=x onerror=alert(1)>", theirs: "public" }],
        },
      ],
    });
    expect(await screen.findByText("<img src=x onerror=alert(1)>")).toBeInTheDocument();
    expect(document.querySelector("img")).toBeNull();
  });

  it("has no axe violations with a conflict on screen", async () => {
    const { container } = renderBringBack({ conflicts: [CONFLICT] });
    await conflictGroups();
    await expectNoAxeViolations(container);
  });

  it("has no axe violations in the failed state", async () => {
    const { container } = renderBringBack({
      fails: { on: "compare", status: 503, detail: "Not answering." },
    });
    await screen.findByRole("alert");
    await expectNoAxeViolations(container);
  });
});
