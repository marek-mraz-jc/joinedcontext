/**
 * T-7588: the Organization page's remaining cases: each installation operator's section loads
 * with its tab for an operator, and the project list says why its titles or people counts could
 * not be read and reads both again on Retry.
 */
import { cleanup, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { OrganizationPage } from "../src/pages/organization/OrganizationPage";
import type { OrganizationTab } from "../src/pages/organization/OrganizationPage";
import { json, list, problem, renderPage } from "./page_contract";

const OPERATOR = {
  project: "org",
  bootstrap: true,
  grants: [{ role: "org-admin", binding: "admins", rule: { kinds: ["RoleBinding", "Role", "Group", "Organization", "Project"], verbs: ["propose", "approve", "delete"] } }],
};

function render(tab: OrganizationTab, answer: (url: URL) => Response | undefined = () => undefined) {
  renderPage(<OrganizationPage tab={tab} anchor="helsinki" />, {
    path: `/organization/${tab}`,
    answer: (url) => {
      if (url.pathname.endsWith("/permissions/me")) return json(OPERATOR);
      return answer(url);
    },
  });
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("the Organization page for an installation operator", () => {
  for (const tab of ["operator-organizations", "operator-hosts", "operator-installation", "operator-changes", "operator-audit"] as const) {
    it(`loads the ${tab} section with its tab`, async () => {
      render(tab);
      await waitFor(() => expect(document.getElementById(`${tab}-heading`)).not.toBeNull(), { timeout: 15_000 });
      expect(document.getElementById(`${tab}-heading`)?.tagName).toBe("H2");
    });
  }
});

describe("the Organization's projects", () => {
  it("says why the titles and counts are missing and reads both again on Retry", async () => {
    const user = userEvent.setup();
    let manifests = 0;
    let bindings = 0;
    render("projects", (url) => {
      if (url.pathname === "/api/v1/projects") return json(list([{ name: "helsinki" }]));
      if (url.pathname === "/api/v1/projects/org/projects") return ++manifests === 1 ? problem(503, "the manifest store is restarting") : json(list([]));
      if (url.pathname === "/api/v1/projects/org/rolebindings") return ++bindings === 1 ? problem(503, "the bindings are not loaded yet") : json(list([]));
      return undefined;
    });
    expect(await screen.findByText(/the manifest store is restarting/)).toBeInTheDocument();
    await user.click(screen.getAllByRole("button", { name: en.app.error.retry }).at(-1) as HTMLElement);
    await waitFor(() => expect(manifests).toBe(2));
    expect(bindings).toBe(2);
    await waitFor(() => expect(screen.queryByText(/the manifest store is restarting/)).toBeNull());
  });
});
