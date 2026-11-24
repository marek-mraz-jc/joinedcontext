/**
 * T-7588: Administration's side navigation for an Organization's own administrator (ADR-N-058
 * section 2.4): only the sections that are theirs, each group without one left out.
 */
import { screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { AdminNav } from "../src/pages/organization/AdminNav";
import { json, renderPage } from "./page_contract";

beforeEach(async () => {
  await i18n.changeLanguage("en");
});
afterEach(() => {
  vi.unstubAllGlobals();
});

describe("Administration's navigation for an Organization's own administrator", () => {
  it("links the Organization's own sections and none of the installation's", async () => {
    renderPage(<AdminNav project="helsinki" active="members" onNavigate={() => undefined} />, {
      path: "/organization/members",
      answer: (url) =>
        url.pathname.endsWith("/permissions/me")
          ? json({
              project: "org",
              bootstrap: false,
              grants: [{ role: "org-admin", binding: "zilina-admins", organization: "zilina", rule: { kinds: ["RoleBinding", "Group", "Organization"], verbs: ["propose", "approve"] } }],
            })
          : undefined,
    });
    const members = await screen.findByRole("link", { name: en.organization.tab.members });
    expect(members).toHaveAttribute("aria-current", "page");
    expect(screen.queryByRole("link", { name: en.organization.tab.brokers })).toBeNull();
    expect(screen.queryByText(en.organization.nav.notAdmin)).toBeNull();
  });
});
