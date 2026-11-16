/**
 * T-7154: Administration → Backups. Nothing set up says what to do; a field the server refuses is
 * marked under that field; a saved bucket never shows its secret; a deleted backup says its
 * versions are kept; and nothing is fetched for anybody but an administrator.
 * T-7164: a restore lists the version's people missing today to invite by hand, never proposes them.
 */
import { cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { Effective } from "../src/api/permissions";
import { Backups } from "../src/pages/organization/Backups";
import { expectNoAxeViolations, json, renderPage } from "./page_contract";

const ADMIN = {
  project: "org",
  bootstrap: false,
  grants: [{ role: "org-admin", binding: "admins", rule: { kinds: ["RoleBinding"], verbs: ["propose", "approve", "delete"] } }],
} as unknown as Effective;

const VIEWER = { project: "org", bootstrap: false, grants: [] } as unknown as Effective;

const TARGET = {
  organization: "bbsk",
  kind: "configuration",
  endpoint: "https://s3.eu-central-1.amazonaws.com",
  region: "eu-central-1",
  bucket: "city-backups",
  prefix: "jc/",
  accessKeyId: "AKIAEXAMPLE",
  enabled: true,
  everyHours: 24,
  keepDays: 90,
  updatedBy: "ada",
  updatedAt: "2026-11-16T09:00:00Z",
  lastRunAt: null,
};

function render(permissions: Effective, target: unknown, refuse = false, deleted = false) {
  const requests: { method: string; path: string; body?: string }[] = [];
  const result = renderPage(<Backups />, {
    path: "/organization/backups",
    answer: async (url, request) => {
      const method = request.method;
      requests.push({ method, path: url.pathname, body: method === "GET" ? undefined : await request.clone().text() });
      if (url.pathname.endsWith("/permissions/me")) return json(permissions);
      if (url.pathname === "/api/v1/organization/backups/configuration" && method === "PUT") {
        if (refuse)
          return json(
            { title: "Unprocessable Content", status: 422, detail: "bucket: 3 to 63 lower-case letters", errors: [{ path: "bucket", message: "3 to 63 lower-case letters" }] },
            422,
          );
        return json(TARGET);
      }
      if (url.pathname === "/api/v1/organization/backups/configuration") return json({ target, log: [] });
      if (url.pathname.endsWith("/restore"))
        return json({
          organization: "bbsk",
          versionId: "v1",
          takenAt: "2026-11-16T09:00:00Z",
          changes: 0,
          groups: [
            {
              name: "organization/people",
              created: ["aino <aino@example.org>"],
              changed: [],
              unchanged: 3,
              note: "a restore never creates people: invite these by hand on People",
            },
          ],
        });
      if (url.pathname.endsWith("/versions"))
        return json({
          versioning: "Enabled",
          objectLock: false,
          versions: deleted
            ? [
                { versionId: "m2", lastModified: "2026-11-16T10:00:00Z", size: 0, isLatest: true, deleteMarker: true },
                { versionId: "v1", lastModified: "2026-11-16T09:00:00Z", size: 2048, isLatest: false, deleteMarker: false },
              ]
            : [],
        });
      return undefined;
    },
  });
  return { ...result, requests };
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(() => {
  cleanup();
});

describe("Administration → Backups", () => {
  it("says what to do when nothing is set up, and every field has a hint", async () => {
    const { container } = render(ADMIN, null);
    expect(await screen.findByText(en.backups.form.emptyTitle)).toBeInTheDocument();
    for (const hint of Object.values(en.backups.hint)) expect(screen.getByText(hint)).toBeInTheDocument();
    await expectNoAxeViolations(container);
  });

  it("marks the field the server refuses, under that field", async () => {
    const { requests } = render(ADMIN, null, true);
    fireEvent.change(await screen.findByLabelText(en.backups.form.bucket), { target: { value: "City_Backups" } });
    fireEvent.click(screen.getByRole("button", { name: en.backups.form.save }));
    expect(await screen.findByText("3 to 63 lower-case letters")).toBeInTheDocument();
    const put = requests.find((r) => r.method === "PUT");
    expect(put?.body).toContain("City_Backups");
  });

  it("keeps the saved secret unseen and sends none unless one is typed", async () => {
    const { requests } = render(ADMIN, TARGET);
    expect(await screen.findByText("Runs every 24 hours, and whenever you press Run now.")).toBeInTheDocument();
    expect((screen.getByLabelText(en.backups.form.secretAccessKey) as HTMLInputElement).value).toBe("");
    fireEvent.click(screen.getByRole("button", { name: en.backups.form.save }));
    await waitFor(() => expect(requests.some((r) => r.method === "PUT")).toBe(true));
    const body = JSON.parse(requests.find((r) => r.method === "PUT")?.body ?? "{}") as { secretAccessKey?: unknown };
    expect(body.secretAccessKey).toBeNull();
  });

  it("says a deleted backup keeps its earlier versions", async () => {
    render(ADMIN, TARGET, false, true);
    expect(await screen.findByText(en.backups.versions.deletedHint)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: en.backups.versions.restore })).toBeInTheDocument();
  });

  it("lists the version's people missing today to invite by hand, and proposes no restore for them", async () => {
    const { container } = render(ADMIN, TARGET, false, true);
    fireEvent.click(await screen.findByRole("button", { name: en.backups.versions.restore }));
    const plan = await screen.findByTestId("backup-restore-plan");
    expect(await screen.findByText(en.backups.restore.people)).toBeInTheDocument();
    expect(screen.getByText("aino <aino@example.org>")).toBeInTheDocument();
    expect(screen.getByText(en.backups.restore.peopleNote)).toBeInTheDocument();
    expect(plan).not.toHaveTextContent(en.backups.restore.organizationNote);
    expect(screen.queryByRole("button", { name: en.backups.restore.propose })).toBeNull();
    await expectNoAxeViolations(container);
  });

  it("fetches nothing of the backup for a person who does not administer the Organization", async () => {
    const { requests } = render(VIEWER, TARGET);
    expect(await screen.findByText(en.organization.adminOnly)).toBeInTheDocument();
    expect(requests.some((r) => r.path.includes("/backups/"))).toBe(false);
  });
});
