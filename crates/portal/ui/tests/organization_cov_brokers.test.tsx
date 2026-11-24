/**
 * Organization → Brokers: a failed read says why and Retry asks again; a refusal offers no retry.
 */
import { cleanup, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { Effective } from "../src/api/permissions";
import { Brokers } from "../src/pages/organization/Brokers";
import { json, problem, renderPage } from "./page_contract";

const ADMIN = {
  project: "org",
  bootstrap: false,
  grants: [{ role: "org-admin", binding: "admins", rule: { kinds: ["RoleBinding"], verbs: ["propose", "approve", "delete"] } }],
} as unknown as Effective;

const MAIN = {
  name: "main",
  declared: true,
  default: true,
  size: "small",
  features: [],
  health: "up",
  version: "0.14.2",
  reason: null,
  spaces: ["air/obs"],
  entities: 7,
  observedAt: "2026-10-12T08:00:00Z",
};

beforeEach(async () => {
  await i18n.changeLanguage("en");
});
afterEach(cleanup);

function renderBrokers(answer: () => Response) {
  const reads: string[] = [];
  renderPage(<Brokers />, {
    path: "/organization/brokers",
    answer: (url) => {
      if (url.pathname.endsWith("/permissions/me")) return json(ADMIN);
      if (url.pathname === "/api/v1/organization/brokers") {
        reads.push(url.pathname);
        return answer();
      }
      return undefined;
    },
  });
  return reads;
}

describe("Organization → Brokers: a read that fails", () => {
  it("keeps the table away, says why, and lists the brokers after Retry", async () => {
    let healthy = false;
    const reads = renderBrokers(() => (healthy ? json({ items: [MAIN] }) : problem(503, "The broker registry is not answering.")));
    expect(await screen.findByText(/The broker registry is not answering/)).toBeInTheDocument();
    expect(screen.queryByRole("table", { name: en.organization.brokers.caption })).toBeNull();
    healthy = true;
    await userEvent.setup().click(screen.getByRole("button", { name: en.app.error.retry }));
    const table = await screen.findByRole("table", { name: en.organization.brokers.caption });
    expect(within(table).getByText("0.14.2")).toBeInTheDocument();
    expect(reads).toHaveLength(2);
  });

  it("says a refusal as it is, with nothing to retry", async () => {
    renderBrokers(() => problem(403, "Brokers are for administrators."));
    expect(await screen.findByText(/Brokers are for administrators/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: en.app.error.retry })).toBeNull();
  });
});
