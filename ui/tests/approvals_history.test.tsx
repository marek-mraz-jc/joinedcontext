/**
 * T-3292: a person reads the change history from the Approvals page: what was merged or rejected,
 * who proposed and who decided it, and older pages on request; the filters ask the API once per
 * submit and never per keystroke.
 */
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { download } from "../src/pages/models/diagramExport";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { ApprovalsHistory } from "../src/routes/ApprovalsHistory";
import { json, renderPage } from "./page_contract";
import {
  expectAxeClean,
  jsonResponse,
  LOCALES,
  problem,
  renderRoute,
} from "./pageHarness";

const PATH = "/projects/helsinki/approvals";

vi.mock("../src/pages/models/diagramExport", async (original) => ({
  ...(await original<typeof import("../src/pages/models/diagramExport")>()),
  download: vi.fn(),
}));

const closed = (name: string, over: Record<string, unknown> = {}) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name, namespace: "helsinki" },
  summary: {
    key: "change.summary.create",
    params: { kind: "ContextSpace", name: "mobility", fields: 0 },
  },
  author: { name: "Jana Kováčová", email: "jana@hel.fi" },
  createdAt: "2026-10-01T09:00:00Z",
  status: {
    lane: "green",
    phase: "Merged",
    plan: { create: 0, update: 0, delete: 0 },
  },
  decision: { by: "eva.approver@hel.fi", at: "2026-10-01T10:00:00Z" },
  ...over,
});

const REJECTED = closed("chg-00000002", {
  summary: {
    key: "change.summary.update",
    params: { kind: "Endpoint", name: "public-air", fields: 0 },
  },
  status: {
    lane: "yellow",
    phase: "Rejected",
    plan: { create: 0, update: 0, delete: 0 },
  },
  decision: {
    by: "eva.approver@hel.fi",
    at: "2026-10-02T10:00:00Z",
    reason: "the URL is the old one",
  },
});

/** The history pages by `page`, recording each query the API was asked. */
function historyOf(pages: Record<string, { items: unknown[]; next?: number }>) {
  const asked: string[] = [];
  const answer = (path: string, request: Request) => {
    if (!path.endsWith("/changes/history")) return undefined;
    const url = new URL(request.url);
    asked.push(url.search);
    const page = pages[url.searchParams.get("page") ?? "1"] ?? { items: [] };
    return jsonResponse({
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "ChangeList",
      ...page,
    });
  };
  return { answer, asked };
}

async function openHistory() {
  await userEvent.click(
    await screen.findByRole("tab", { name: en.approvals.tabs.history }),
  );
  return screen.findByRole("table", { name: en.approvals.history.caption });
}

