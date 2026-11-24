// T-7589: coverage for OperatorHosts (column text extractors for search, cancel remove dialog)
import { cleanup, fireEvent, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { Effective } from "../src/api/permissions";
import { OperatorHosts } from "../src/pages/operator/OperatorHosts";
import { json, renderPage } from "./page_contract";

const OPERATOR = { project: "org", bootstrap: true, grants: [] } as unknown as Effective;

function renderHosts(
  answers: (path: string, method: string, body: string) => Response | undefined,
  permissions = OPERATOR,
) {
  renderPage(<OperatorHosts />, {
    path: "/organization/operator-hosts",
    answer: async (url, request) => {
      const body = request.method === "GET" ? "" : await request.clone().text();
      if (url.pathname.endsWith("/permissions/me")) return json(permissions);
      return answers(url.pathname, request.method, body);
    },
  });
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(() => cleanup());

describe("OperatorHosts coverage", () => {

  it("cancels remove dialog and resets removing state", async () => {
    const items = [
      {
        host: "smartcity.bbsk.sk",
        organization: "bbsk",
        organizationId: "sk_bbsk",
        role: "working",
        state: "served",
        detail: null,
        detailAt: null,
        updatedAt: "2026-11-16T09:00:00Z",
      },
    ];

    renderHosts((path) => {
      if (path === "/api/v1/operator/hosts") {
        return json({ items, certificates: { usedToday: 1, perDay: 10, perDomainWeek: 20, nextFreeAt: null } });
      }
      return undefined;
    });

    const removeBtn = (await screen.findAllByRole("button", { name: en.operator.hosts.remove }))[0];
    fireEvent.click(removeBtn);

    const dialog = await screen.findByRole("alertdialog");
    expect(dialog).toBeInTheDocument();
    expect(within(dialog).getByText(en.operator.hosts.removeLead)).toBeInTheDocument();

    const cancelBtn = within(dialog).getByRole("button", { name: en.form.cancel });
    fireEvent.click(cancelBtn);

    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
  });

  it("handles remove failure in dialog and resets on cancel", async () => {
    const items = [
      {
        host: "error.bbsk.sk",
        organization: "bbsk",
        organizationId: "sk_bbsk",
        role: "working",
        state: "served",
        detail: null,
        detailAt: null,
        updatedAt: "2026-11-16T09:00:00Z",
      },
    ];

    renderHosts((path, method) => {
      if (path === "/api/v1/operator/hosts") {
        return json({ items, certificates: { usedToday: 1, perDay: 10, perDomainWeek: 20, nextFreeAt: null } });
      }
      if (path === "/api/v1/operator/changes" && method === "POST") {
        return json({ title: "Internal Server Error", status: 500, detail: "Cannot remove host" }, 500);
      }
      return undefined;
    });

    const removeBtn = (await screen.findAllByRole("button", { name: en.operator.hosts.remove }))[0];
    fireEvent.click(removeBtn);

    const dialog = await screen.findByRole("alertdialog");
    const confirmBtn = within(dialog).getByRole("button", { name: en.operator.hosts.removeConfirm });
    fireEvent.click(confirmBtn);

    expect(await within(dialog).findByText("Cannot remove host")).toBeInTheDocument();

    const cancelBtn = within(dialog).getByRole("button", { name: en.form.cancel });
    fireEvent.click(cancelBtn);

    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
  });
});
