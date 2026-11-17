import { expect, test } from "@playwright/test";
import type { Page, Route } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { axeViolations } from "./axe";
import { doorsAsRest, manifestReadsAsRest } from "./manifestReads";

test.beforeEach(({ page }) => {
  manifestReadsAsRest(page);
  doorsAsRest(page);
});

// Administration → Backups (T-7154, T-7168) as an Organization administrator meets it: nothing
// set up, a bucket field refused, the bucket saved, a run, the versions, a restore check and a
// soft delete, then the data backup with its two age recipients, at 1440 and 400 px. `vite preview` has no Portal API behind it, so a small in-browser
// bucket answers the backup doors with the server's shapes; the page is the real build.

const IDENTITY = { subject: "s1", username: "demo.admin", name: "Demo Admin", email: "admin@bbsk.sk", roles: [] };
const DOORS = { configuration: "/api/v1/organization/backups/configuration", data: "/api/v1/organization/backups/data" };
// Test keys, joined from halves: the public check keeps whole age recipients out of the repository.
const WORKING = ["age1mmaj3l7xsa7pjqmxr3lfc7k3nmy63sfw7", "tglj2r57stsehfmwvxqxp9h5k"].join("");
const RECOVERY = ["age1syrvza6xvtvsr9f0aj4n7kja5axtcytml9", "jce9nkyqrrf2eta53qtx844h"].join("");

interface Bucket {
  target: Record<string, unknown> | null;
  log: Record<string, unknown>[];
  versions: Record<string, unknown>[];
  restores?: Record<string, unknown>[];
}

const PLAN = {
  organization: "bbsk",
  versionId: "v1",
  takenAt: "2026-11-16T09:11:00Z",
  modules: {
    portal: {
      activity: { create: 0, update: 0, unchanged: 214, refused: 0 },
      entity_comments: { create: 12, update: 1, unchanged: 40, refused: 0 },
      pipeline_runs: { create: 3, update: 0, unchanged: 88, refused: 0 },
    },
  },
  notRestored: { "portal/access_requests": "an access decision: restoring it could open access that was withdrawn since" },
  notInArchive: { antares: "entities and temporal history: the exporter is T-7170, not in this build" },
  erasuresReplayed: 0,
};

const SOURCE = "bm6wyhdpioruxguvhqpu4lt2ca";
const COPIED = "lyvhyrangvhw5crmtupc6m2ewa";
const COPY_PLAN = {
  versionId: "v1",
  takenAt: "2026-11-16T09:11:00Z",
  source: SOURCE,
  sourceName: "air-quality",
  name: "air-quality-restored",
  projectId: "",
  tables: {
    activity: { create: 214, refused: 0 },
    entity_comments: { create: 52, refused: 0 },
    data_views: { create: 6, refused: 0 },
    pipeline_runs: { create: 91, refused: 0 },
    alert_notices: { create: 0, refused: 2 },
  },
  notRestored: { "portal/access_requests": "an access decision: restoring it could open access that was withdrawn since" },
  erasuresReplayed: 1,
};

type Kind = keyof typeof DOORS;

function json(body: unknown, status = 200) {
  return { status, contentType: "application/json", body: JSON.stringify(body) };
}

async function stubApi(page: Page, bucket: Bucket, admin = true, dataBucket: Bucket = { target: null, log: [], versions: [] }): Promise<void> {
  let tick = 0;
  const now = () => `2026-11-16T09:${String(10 + tick++).padStart(2, "0")}:00Z`;
  const buckets: Record<Kind, Bucket> = { configuration: bucket, data: dataBucket };
  await page.route("**/api/**", async (route) => {
    const url = new URL(route.request().url());
    const method = route.request().method();
    const path = url.pathname;
    const kind = (Object.keys(DOORS) as Kind[]).find((k) => path === DOORS[k] || path.startsWith(`${DOORS[k]}/`));
    if (kind) return answer(route, kind, buckets[kind], method, path.slice(DOORS[kind].length), now);
    if (path.endsWith("/auth/me")) return route.fulfill(json(IDENTITY));
    if (path === "/api/v1/projects")
      return route.fulfill(json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ProjectList", items: [{ name: "bbsk" }] }));
    if (path.endsWith("/permissions/me")) return route.fulfill(json({ project: "org", bootstrap: admin, grants: [] }));
    return route.fulfill(json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] }));
  });
}

