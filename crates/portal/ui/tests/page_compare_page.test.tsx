/**
 * T-1784: "What the copy changes" against the page contract (UI-11, UI-15, UI-16, UI-61).
 *
 * `workspaces_ui.test.tsx` keeps the grouping and the ordering of the file list. What is
 * asserted here is that the page is a page in each of its four states: the heading stands while
 * the comparison is read and after it failed, the summary counts nothing until there is
 * something to count, a failure carries the API's own sentence and a retry, and axe is clean
 * with files, with none and with a conflict.
 */
import { cleanup, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { ComparePage } from "../src/pages/workspaces/ComparePage";
import {
  expectHeadingOutline,
  expectNoAxeViolations,
  inEveryLocale,
  json,
  problem,
  renderPage,
  tabOrder,
} from "./page_contract";

const PROJECT = "helsinki";
const COPY = "air-v2";
const TITLE = `What the copy ${COPY} changes`;

function file(name: string, operation: "Create" | "Update" | "Delete") {
  return {
    path: `projects/${PROJECT}/spaces/${name}.yaml`,
    kind: "ContextSpace",
    operation,
    lane: "yellow",
    fields: [{ path: "spec.audience", from: "public", to: "internal" }],
  };
}

interface World {
  files?: number;
  conflicts?: number;
  fails?: { status: number; detail: string };
  pending?: boolean;
}

function renderCompare(world: World = {}) {
  const { files = 2, conflicts = 0, fails, pending = false } = world;
  return renderPage(<ComparePage project={PROJECT} name={COPY} />, {
    path: `/projects/${PROJECT}/workspaces/${COPY}/compare`,
    answer: (url) => {
      if (!url.pathname.endsWith("/compare")) return undefined;
      if (pending) return new Promise<Response>(() => undefined);
      if (fails) return problem(fails.status, fails.detail);
      return json({
        files: Array.from({ length: files }, (_, index) =>
          file(`space-${index}`, index % 2 === 0 ? "Create" : "Update"),
        ),
        conflicts: Array.from({ length: conflicts }, (_, index) => ({
          path: `projects/${PROJECT}/spaces/clash-${index}.yaml`,
          fields: [],
        })),
      });
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

describe("what the copy changes", () => {
  it("has one H1 and names the page and the project in the tab", async () => {
    const { container } = renderCompare();
    expect(
      await screen.findByRole("heading", { level: 1, name: TITLE }),
    ).toBeInTheDocument();
    expectHeadingOutline(container);
    await waitFor(() => {
      expect(document.title).toBe(
        `${TITLE} · ${PROJECT} · Helsinki Region Context`,
      );
    });
  });

  // The heading stands while the comparison is read, and it counts nothing yet: "0 added,
  // 0 changed, 0 removed" is an answer, and it would be the wrong one.
  it("keeps the heading while the comparison is read and counts nothing yet", async () => {
    renderCompare({ pending: true });
    expect(
      await screen.findByRole("heading", { level: 1, name: TITLE }),
    ).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent(en.app.loading);
    expect(screen.queryByText(/added/)).not.toBeInTheDocument();
  });

  it("counts what changed once it knows", async () => {
    renderCompare({ files: 3 });
    expect(await screen.findByText("2 added, 1 changed, 0 removed")).toBeInTheDocument();
  });

  it("says the copy changes nothing when it changes nothing", async () => {
    const { container } = renderCompare({ files: 0 });
    expect(await screen.findByText(en.workspaces.compare.empty)).toBeInTheDocument();
    // T-2760: what to do next, and the way there.
    expect(screen.getByText(en.workspaces.compare.emptyHint)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: en.workspaces.compare.back })).toHaveAttribute(
      "href",
      `/projects/${PROJECT}/workspaces`,
    );
    await expectNoAxeViolations(container);
  });


  // A comparison that failed is not "the copy changes nothing".
  it("shows the reason the comparison failed, with a retry", async () => {
    renderCompare({ fails: { status: 503, detail: "The copy's store is not answering." } });
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "The copy's store is not answering.",
    );
    expect(screen.getByRole("button", { name: en.app.error.retry })).toBeInTheDocument();
    expect(screen.queryByText(en.workspaces.compare.empty)).not.toBeInTheDocument();
    expect(
      screen.getByRole("heading", { level: 1, name: TITLE }),
    ).toBeInTheDocument();
  });

  // UI-44: refused is the API's sentence, not "The Portal did not answer".
  it("says a refusal in the API's own words", async () => {
    renderCompare({ fails: { status: 403, detail: "You may not read this copy." } });
    expect(await screen.findByRole("alert")).toHaveTextContent("You may not read this copy.");
  });

  it("asks again when the retry is pressed", async () => {
    let failing = true;
    renderPage(<ComparePage project={PROJECT} name={COPY} />, {
      path: `/projects/${PROJECT}/workspaces/${COPY}/compare`,
      answer: (url) => {
        if (!url.pathname.endsWith("/compare")) return undefined;
        return failing ? problem(503, "Not answering.") : json({ files: [], conflicts: [] });
      },
    });
    const user = userEvent.setup();
    await screen.findByRole("alert");
    failing = false;
    await user.click(screen.getByRole("button", { name: en.app.error.retry }));
    expect(await screen.findByText(en.workspaces.compare.empty)).toBeInTheDocument();
  });

  // UI-61: a file the project changed too is said before the list, with the one way on.
  it("points at the bring back page when a file was changed in the project too", async () => {
    const { container } = renderCompare({ files: 1, conflicts: 2 });
    const notice = await screen.findByText(/2 files also changed in the project/);
    expect(notice).toBeInTheDocument();
    expect(
      screen.getByRole("link", { name: en.workspaces.compare.resolveConflicts }),
    ).toBeInTheDocument();
    await expectNoAxeViolations(container);
  });

  it.each([0, 1, 60])("draws %i changed files without changing how the page reads", async (files) => {
    const { container } = renderCompare({ files });
    await screen.findByRole("heading", { level: 1, name: TITLE });
    if (files === 0) {
      expect(await screen.findByText(en.workspaces.compare.empty)).toBeInTheDocument();
    } else {
      await waitFor(() => {
        // The files, not the breadcrumb's steps.
        expect([...container.querySelectorAll("li")].filter((item) => !item.closest("nav")).length).toBe(files);
      });
    }
    expectHeadingOutline(container);
  });

  // UI-16: the fields of each file open from the keyboard, and nothing takes focus on arrival.
  it("opens a file's fields from the keyboard", async () => {
    const { container } = renderCompare({ files: 1 });
    const summary = await screen.findByText(en.workspaces.compare.showDiff);
    // Nothing takes focus on arrival (UI-16).
    expect(document.activeElement).toBe(document.body);
    // The disclosure is a native `<summary>`, so the keyboard reaches it and Enter opens it;
    // jsdom does not act on Enter over a summary, so what is asserted here is that the keyboard
    // gets there at all and that opening it shows the fields.
    expect(await tabOrder(container)).toContain(summary);
    const user = userEvent.setup();
    await user.click(summary);
    expect(container.querySelector("details[open]")).not.toBeNull();
    expect(screen.getByRole("table", { name: en.approvals.diffType })).toBeInTheDocument();
  });

  it("says the same things in every language", async () => {
    await inEveryLocale(async (locale) => {
      cleanup();
      renderCompare({ files: 0 });
      expect(
        await screen.findByRole("heading", { level: 1, name: i18n.t("workspaces.compare.titleOf", { name: COPY }) }),
        `the title is missing in ${locale}`,
      ).toBeInTheDocument();
      expect(await screen.findByText(i18n.t("workspaces.compare.empty"))).toBeInTheDocument();
    });
  });

  // PF-50: a path the copy carried is read as text, never as markup.
  it("renders a file path that arrived as markup as text", async () => {
    renderPage(<ComparePage project={PROJECT} name={COPY} />, {
      path: `/projects/${PROJECT}/workspaces/${COPY}/compare`,
      answer: (url) =>
        url.pathname.endsWith("/compare")
          ? json({
              files: [{ ...file("x", "Create"), path: "<img src=x onerror=alert(1)>.yaml" }],
              conflicts: [],
            })
          : undefined,
    });
    expect(await screen.findByText("<img src=x onerror=alert(1)>")).toBeInTheDocument();
    expect(document.querySelector("img")).toBeNull();
  });

  it("has no axe violations with files listed", async () => {
    const { container } = renderCompare({ files: 4 });
    expect(await screen.findAllByText(en.workspaces.compare.showDiff)).toHaveLength(4);
    await expectNoAxeViolations(container);
  });

  it("has no axe violations in the failed state", async () => {
    const { container } = renderCompare({ fails: { status: 503, detail: "Not answering." } });
    await screen.findByRole("alert");
    await expectNoAxeViolations(container);
  });
});
