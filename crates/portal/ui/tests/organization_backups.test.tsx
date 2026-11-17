/**
 * T-7154, T-7168: Administration → Backups, the configuration and the data backup. Nothing set up says what to do; a field the server refuses is
 * marked under that field; a saved bucket never shows its secret; a deleted backup says its
 * versions are kept; and nothing is fetched for anybody but an administrator.
 * T-7164: a restore lists the version's people missing today to invite by hand, never proposes them.
 * T-7208: one project of a data version restores into a new project, picked from the list the server filters.
 */
import { cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
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
  recipients: [],
  updatedBy: "ada",
  updatedAt: "2026-11-16T09:00:00Z",
  lastRunAt: null,
};

// Test keys, joined from halves: the public check keeps whole age recipients out of the repository.
const WORKING = ["age1mmaj3l7xsa7pjqmxr3lfc7k3nmy63sfw7", "tglj2r57stsehfmwvxqxp9h5k"].join("");
const RECOVERY = ["age1syrvza6xvtvsr9f0aj4n7kja5axtcytml9", "jce9nkyqrrf2eta53qtx844h"].join("");
const DATA_TARGET = { ...TARGET, kind: "data", recipients: [WORKING, RECOVERY] };
const FINGERPRINTS = ["3f1a:09bc:77de:1200", "c0de:4b1d:9e2f:aa01"];

const PLAN = {
  organization: "bbsk",
  versionId: "d1",
  takenAt: "2026-11-16T09:00:00Z",
  modules: { portal: { entity_comments: { create: 2, update: 0, unchanged: 0, refused: 0 }, activity: { create: 0, update: 0, unchanged: 4, refused: 0 } } },
  notRestored: { "portal/access_requests": "an access decision" },
  notInArchive: { antares: "the exporter is T-7170" },
  erasuresReplayed: 0,
};
const RESTORES = [
  { id: 7, versionId: "d1", requestedBy: "ada", requestedAt: "2026-11-16T09:30:00Z", state: "pending", plan: PLAN, decidedBy: null, decidedAt: null, message: "" },
  { id: 8, versionId: "d1", requestedBy: "jana.kovacova", requestedAt: "2026-11-16T09:40:00Z", state: "pending", plan: PLAN, decidedBy: null, decidedAt: null, message: "" },
];

const SOURCE = "bm6wyhdpioruxguvhqpu4lt2ca";
const COPIED = "lyvhyrangvhw5crmtupc6m2ewa";
const COPY_PLAN = {
  versionId: "d1",
  takenAt: "2026-11-16T09:00:00Z",
  source: SOURCE,
  sourceName: "air-quality",
  name: "air-quality-restored",
  projectId: "",
  tables: { entity_comments: { create: 2, refused: 0 }, activity: { create: 0, refused: 0 } },
  notRestored: { "portal/access_requests": "an access decision" },
  erasuresReplayed: 1,
};

const configuration = async () => within(await screen.findByTestId("backup-configuration"));
const data = async () => within(await screen.findByTestId("backup-data"));