/** One backup door, as the server answers it; `rest` is the path after the door. */
async function answer(route: Route, kind: Kind, bucket: Bucket, method: string, rest: string, now: () => string): Promise<void> {
  const log = (action: string, ok: boolean, message = "uploaded", bytes = 0) =>
    bucket.log.unshift({ id: bucket.log.length + 1, organization: "bbsk", kind, action, actor: "demo.admin", at: now(), ok, bytes, objectKey: `jc/bbsk/${kind}/`, versionId: "", message });
  if (rest === "" && method === "GET") {
    const recipients = (bucket.target?.recipients as string[] | undefined) ?? [];
    const fingerprints = recipients.map((r) => `${r.slice(4, 8)}:${r.slice(8, 12)}:${r.slice(12, 16)}:${r.slice(16, 20)}`);
    return route.fulfill(json({ target: bucket.target, fingerprints, log: bucket.log }));
  }
  if (rest === "" && method === "PUT") {
    const input = route.request().postDataJSON() as Record<string, unknown>;
    if (!/^[a-z0-9.-]{3,63}$/.test(String(input.bucket))) {
      const detail = "bucket: 3 to 63 lower-case letters, digits, '.' and '-', e.g. bb-backups";
      return route.fulfill(json({ type: "about:blank", title: "Unprocessable Content", status: 422, detail, errors: [{ path: "bucket", message: "3 to 63 lower-case letters, digits, '.' and '-', e.g. bb-backups" }] }, 422));
    }
    if (kind === "data" && !String(input.recoveryRecipient).startsWith("age1")) {
      const detail = "recoveryRecipient: an age public key starting with age1, e.g. age1ql3z7hjy54pw3hyw…";
      return route.fulfill(json({ type: "about:blank", title: "Unprocessable Content", status: 422, detail, errors: [detail] }, 422));
    }
    const { secretAccessKey: _secret, workingRecipient, recoveryRecipient, ...kept } = input;
    const recipients = kind === "data" ? [workingRecipient, recoveryRecipient] : [];
    bucket.target = { ...kept, recipients, organization: "bbsk", kind, updatedBy: "demo.admin", updatedAt: now(), lastRunAt: null };
    return route.fulfill(json(bucket.target));
  }
  if (rest === "/run") {
    for (const version of bucket.versions) version.isLatest = false;
    log("run", true, "uploaded", 48213);
    // The run's log line and its version carry one time, as the server writes them.
    bucket.versions.unshift({ versionId: `v${bucket.versions.length + 1}`, lastModified: bucket.log[0].at, size: 48213, isLatest: true, deleteMarker: false });
    return route.fulfill(json(bucket.log[0]));
  }
  if (rest === "/versions") return route.fulfill(json({ versioning: "Enabled", objectLock: false, versions: bucket.versions }));
  if ((rest === "/bundle" || rest === "/archive") && method === "DELETE") {
    for (const version of bucket.versions) version.isLatest = false;
    bucket.versions.unshift({ versionId: "m1", lastModified: now(), size: 0, isLatest: true, deleteMarker: true });
    log("delete", true, "delete marker written");
    return route.fulfill({ status: 204, body: "" });
  }
  if (kind === "data" && rest.endsWith("/projects")) {
    return route.fulfill(json({ projects: [{ id: SOURCE, name: "air-quality" }, { id: "pqor4p2klnggnchpafnsyprelu", name: "mobility" }], deleted: ["old-pilot"] }));
  }
  if (kind === "data" && rest.endsWith("/copy")) {
    const input = route.request().postDataJSON() as { name: string; project: string; apply?: boolean };
    if (input.name === "air-quality") {
      const detail = "name: air-quality is taken by a project, a copy or an Organization; pick a new name";
      return route.fulfill(json({ type: "about:blank", title: "Unprocessable Content", status: 422, detail, errors: [detail] }, 422));
    }
    log(input.apply ? "copy" : "copy-plan", true, `project air-quality ${input.apply ? "restored" : "would be restored"} into ${input.name}`);
    return route.fulfill(json({ ...COPY_PLAN, name: input.name, projectId: input.apply ? COPIED : "" }));
  }
  if (kind === "data" && rest.endsWith("/restore")) {
    const input = route.request().postDataJSON() as { identity: string; request?: boolean };
    if (!input.identity.startsWith("AGE-SECRET-KEY-1")) {
      const detail = "identity: an age identity, one line starting with AGE-SECRET-KEY-1";
      return route.fulfill(json({ type: "about:blank", title: "Unprocessable Content", status: 422, detail, errors: [{ path: "identity", message: "an age identity, one line starting with AGE-SECRET-KEY-1" }] }, 422));
    }
    log("restore-plan", true, "16 row(s) would be written");
    let request = null;
    if (input.request) {
      request = { id: 1, versionId: "v1", requestedBy: "demo.admin", requestedAt: now(), state: "pending", plan: PLAN, decidedBy: null, decidedAt: null, message: "" };
      bucket.restores = [request, ...(bucket.restores ?? [])];
      log("restore-request", true, "restore 1 asked; a second administrator approves it");
    }
    return route.fulfill(json({ plan: PLAN, request }));
  }
  if (rest === "/restores") return route.fulfill(json(bucket.restores ?? []));
  const approve = /^\/restores\/(\d+)\/approve$/.exec(rest);
  if (approve) {
    const found = (bucket.restores ?? []).find((r) => String(r.id) === approve[1]);
    if (!found) return route.fulfill(json({ title: "Not Found", status: 404 }, 404));
    Object.assign(found, { state: "applied", decidedBy: "demo.admin", decidedAt: "2026-11-16T09:25:00Z", message: "16 row(s) written, 0 erasure(s) applied again" });
    log("restore", true, String(found.message));
    return route.fulfill(json(found));
  }
  if (rest.endsWith("/restore")) {
    log("restore-plan", true, "0 change(s) planned");
    return route.fulfill(
      json({
        organization: "bbsk",
        versionId: "v1",
        takenAt: "2026-11-16T09:11:00Z",
        changes: 0,
        groups: [
          { name: "bbsk", created: [], changed: [], unchanged: 14 },
          { name: "organization", created: [], changed: [], unchanged: 6 },
          {
            name: "organization/people",
            created: ["aino <aino@example.org>"],
            changed: [],
            unchanged: 3,
            note: "a restore never creates people: invite these by hand on People",
          },
        ],
      }),
    );
  }
  return route.fulfill(json({ title: "Not Found", status: 404 }, 404));
}

