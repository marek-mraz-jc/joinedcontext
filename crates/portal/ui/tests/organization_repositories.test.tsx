/**
 * T-7409: Administration → Repositories. The Organization's repositories in the shared grid, each
 * with a Download link to the streamed archive; nothing yet says what to do; a refused list shows
 * the server's reason; and nothing is fetched for anybody but an administrator.
 */
import { cleanup, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import type { Effective } from "../src/api/permissions";
import { Repositories, archiveHref } from "../src/pages/organization/Repositories";
import { expectNoAxeViolations, json, renderPage } from "./page_contract";

const ADMIN = {
  project: "org",
  bootstrap: false,
  grants: [{ role: "org-admin", binding: "admins", rule: { kinds: ["RoleBinding"], verbs: ["propose", "approve", "delete"] } }],
} as unknown as Effective;
const VIEWER = { project: "org", bootstrap: false, grants: [] } as unknown as Effective;

const LISTING = {
  organization: "bbsk",
  maxArchiveBytes: 67108864,
  repositories: [
    { name: "air-quality", role: "project", project: "air-quality", app: null, defaultBranch: "main", updatedAt: "2026-11-18T09:30:00Z", sizeBytes: 20480, archived: false },
    { name: "air-quality_air-map", role: "application", project: "air-quality", app: "air-map", defaultBranch: "main", updatedAt: "2026-11-17T12:00:00Z", sizeBytes: 3145728, archived: true },
  ],
};

function render(permissions: Effective, answer: () => Response) {
  const asked: string[] = [];
  const result = renderPage(<Repositories />, {
    path: "/organization/repositories",
    answer: async (url) => {
      asked.push(url.pathname);
      if (url.pathname.endsWith("/permissions/me")) return json(permissions);
      if (url.pathname === "/api/v1/organization/repositories") return answer();
      return json({}, 404);
    },
  });
  return { ...result, asked };
}

describe("Administration → Repositories", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(cleanup);

  it("lists the repositories with what each holds and a download of its archive", async () => {
    const { container } = render(ADMIN, () => json(LISTING));
    const app = await screen.findByText("air-quality_air-map");
    const row = app.closest("tr") ?? app.closest("li");
    expect(row).not.toBeNull();
    const scope = within(row as HTMLElement);
    expect(scope.getByText("App source")).toBeTruthy();
    expect(scope.getByText("App air-map in air-quality")).toBeTruthy();
    expect(scope.getByText("Archived")).toBeTruthy();
    expect(scope.getByText("3 MB")).toBeTruthy();
    const link = scope.getByRole("link", { name: "Download air-quality_air-map as .tar.gz" });
    expect(link.getAttribute("href")).toBe("/api/v1/organization/repositories/air-quality_air-map/archive");
    expect(link.hasAttribute("download")).toBe(true);
    expect(screen.getByText(/Archives up to 64 MB download here/)).toBeTruthy();
    await expectNoAxeViolations(container);
  });

  it("disables the download of a repository past the limit and says why", async () => {
    const big = { ...LISTING.repositories[0], name: "city-scans", sizeBytes: 94371840 };
    render(ADMIN, () => json({ ...LISTING, repositories: [big] }));
    const button = await screen.findByRole("button", { name: "Download" });
    expect((button as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByText("Larger than 64 MB: clone it with git instead.")).toBeTruthy();
    expect(screen.queryByRole("link", { name: /Download city-scans/ })).toBeNull();
  });

  it("says what to do when the Organization has no repository yet", async () => {
    render(ADMIN, () => json({ ...LISTING, repositories: [] }));
    expect(await screen.findByText("No repositories yet")).toBeTruthy();
    expect(screen.getByText(/Create a project under Projects first/)).toBeTruthy();
    expect(screen.getByRole("link", { name: "Open Projects" }).getAttribute("href")).toBe("/organization/projects");
  });

  it("shows the server's reason when the list fails", async () => {
    render(ADMIN, () => json({ title: "Service Unavailable", status: 503, detail: "this Portal has no forge configured" }, 503));
    expect(await screen.findByText(/this Portal has no forge configured/)).toBeTruthy();
  });

  it("fetches nothing for somebody who does not administer the Organization", async () => {
    const { asked } = render(VIEWER, () => json(LISTING));
    expect(await screen.findByText(/administrator/i)).toBeTruthy();
    expect(asked).not.toContain("/api/v1/organization/repositories");
  });

  it("encodes a repository name into the download address", () => {
    expect(archiveHref("a b/c")).toBe("/api/v1/organization/repositories/a%20b%2Fc/archive");
  });
});