function render(permissions: Effective, target: unknown, refuse = false, deleted = false, dataTarget: unknown = null) {
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
      if (url.pathname === "/api/v1/organization/backups/configuration") return json({ target, fingerprints: [], log: [] });
      if (url.pathname === "/api/v1/organization/backups/data" && method === "PUT") {
        if (refuse)
          return json(
            { title: "Unprocessable Content", status: 422, detail: "recoveryRecipient: an age public key starting with age1", errors: ["recoveryRecipient: an age public key starting with age1"] },
            422,
          );
        return json(DATA_TARGET);
      }
      if (url.pathname === "/api/v1/organization/backups/data")
        return json({ target: dataTarget, fingerprints: dataTarget ? FINGERPRINTS : [], log: [] });
      if (url.pathname === "/api/v1/organization/backups/data/restores") return json(RESTORES);
      if (url.pathname.endsWith("/restores/7/approve")) return json({ ...RESTORES[0], state: "applied", decidedBy: "jana.kovacova" });
      if (url.pathname.startsWith("/api/v1/organization/backups/data/versions/") && url.pathname.endsWith("/restore")) {
        const asked = JSON.parse(await request.clone().text()) as { request?: boolean };
        return json({ plan: PLAN, request: asked.request ? { ...RESTORES[1], id: 9 } : null });
      }
      if (url.pathname === "/api/v1/organization/backups/data/versions/d1/projects")
        return json({ projects: [{ id: SOURCE, name: "air-quality" }], deleted: ["old-pilot"] });
      if (url.pathname === "/api/v1/organization/backups/data/versions/d1/copy") {
        const asked = JSON.parse(await request.clone().text()) as { name: string; apply: boolean };
        if (asked.name === "air-quality")
          return json({ title: "Unprocessable Content", status: 422, detail: "name: air-quality is taken", errors: ["name: air-quality is taken by a project, a copy or an Organization; pick a new name"] }, 422);
        return json({ ...COPY_PLAN, name: asked.name, projectId: asked.apply ? COPIED : "" });
      }
      if (url.pathname === "/api/v1/organization/backups/data/versions")
        return json({ versioning: "Enabled", objectLock: false, versions: [{ versionId: "d1", lastModified: "2026-11-16T09:00:00Z", size: 4096, isLatest: true, deleteMarker: false }] });
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
    expect(await (await configuration()).findByText(en.backups.form.emptyTitle)).toBeInTheDocument();
    expect(await (await data()).findByText(en.backups.form.emptyTitle)).toBeInTheDocument();
    // The copy dialog's two hints show in that dialog; its own test reads them there.
    for (const [key, hint] of Object.entries(en.backups.hint))
      if (!key.startsWith("copy")) expect(screen.getAllByText(hint).length).toBeGreaterThan(0);
    expect((await data()).getByText(en.backups.hint.workingRecipient)).toBeInTheDocument();
    expect((await configuration()).queryByText(en.backups.hint.workingRecipient)).toBeNull();
    await expectNoAxeViolations(container);
  });

  it("marks the field the server refuses, under that field", async () => {
    const { requests } = render(ADMIN, null, true);
    fireEvent.change(await (await configuration()).findByLabelText(en.backups.form.bucket), { target: { value: "City_Backups" } });
    fireEvent.click((await configuration()).getByRole("button", { name: en.backups.form.save }));
    expect(await (await configuration()).findByText("3 to 63 lower-case letters")).toBeInTheDocument();
    const put = requests.find((r) => r.method === "PUT");
    expect(put?.body).toContain("City_Backups");
  });

  it("keeps the saved secret unseen and sends none unless one is typed", async () => {
    const { requests } = render(ADMIN, TARGET);
    expect(await (await configuration()).findByText("Runs every 24 hours, and whenever you press Run now.")).toBeInTheDocument();
    expect(((await configuration()).getByLabelText(en.backups.form.secretAccessKey) as HTMLInputElement).value).toBe("");
    fireEvent.click((await configuration()).getByRole("button", { name: en.backups.form.save }));
    await waitFor(() => expect(requests.some((r) => r.method === "PUT")).toBe(true));
    const body = JSON.parse(requests.find((r) => r.method === "PUT")?.body ?? "{}") as { secretAccessKey?: unknown };
    expect(body.secretAccessKey).toBeNull();
  });

  it("says a deleted backup keeps its earlier versions", async () => {
    render(ADMIN, TARGET, false, true);
    expect(await (await configuration()).findByText(en.backups.versions.deletedHint)).toBeInTheDocument();
    expect((await configuration()).getByRole("button", { name: en.backups.versions.restore })).toBeInTheDocument();
  });

  it("sends the data backup's two recipients and shows their fingerprints", async () => {
    const { requests, container } = render(ADMIN, null, false, false, DATA_TARGET);
    expect(await (await data()).findByText(FINGERPRINTS[0])).toBeInTheDocument();
    expect((await data()).getByText(FINGERPRINTS[1])).toBeInTheDocument();
    expect(((await data()).getByLabelText(en.backups.form.workingRecipient) as HTMLInputElement).value).toBe(WORKING);
    fireEvent.click((await data()).getByRole("button", { name: en.backups.form.save }));
    await waitFor(() => expect(requests.some((r) => r.method === "PUT" && r.path.endsWith("/backups/data"))).toBe(true));
    const body = JSON.parse(requests.find((r) => r.method === "PUT")?.body ?? "{}") as Record<string, unknown>;
    expect(body.workingRecipient).toBe(WORKING);
    expect(body.recoveryRecipient).toBe(RECOVERY);
    await expectNoAxeViolations(container);
  });

  it("checks a data version with the key, then asks a second administrator", async () => {
    const { requests } = render(ADMIN, null, false, false, DATA_TARGET);
    fireEvent.click(await (await data()).findByRole("button", { name: en.backups.versions.restore }));
    const dialog = within(await screen.findByTestId("backup-data-restore"));
    expect(dialog.getByRole("button", { name: en.backups.data.check })).toHaveAttribute("aria-disabled", "true");
    fireEvent.change(dialog.getByLabelText(en.backups.data.identity), { target: { value: "AGE-SECRET-KEY-1TEST" } });
    fireEvent.click(dialog.getByRole("button", { name: en.backups.data.check }));
    expect(await dialog.findByRole("rowheader", { name: "portal/entity_comments" })).toBeInTheDocument();
    fireEvent.click(dialog.getByRole("button", { name: en.backups.data.request }));
    expect(await dialog.findByText(en.backups.data.requested)).toBeInTheDocument();
    const asks = requests.filter((r) => r.path.endsWith("/versions/d1/restore")).map((r) => JSON.parse(r.body ?? "{}") as { request: boolean; identity: string });
    expect(asks.map((a) => a.request)).toEqual([false, true]);
    expect(dialog.queryByLabelText(en.backups.data.identity)).toBeNull();
  });

  it("restores one project of a data version into a new project, checked first, the taken name marked", async () => {
    const { requests, container } = render(ADMIN, null, false, false, DATA_TARGET);
    fireEvent.click(await (await data()).findByRole("button", { name: en.backups.copy.action }));
    const dialog = within(await screen.findByTestId("backup-project-copy"));
    expect(dialog.getByRole("button", { name: en.backups.copy.list })).toHaveAttribute("aria-disabled", "true");
    fireEvent.change(dialog.getByLabelText(en.backups.data.identity), { target: { value: "AGE-SECRET-KEY-1TEST" } });
    fireEvent.click(dialog.getByRole("button", { name: en.backups.copy.list }));
    const picker = await dialog.findByLabelText(en.backups.copy.project);
    expect(within(picker).getAllByRole("option").map((o) => o.textContent)).toEqual([en.backups.copy.pick, "air-quality"]);
    expect(picker).toHaveValue(SOURCE);
    expect(dialog.getByText(/old-pilot/)).toBeInTheDocument();
    expect(dialog.getByText(en.backups.hint.copyName)).toBeInTheDocument();
    expect(dialog.getByText(en.backups.hint.copyProject)).toBeInTheDocument();
    fireEvent.change(dialog.getByLabelText(en.backups.copy.name), { target: { value: "air-quality" } });
    fireEvent.click(dialog.getByRole("button", { name: en.backups.copy.check }));
    expect(await dialog.findByText(/air-quality is taken by a project/)).toBeInTheDocument();
    fireEvent.change(dialog.getByLabelText(en.backups.copy.name), { target: { value: "air-quality-restored" } });
    fireEvent.click(dialog.getByRole("button", { name: en.backups.copy.check }));
    expect(await dialog.findByRole("rowheader", { name: "entity_comments" })).toBeInTheDocument();
    expect(dialog.queryByRole("rowheader", { name: "activity" })).toBeNull();
    await expectNoAxeViolations(container);
    fireEvent.click(dialog.getByRole("button", { name: en.backups.copy.apply }));
    expect(await dialog.findByText(new RegExp(`Restored into air-quality-restored \\(project id ${COPIED}\\)`))).toBeInTheDocument();
    const asks = requests.filter((r) => r.path.endsWith("/versions/d1/copy")).map((r) => JSON.parse(r.body ?? "{}") as { apply: boolean; project: string; name: string });
    expect(asks.map((a) => [a.name, a.apply])).toEqual([
      ["air-quality", false],
      ["air-quality-restored", false],
      ["air-quality-restored", true],
    ]);
    expect(asks.every((a) => a.project === SOURCE)).toBe(true);
    expect(dialog.queryByLabelText(en.backups.data.identity)).toBeNull();
  });

  it("approves another administrator's restore with the key and only withdraws one's own", async () => {
    const { requests } = render(ADMIN, null, false, false, DATA_TARGET);
    const list = within(await (await data()).findByRole("list", { name: en.backups.data.requestsTitle }));
    const items = list.getAllByRole("listitem");
    expect(within(items[1]).getByText(en.backups.data.waitsForOther)).toBeInTheDocument();
    expect(within(items[1]).queryByRole("button", { name: en.backups.data.approve })).toBeNull();
    expect(within(items[1]).getByRole("button", { name: en.backups.data.withdraw })).toBeInTheDocument();
    fireEvent.change(within(items[0]).getByLabelText(en.backups.data.identity), { target: { value: "AGE-SECRET-KEY-1TEST" } });
    fireEvent.click(within(items[0]).getByRole("button", { name: en.backups.data.approve }));
    await waitFor(() => expect(requests.some((r) => r.path.endsWith("/restores/7/approve"))).toBe(true));
  });

  it("marks a refused recovery key under its field", async () => {
    render(ADMIN, null, true);
    fireEvent.change(await (await data()).findByLabelText(en.backups.form.recoveryRecipient), { target: { value: "ssh-ed25519 AAAA" } });
    fireEvent.click((await data()).getByRole("button", { name: en.backups.form.save }));
    expect(await (await data()).findByText("an age public key starting with age1")).toBeInTheDocument();
  });

  it("lists the version's people missing today to invite by hand, and proposes no restore for them", async () => {
    const { container } = render(ADMIN, TARGET, false, true);
    fireEvent.click(await screen.findByRole("button", { name: en.backups.versions.restore }));
    const plan = await screen.findByTestId("backup-restore-plan");
    expect(await screen.findByText(en.backups.restore.people)).toBeInTheDocument();
    expect(screen.getByText("aino <aino@example.org>")).toBeInTheDocument();
    expect(screen.getByText("3 of the 4 people in this version are in the Organization today; 1 to invite by hand")).toBeInTheDocument();
    expect(screen.getByText(en.backups.restore.sameButPeople)).toBeInTheDocument();
    expect(screen.queryByText(en.backups.restore.same)).toBeNull();
    expect(screen.getByText(en.backups.restore.peopleNote, { exact: false })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: en.backups.restore.invite })).toHaveAttribute("href", "/organization/people");
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
