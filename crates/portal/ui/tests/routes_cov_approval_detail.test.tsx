/**
 * ApprovalDetailPage: Retry after a failed read, and the Cancel button of the reject dialog (the
 * dialog's own close control is a different button).
 */
import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { jsonResponse, problem, renderRoute } from "./pageHarness";

const ROUTE = "/projects/helsinki/approvals/chg-1a2b3c4d";

const PENDING = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ChangeProposal",
  metadata: { name: "chg-1a2b3c4d", namespace: "helsinki" },
  summary: { key: "change.summary.update", params: { kind: "Endpoint", name: "public-air", fields: 1 } },
  author: { name: "Marek Mráz", email: "marek@hel.fi" },
  createdAt: "2026-03-03T12:00:00Z",
  status: { lane: "yellow", phase: "PendingApproval", plan: { update: 1 } },
  planFields: [{ path: "spec.audience", from: "public", to: "internal" }],
};

describe("the change under review", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("reads the change again when Retry is pressed after a failed read", async () => {
    const user = userEvent.setup();
    let reads = 0;
    const { calls } = await renderRoute({
      path: ROUTE,
      answer: (path) => {
        if (!path.endsWith("/changes/chg-1a2b3c4d")) return undefined;
        reads += 1;
        return reads === 1 ? problem(503, "The change store did not answer.") : jsonResponse(PENDING);
      },
    });
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("The change store did not answer.");
    await user.click(within(alert).getByRole("button", { name: en.app.error.retry }));
    expect(await screen.findByRole("heading", { level: 1, name: /public-air/ })).toBeInTheDocument();
    expect(reads).toBe(2);
    expect(calls().filter((call) => call === "GET /api/v1/projects/helsinki/changes/chg-1a2b3c4d")).toHaveLength(2);
  });

  it("sends nothing when the reject dialog is dismissed with its Cancel button", async () => {
    const user = userEvent.setup();
    const { calls } = await renderRoute({
      path: ROUTE,
      answer: (path) => (path.endsWith("/changes/chg-1a2b3c4d") ? jsonResponse(PENDING) : undefined),
    });
    await user.click(await screen.findByRole("button", { name: en.approvals.reject }));
    const dialog = await screen.findByRole("dialog", { name: en.approvals.rejectTitle });
    const [, cancel] = within(dialog).getAllByRole("button", { name: en.approvals.rejectCancel });
    await user.click(cancel);
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(calls().filter((call) => call.startsWith("POST"))).toEqual([]);
  });
});