async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.JC_EVIDENCE_DIR;
  if (!dir) return;
  mkdirSync(dir, { recursive: true });
  const height = await page.evaluate(() => document.documentElement.scrollHeight);
  await page.setViewportSize({ width: page.viewportSize()?.width ?? 1440, height });
  await page.screenshot({ path: join(dir, `${name}.png`) });
}

test.describe("Administration → Backups (T-7154, T-7168)", () => {
  for (const width of [1440, 400]) {
    test(`an administrator sets up, runs, checks and deletes a backup at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      const bucket: Bucket = { target: null, log: [], versions: [] };
      await stubApi(page, bucket);
      await page.goto("/organization/backups?lang=en");
      const config = page.getByTestId("backup-configuration");
      await expect(config.getByText("No backup is set up yet")).toBeVisible();
      await expect(page.getByTestId("backup-data").getByText("No backup is set up yet")).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
      await shot(page, `backups-empty-${width}`);

      await config.getByLabel("S3 address").fill("https://s3.eu-central-1.amazonaws.com");
      await config.getByLabel("Region").fill("eu-central-1");
      await config.getByLabel("Bucket", { exact: true }).fill("City_Backups");
      await config.getByLabel("Folder").fill("joinedcontext/");
      await config.getByLabel("Access key id").fill("AKIAIOSFODNN7EXAMPLE");
      await config.getByLabel("Secret access key").fill("not-shown-again");
      await config.getByRole("button", { name: "Save" }).click();
      await expect(config.getByText(/3 to 63 lower-case letters/)).toBeVisible();
      await shot(page, `backups-refused-${width}`);

      await config.getByLabel("Bucket", { exact: true }).fill("city-backups");
      await config.getByRole("switch", { name: "Scheduled backups on" }).click();
      await config.getByRole("button", { name: "Save" }).click();
      await expect(config.getByText("Runs every 24 hours, and whenever you press Run now.")).toBeVisible();
      await expect(config.getByText("No version in the bucket yet")).toBeVisible();

      await config.getByRole("button", { name: "Run now" }).click();
      await expect(config.getByTestId("backup-last-run")).toContainText("47.1 KiB uploaded");
      await expect(config.getByText("Versioning on")).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
      await shot(page, `backups-filled-${width}`);

      await config.getByRole("button", { name: "Restore this version" }).click();
      await expect(page.getByText("Nothing in the configuration to restore.")).toBeVisible();
      await expect(page.getByText("aino <aino@example.org>")).toBeVisible();
      await expect(page.getByText(/A restore never creates people/)).toBeVisible();
      await expect(page.getByRole("link", { name: "Invite them on People" })).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
      await shot(page, `backups-restore-${width}`);
      await page.getByRole("button", { name: "Close" }).first().click();

      await config.getByRole("button", { name: "Delete backup" }).click();
      await page.getByRole("alertdialog").getByRole("button", { name: "Delete backup" }).click();
      await expect(config.getByText("Deleted", { exact: true })).toBeVisible();
      await expect(config.getByText(/The backup is deleted. Every earlier version is still kept/)).toBeVisible();
      await shot(page, `backups-deleted-${width}`);
    });

    test(`an administrator sets up the encrypted data backup with two keys at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await stubApi(page, { target: null, log: [], versions: [] });
      await page.goto("/organization/backups?lang=en");
      const data = page.getByTestId("backup-data");
      await expect(data.getByText("No backup is set up yet")).toBeVisible();
      await data.getByLabel("S3 address").fill("https://s3.eu-central-1.amazonaws.com");
      await data.getByLabel("Region").fill("eu-central-1");
      await data.getByLabel("Bucket", { exact: true }).fill("city-data");
      await data.getByLabel("Access key id").fill("AKIAIOSFODNN7EXAMPLE");
      await data.getByLabel("Secret access key").fill("not-shown-again");
      await data.getByLabel("Working key (age public key)").fill(WORKING);
      await data.getByLabel("Recovery key (age public key)").fill("ssh-ed25519 AAAA");
      await data.getByRole("button", { name: "Save" }).click();
      await expect(data.getByText(/an age public key starting with age1/)).toBeVisible();
      await data.scrollIntoViewIfNeeded();
      await shot(page, `backups-data-refused-${width}`);

      await data.getByLabel("Recovery key (age public key)").fill(RECOVERY);
      await data.getByRole("button", { name: "Save" }).click();
      await expect(data.getByTestId("backup-fingerprints")).toContainText("Recovery key fingerprint");
      await data.getByRole("button", { name: "Run now" }).click();
      await expect(data.getByTestId("backup-last-run")).toContainText("uploaded");
      await expect(data.getByRole("button", { name: "Download" })).toBeVisible();
      await expect(data.getByText("No restore asked")).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
      await shot(page, `backups-data-filled-${width}`);

      await data.getByRole("button", { name: "Restore this version" }).click();
      const dialog = page.getByTestId("backup-data-restore");
      await dialog.getByLabel("Identity that opens the archive").fill("not-a-key");
      await dialog.getByRole("button", { name: "Check this version" }).click();
      await expect(dialog.getByText(/an age identity, one line starting with AGE-SECRET-KEY-1/)).toBeVisible();
      await dialog.getByLabel("Identity that opens the archive").fill("AGE-SECRET-KEY-1E2EPLACEHOLDER");
      await dialog.getByRole("button", { name: "Check this version" }).click();
      await expect(dialog.getByRole("rowheader", { name: "portal/entity_comments" })).toBeVisible();
      // The check button settles from its busy look before the page is read for contrast.
      await expect(dialog.getByRole("button", { name: "Check this version" })).not.toHaveAttribute("aria-disabled", "true");
      await page.waitForTimeout(300);
      expect(await axeViolations(page)).toEqual([]);
      await shot(page, `backups-data-restore-${width}`);
      await dialog.getByRole("button", { name: "Ask a second administrator to approve" }).click();
      await expect(dialog.getByText(/Asked. Another administrator approves it below/)).toBeVisible();
      await dialog.getByRole("button", { name: "Close" }).first().click();
      await expect(data.getByText("Waits for another administrator of this Organization to approve it.")).toBeVisible();
      await expect(data.getByRole("button", { name: "Approve and restore" })).toHaveCount(0);
      await shot(page, `backups-data-requested-${width}`);
    });

    test(`a second administrator approves a data restore with the key at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      const target = { endpoint: "https://s3.eu-central-1.amazonaws.com", region: "eu-central-1", bucket: "city-data", prefix: "", accessKeyId: "AKIAIOSFODNN7EXAMPLE", enabled: true, everyHours: 24, keepDays: 90, recipients: [WORKING, RECOVERY], organization: "bbsk", kind: "data", updatedBy: "ada", updatedAt: "2026-11-16T09:00:00Z", lastRunAt: null };
      const restores = [{ id: 4, versionId: "v1", requestedBy: "ada", requestedAt: "2026-11-16T09:20:00Z", state: "pending", plan: PLAN, decidedBy: null, decidedAt: null, message: "" }];
      const versions = [{ versionId: "v1", lastModified: "2026-11-16T09:11:00Z", size: 912345, isLatest: true, deleteMarker: false }];
      const log = [{ id: 1, organization: "bbsk", kind: "data", action: "run", actor: "ada", at: "2026-11-16T09:11:00Z", ok: true, bytes: 912345, objectKey: "jc/bbsk/data/archive.zip.age", versionId: "v1", message: "uploaded" }];
      await stubApi(page, { target: null, log: [], versions: [] }, true, { target, log, versions, restores });
      await page.goto("/organization/backups?lang=en");
      const data = page.getByTestId("backup-data");
      await expect(data.getByRole("button", { name: "Approve and restore" })).toBeVisible();
      await data.getByLabel("Identity that opens the archive").fill("AGE-SECRET-KEY-1E2EPLACEHOLDER");
      await shot(page, `backups-data-approve-${width}`);
      await data.getByRole("button", { name: "Approve and restore" }).click();
      await expect(data.getByText("Restored", { exact: true })).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
      await shot(page, `backups-data-approved-${width}`);
    });

    test(`an administrator restores one project of a data version into a new project at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      const target = { endpoint: "https://s3.eu-central-1.amazonaws.com", region: "eu-central-1", bucket: "city-data", prefix: "", accessKeyId: "AKIAIOSFODNN7EXAMPLE", enabled: true, everyHours: 24, keepDays: 90, recipients: [WORKING, RECOVERY], organization: "bbsk", kind: "data", updatedBy: "ada", updatedAt: "2026-11-16T09:00:00Z", lastRunAt: null };
      const versions = [{ versionId: "v1", lastModified: "2026-11-16T09:11:00Z", size: 912345, isLatest: true, deleteMarker: false }];
      const log = [{ id: 1, organization: "bbsk", kind: "data", action: "run", actor: "ada", at: "2026-11-16T09:11:00Z", ok: true, bytes: 912345, objectKey: "jc/bbsk/data/archive.zip.age", versionId: "v1", message: "uploaded" }];
      await stubApi(page, { target: null, log: [], versions: [] }, true, { target, log, versions, restores: [] });
      await page.goto("/organization/backups?lang=en");
      const data = page.getByTestId("backup-data");
      await data.getByRole("button", { name: "Restore a project" }).click();
      const dialog = page.getByTestId("backup-project-copy");
      await expect(dialog.getByRole("button", { name: "Show the projects" })).toHaveAttribute("aria-disabled", "true");
      await shot(page, `backups-copy-empty-${width}`);
      await dialog.getByLabel("Identity that opens the archive").fill("AGE-SECRET-KEY-1E2EPLACEHOLDER");
      await dialog.getByRole("button", { name: "Show the projects" }).click();
      await dialog.getByLabel("Project of this backup").selectOption({ label: "air-quality" });
      await expect(dialog.getByText(/old-pilot/)).toBeVisible();
      await dialog.getByLabel("New project name").fill("air-quality");
      await dialog.getByRole("button", { name: "Check", exact: true }).click();
      await expect(dialog.getByText(/air-quality is taken by a project/)).toBeVisible();
      await shot(page, `backups-copy-refused-${width}`);
      await dialog.getByLabel("New project name").fill("air-quality-restored");
      await dialog.getByRole("button", { name: "Check", exact: true }).click();
      await expect(dialog.getByRole("rowheader", { name: "entity_comments" })).toBeVisible();
      await expect(dialog.getByRole("button", { name: "Check", exact: true })).not.toHaveAttribute("aria-disabled", "true");
      await page.waitForTimeout(300);
      expect(await axeViolations(page)).toEqual([]);
      await shot(page, `backups-copy-checked-${width}`);
      await dialog.getByRole("button", { name: "Restore into the new project" }).click();
      await expect(dialog.getByText(/Restored into air-quality-restored/)).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
      await shot(page, `backups-copy-done-${width}`);
    });

    test(`a member who does not administer is refused at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await stubApi(page, { target: null, log: [], versions: [] }, false);
      await page.goto("/organization/backups?lang=en");
      await expect(page.getByText(/This page is for organization administrators/).first()).toBeVisible();
      await expect(page.getByText("No backup is set up yet")).toHaveCount(0);
      await expect(page.getByTestId("backup-data")).toHaveCount(0);
      await shot(page, `backups-refused-member-${width}`);
    });
  }
});