describe("the change history", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("lists_what_was_decided_by_whom_and_why_and_links_each_change", async () => {
    const { answer } = historyOf({
      "1": { items: [REJECTED, closed("chg-00000001")] },
    });
    const { container } = await renderRoute({ path: PATH, answer });
    const table = await openHistory();

    const rows = await within(table).findAllByRole("row");
    expect(rows).toHaveLength(3);
    expect(
      within(rows[1]).getByRole("link", {
        name: "Jana Kováčová proposed to change Endpoint public-air",
      }),
    ).toHaveAttribute("href", "/projects/helsinki/approvals/chg-00000002");
    expect(within(rows[1]).getByText("Rejected")).toBeInTheDocument();
    expect(
      within(rows[1]).getByText("Reason: the URL is the old one"),
    ).toBeInTheDocument();
    expect(within(rows[2]).getByText("Merged")).toBeInTheDocument();
    expect(
      within(rows[2]).getByText("eva.approver@hel.fi"),
    ).toBeInTheDocument();
    expect(within(rows[2]).getByText("Jana Kováčová")).toBeInTheDocument();
    // The last page offers nothing older.
    expect(
      screen.queryByRole("button", { name: en.approvals.history.older }),
    ).toBeNull();
    await expectAxeClean(container);
  });

  it("a_change_closed_in_the_repository_says_so_instead_of_naming_nobody", async () => {
    const { answer } = historyOf({
      "1": { items: [closed("chg-00000003", { decision: undefined })] },
    });
    await renderRoute({ path: PATH, answer });
    const table = await openHistory();
    expect(
      await within(table).findByText(en.approvals.history.inForge),
    ).toBeInTheDocument();
  });

  it("asks_for_older_changes_only_when_the_person_does", async () => {
    const { answer, asked } = historyOf({
      "1": { items: [closed("chg-00000005")], next: 2 },
      "2": { items: [closed("chg-00000004")] },
    });
    await renderRoute({ path: PATH, answer });
    const table = await openHistory();
    await within(table).findByText("chg-00000005");
    expect(asked).toEqual(["?page=1"]);

    await userEvent.click(
      screen.getByRole("button", { name: en.approvals.history.older }),
    );
    expect(await within(table).findByText("chg-00000004")).toBeInTheDocument();
    expect(asked).toEqual(["?page=1", "?page=2"]);
    await waitFor(() =>
      expect(
        screen.queryByRole("button", { name: en.approvals.history.older }),
      ).toBeNull(),
    );
  });

  it("a_page_with_nothing_readable_still_offers_the_older_ones", async () => {
    const { answer } = historyOf({ "1": { items: [], next: 2 } });
    await renderRoute({ path: PATH, answer });
    await openHistory();
    expect(
      await screen.findByText(en.approvals.history.noneOnPage),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: en.approvals.history.older }),
    ).toBeInTheDocument();
  });

  it("filters_on_submit_and_never_per_keystroke_and_clears_back", async () => {
    const { answer, asked } = historyOf({ "1": { items: [] } });
    // The kinds offered are the ones the person holds a right on (T-4947).
    await renderRoute({
      path: PATH,
      answer,
      permissions: { project: "helsinki", bootstrap: false, grants: [{ rule: { kinds: ["Endpoint", "Pipeline"], verbs: ["read"] } }] },
    });
    await openHistory();
    await screen.findByText(en.approvals.history.empty);

    await userEvent.selectOptions(
      screen.getByLabelText(en.approvals.history.kind),
      "Endpoint",
    );
    await userEvent.type(
      screen.getByLabelText(en.approvals.history.name),
      "air{Enter}",
    );
    await screen.findByText(en.approvals.noneMatch);
    expect(asked).toEqual(["?page=1", "?page=1&kind=Endpoint&name=air"]);

    await userEvent.click(
      screen.getByRole("button", { name: en.approvals.history.clear }),
    );
    expect(
      await screen.findByText(en.approvals.history.empty),
    ).toBeInTheDocument();
    expect(screen.getByLabelText(en.approvals.history.kind)).toHaveValue("");
  });

  // T-5114: the audit trail leaves the Portal as CSV, under the filter the list was asked for.
  it("exports_the_filtered_history_as_csv_and_says_when_the_file_is_cut", async () => {
    const { answer, asked } = historyOf({ "1": { items: [REJECTED] } });
    const exported: string[] = [];
    const csv = "closedAt,change\r\n2026-10-02T10:00:00Z,chg-00000002\r\n";
    await renderRoute({
      path: PATH,
      answer: (path, request) => {
        if (!path.endsWith("/changes/history/export")) return answer(path, request);
        exported.push(new URL(request.url).search);
        const headers: Record<string, string> = { "content-type": "text/csv; charset=utf-8" };
        if (exported.length > 1) headers["jc-next-page"] = "11";
        return new Response(csv, { status: 200, headers });
      },
      permissions: { project: "helsinki", bootstrap: false, grants: [{ rule: { kinds: ["Endpoint"], verbs: ["read"] } }] },
    });
    await openHistory();
    const button = screen.getByRole("button", { name: en.approvals.history.export });
    expect(button).toHaveAccessibleDescription(i18n.t("approvals.history.exportHint", { project: "helsinki" }));
    expect(screen.getByText(en.approvals.history.dataWrites)).toBeInTheDocument();

    await userEvent.selectOptions(screen.getByLabelText(en.approvals.history.kind), "Endpoint");
    await userEvent.click(screen.getByRole("button", { name: en.approvals.history.apply }));
    await waitFor(() => expect(asked).toContain("?page=1&kind=Endpoint"));
    await userEvent.click(button);
    await waitFor(() =>
      expect(download).toHaveBeenCalledWith(csv, "helsinki-changes.csv", "text/csv;charset=utf-8"),
    );
    expect(exported).toEqual(["?kind=Endpoint"]);
    expect(screen.queryByText(en.approvals.history.exportCut)).toBeNull();

    await userEvent.click(button);
    expect(await screen.findByText(en.approvals.history.exportCut)).toBeInTheDocument();
  });

  it("says_why_the_history_could_not_be_read", async () => {
    await renderRoute({
      path: PATH,
      answer: (path) =>
        path.endsWith("/changes/history")
          ? problem(503, "The forge does not answer.")
          : undefined,
    });
    await userEvent.click(
      await screen.findByRole("tab", { name: en.approvals.tabs.history }),
    );
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "The forge does not answer.",
    );
  });

  it("renders_a_reason_out_of_the_api_as_text_and_never_as_markup", async () => {
    const { answer } = historyOf({
      "1": {
        items: [
          closed("chg-00000006", {
            decision: { by: "x", reason: "<img src=x onerror=alert(1)>" },
          }),
        ],
      },
    });
    await renderRoute({ path: PATH, answer });
    const table = await openHistory();
    expect(
      await within(table).findByText("Reason: <img src=x onerror=alert(1)>"),
    ).toBeInTheDocument();
    expect(table.querySelector("img")).toBeNull();
  });

  it("a_merged_removal_offers_restore_and_the_new_change_waits_for_an_approver", async () => {
    const removal = closed("chg-00000007", {
      summary: { key: "change.summary.delete", params: { kind: "Pipeline", name: "air-ingest", fields: 0 } },
    });
    const restored: string[] = [];
    const { answer } = historyOf({ "1": { items: [removal, REJECTED, closed("chg-00000001")] } });
    await renderRoute({
      path: PATH,
      answer: (path, request) => {
        if (path.endsWith("/restore") && request.method === "POST") {
          restored.push(path);
          return jsonResponse(
            {
              apiVersion: "joinedcontext.com/v1alpha1",
              kind: "Change",
              metadata: { name: "chg-00000009", namespace: "helsinki" },
              status: { lane: "yellow", phase: "PendingApproval", plan: { create: 1, update: 0, delete: 0 } },
            },
            202,
          );
        }
        return answer(path, request);
      },
    });
    const table = await openHistory();
    // Only the merged removal: a rejected change removed nothing, a create has nothing to bring back.
    const buttons = await within(table).findAllByRole("button", { name: /^Restore/ });
    expect(buttons).toHaveLength(1);
    expect(buttons[0]).toHaveAccessibleName("Restore air-ingest");
    await userEvent.click(buttons[0]);
    expect(restored).toEqual(["/api/v1/projects/helsinki/changes/chg-00000007/restore"]);
    expect(await screen.findByText("chg-00000009")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: en.changes.review })).toHaveAttribute(
      "href",
      "/projects/helsinki/approvals/chg-00000009",
    );
  });

  it("a_restore_the_server_refuses_is_said_in_its_words", async () => {
    const removal = closed("chg-00000007", {
      summary: { key: "change.summary.delete", params: { kind: "Pipeline", name: "air-ingest", fields: 0 } },
    });
    const { answer } = historyOf({ "1": { items: [removal] } });
    await renderRoute({
      path: PATH,
      answer: (path, request) =>
        path.endsWith("/restore")
          ? problem(409, "'pipeline.yaml' is in the project again, so change chg-00000007 has nothing left to restore")
          : answer(path, request),
    });
    const table = await openHistory();
    await userEvent.click(await within(table).findByRole("button", { name: "Restore air-ingest" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("is in the project again");
  });

  it("the_tabs_are_walked_by_arrow_keys", async () => {
    const { answer } = historyOf({ "1": { items: [] } });
    await renderRoute({ path: PATH, answer });
    const open = await screen.findByRole("tab", {
      name: en.approvals.tabs.open,
    });
    open.focus();
    await userEvent.keyboard("{ArrowRight}");
    expect(
      screen.getByRole("tab", { name: en.approvals.tabs.history }),
    ).toHaveAttribute("aria-selected", "true");
    expect(
      await screen.findByRole("table", { name: en.approvals.history.caption }),
    ).toBeInTheDocument();
  });

  it("says_it_in_all_four_languages", async () => {
    for (const locale of LOCALES) {
      const { answer } = historyOf({ "1": { items: [] } });
      const { unmount } = await renderRoute({ path: PATH, locale, answer });
      await userEvent.click(
        await screen.findByRole("tab", {
          name: i18n.t("approvals.tabs.history"),
        }),
      );
      expect(
        await screen.findByText(i18n.t("approvals.history.empty")),
      ).toBeInTheDocument();
      unmount();
      vi.restoreAllMocks();
    }
  });

  it("mounted_on_its_own_it_asks_for_the_first_page_and_shows_who_decided", async () => {
    renderPage(<ApprovalsHistory project="helsinki" />, {
      answer: (url) =>
        url.pathname.endsWith("/changes/history")
          ? json({
              apiVersion: "joinedcontext.com/v1alpha1",
              kind: "ChangeList",
              items: [REJECTED],
            })
          : undefined,
      path: "/projects/helsinki/approvals",
    });
    expect(
      await screen.findByText("Reason: the URL is the old one"),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("search", { name: en.approvals.history.filter }),
    ).toBeInTheDocument();
  });

  // T-3274: by person and by day, each row in words, what it changed in words, and Undo.
  it("filters_by_person_and_day_and_says_each_change_and_its_fields_in_words", async () => {
    const update = closed("chg-00000006", {
      summary: { key: "change.summary.update", params: { kind: "Endpoint", name: "public-air", fields: 1 } },
    });
    const { answer, asked } = historyOf({ "1": { items: [update] } });
    const undone: string[] = [];
    await renderRoute({
      path: PATH,
      answer: (path, request) => {
        if (path.endsWith("/changes/chg-00000006") && request.method === "GET") {
          return jsonResponse({ ...update, planFields: [{ path: "spec.audience", from: "organization", to: "public" }, { path: "spec.rateLimitClass", to: "strict" }] });
        }
        if (path.endsWith("/undo") && request.method === "POST") {
          undone.push(path);
          return jsonResponse(
            { apiVersion: "joinedcontext.com/v1alpha1", kind: "Change", metadata: { name: "chg-0000000a", namespace: "helsinki" }, status: { lane: "yellow", phase: "PendingApproval", plan: { create: 0, update: 1, delete: 0 } } },
            202,
          );
        }
        return answer(path, request);
      },
    });
    const table = await openHistory();
    const row = (await within(table).findByRole("link", { name: "Jana Kováčová changed Endpoint public-air" })).closest("tr") as HTMLElement;
    await userEvent.click(within(row).getByRole("button", { name: en.approvals.history.whatChanged }));
    expect(await within(row).findByText("changed Audience from organization to public")).toBeInTheDocument();
    expect(within(row).getByText("set Rate limit class to strict")).toBeInTheDocument();

    await userEvent.type(screen.getByLabelText(en.approvals.history.author), "jana");
    await userEvent.type(screen.getByLabelText(en.approvals.history.since), "2026-10-01");
    await userEvent.click(screen.getByRole("button", { name: en.approvals.history.apply }));
    await waitFor(() => expect(asked.at(-1)).toBe("?page=1&author=jana&since=2026-10-01"));

    await userEvent.click(await within(await openHistory()).findByRole("button", { name: "Undo the change to public-air" }));
    expect(undone).toEqual(["/api/v1/projects/helsinki/changes/chg-00000006/undo"]);
    expect(await screen.findByText("chg-0000000a")).toBeInTheDocument();
  });
});

