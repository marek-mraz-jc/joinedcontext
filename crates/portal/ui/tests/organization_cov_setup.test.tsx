/**
 * Organization → Setup: a failed read says why and Retry asks again; a refusal says it and offers
 * no retry.
 */
import { cleanup, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { Effective } from "../src/api/permissions";
import { OrganizationPage } from "../src/pages/organization/OrganizationPage";
import { json, problem, renderPage } from "./page_contract";

const ADMIN = {
  project: "org",
  bootstrap: false,
  grants: [{ role: "org-admin", binding: "admins", rule: { kinds: ["Organization", "Project", "RoleBinding"], verbs: ["propose", "approve", "delete"] } }],
} as unknown as Effective;

const DONE = {
  complete: true,
  steps: ["organization", "domain", "people", "project", "publishers", "policies"].map((id) => ({ id, done: true })),
  operator: ["branding", "loginTheme", "smtp", "backups"].map((id) => ({ id, done: true })),
};

beforeEach(async () => {
  await i18n.changeLanguage("en");
});
afterEach(cleanup);

function renderSetup(answer: () => Response) {
  const reads: string[] = [];
  renderPage(<OrganizationPage tab="setup" anchor="helsinki" />, {
    path: "/organization/setup",
    answer: (url, request) => {
      if (url.pathname.endsWith("/permissions/me")) return json(ADMIN);
      if (url.pathname === "/api/v1/organization/setup") {
        reads.push(request.method);
        return answer();
      }
      return undefined;
    },
  });
  return reads;
}

describe("Organization → Setup: a read that fails", () => {
  it("says the server could not answer and asks again on Retry", async () => {
    let healthy = false;
    const reads = renderSetup(() => (healthy ? json(DONE) : problem(503, "The setup index is restarting.")));
    expect(await screen.findByText(/The setup index is restarting/)).toBeInTheDocument();
    healthy = true;
    await userEvent.setup().click(screen.getByRole("button", { name: en.app.error.retry }));
    expect(await screen.findByText(en.organization.setup.complete)).toBeInTheDocument();
    expect(reads).toEqual(["GET", "GET"]);
    expect(screen.queryByText(/restarting/)).toBeNull();
  });

  it("says a refusal as it is, with nothing to retry", async () => {
    renderSetup(() => problem(403, "Only an administrator reads the setup."));
    expect(await screen.findByText(/Only an administrator reads the setup/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: en.app.error.retry })).toBeNull();
  });
});
