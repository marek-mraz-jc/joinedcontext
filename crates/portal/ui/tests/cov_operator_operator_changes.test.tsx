// T-7589: coverage for OperatorChanges (rejecting changes, column text extractors for search, target formatting)
import { cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { Effective } from "../src/api/permissions";
import { OperatorChanges } from "../src/pages/operator/OperatorChanges";
import { json, renderPage } from "./page_contract";
const OPERATOR = { project: "org", bootstrap: true, grants: [] } as unknown as Effective;

type Seen = { method: string; path: string; body?: string };

function renderChanges(
  answers: (path: string, method: string, body: string) => Response | undefined,
  permissions = OPERATOR,
) {
  const seen: Seen[] = [];
  renderPage(<OperatorChanges />, {
    path: "/organization/operator-changes",
    answer: async (url, request) => {
      const body = request.method === "GET" ? "" : await request.clone().text();
      seen.push({ method: request.method, path: url.pathname, body });
      if (url.pathname.endsWith("/permissions/me")) return json(permissions);
      return answers(url.pathname, request.method, body);
    },
  });
  return seen;
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(() => cleanup());

describe("OperatorChanges coverage", () => {

  it("rejects a proposed change and shows the rejection message", async () => {
    const items = [
      {
        id: "chg-to-reject",
        kind: "HostAdd",
        payload: { host: "api.bbsk.sk", organization: "bbsk" },
        state: "proposed",
        proposedBy: "other-user",
        proposedByName: "Iny Operator",
        proposedAt: "2026-11-17T08:00:00Z",
        decidedBy: null,
        decidedAt: null,
      },
    ];

    const seen = renderChanges((path, method) => {
      if (path === "/api/v1/operator/changes" && method === "GET") return json({ items });
      if (path === "/api/v1/operator/changes/chg-to-reject/reject" && method === "POST") {
        return json({ ...items[0], state: "rejected" });
      }
      return undefined;
    });

    const rejectButtons = await screen.findAllByRole("button", { name: en.operator.changes.reject });
    fireEvent.click(rejectButtons[0]);

    await waitFor(() => {
      expect(screen.getByText(en.operator.changes.rejected.replace("{target}", "api.bbsk.sk"))).toBeInTheDocument();
    });

    const post = seen.find((s) => s.method === "POST" && s.path === "/api/v1/operator/changes/chg-to-reject/reject");
    expect(post).toBeDefined();
  });

  it("shows an alert when rejecting a change fails", async () => {
    const items = [
      {
        id: "chg-fail",
        kind: "HostAdd",
        payload: { host: "fail.bbsk.sk", organization: "bbsk" },
        state: "proposed",
        proposedBy: "other-user",
        proposedByName: "Iny Operator",
        proposedAt: "2026-11-17T08:00:00Z",
        decidedBy: null,
        decidedAt: null,
      },
    ];

    renderChanges((path, method) => {
      if (path === "/api/v1/operator/changes" && method === "GET") return json({ items });
      if (path.endsWith("/reject") && method === "POST") {
        return json({ title: "Forbidden", status: 403, detail: "Cannot reject this change" }, 403);
      }
      return undefined;
    });

    const rejectButtons = await screen.findAllByRole("button", { name: en.operator.changes.reject });
    fireEvent.click(rejectButtons[0]);

    expect(await screen.findByText("Cannot reject this change")).toBeInTheDocument();
    expect(screen.getByText(en.operator.changes.refused)).toBeInTheDocument();
  });
});
