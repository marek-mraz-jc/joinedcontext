/**
 * T-7588: Administration → Backups, the actions on a saved backup: Run now and its refusal, the
 * run log, deleting the current version, downloading a version (a plain zip, an age archive, an
 * answer with no bytes), a configuration restore proposed as a Change, the identity shown on
 * request, a restore declined or withdrawn, and every dialog closed with Escape.
 */
import { cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { Backups } from "../src/pages/organization/Backups";
import { json, problem, renderPage } from "./page_contract";

const ADMIN = {
  project: "org",
  bootstrap: false,
  grants: [{ role: "org-admin", binding: "admins", rule: { kinds: ["RoleBinding"], verbs: ["propose", "approve", "delete"] } }],
};

const TARGET = {
  organization: "bbsk",
  kind: "configuration",
  endpoint: "https://s3.eu-central-1.amazonaws.com",
  region: "eu-central-1",
  bucket: "city-backups",
  prefix: "jc/",
  accessKeyId: "AKIAEXAMPLE",
  enabled: false,
  everyHours: 24,
  keepDays: 90,
  recipients: [],
  updatedBy: "ada",
  updatedAt: "2026-11-16T09:00:00Z",
  lastRunAt: null,
};
const DATA_TARGET = { ...TARGET, kind: "data", enabled: true };
const PLAN = {
  organization: "bbsk",
  versionId: "d1",
  takenAt: "2026-11-16T09:00:00Z",
  modules: { portal: { entity_comments: { create: 2, update: 0, unchanged: 0, refused: 0 } } },
  notRestored: {},
  notInArchive: {},
  erasuresReplayed: 0,
};
const restoreBy = (id: number, by: string) => ({
  id,
  versionId: "d1",
  requestedBy: by,
  requestedAt: "2026-11-16T09:30:00Z",
  state: "pending",
  plan: PLAN,
  decidedBy: null,
  decidedAt: null,
  message: "",
});
const RUN_OK = { id: 1, action: "run", ok: true, at: "2026-11-16T10:00:00Z", bytes: 3 * 1024 * 1024, actor: "jana.kovacova", message: "" };
const PLAN_FAILED = { id: 2, action: "restore-plan", ok: false, at: "2026-11-16T09:00:00Z", bytes: 0, actor: "ada", message: "the identity does not open this version" };

interface Options {
  /** The configuration backup's log after each Run now. */
  logAfterRun?: unknown[];
  log?: unknown[];
  run?: () => Response;
  versions?: unknown[];
  download?: unknown;
  restores?: unknown[];
  decline?: () => Response;
}

function render(options: Options = {}) {
  const requests: { method: string; path: string; body?: string }[] = [];
  let runs = 0;
  renderPage(<Backups />, {
    path: "/organization/backups",
    answer: async (url, request) => {
      const { method } = request;
      const path = url.pathname;
      requests.push({ method, path, body: method === "GET" ? undefined : await request.clone().text() });
      if (path.endsWith("/permissions/me")) return json(ADMIN);
      if (path === "/api/v1/organization/backups/configuration/run") {
        runs += 1;
        return options.run ? options.run() : json({});
      }
      if (path === "/api/v1/organization/backups/configuration")
        return json({ target: TARGET, fingerprints: [], log: runs > 0 ? (options.logAfterRun ?? []) : (options.log ?? []) });
      if (path === "/api/v1/organization/backups/configuration/bundle") return new Response(null, { status: 204 });
      if (path === "/api/v1/organization/backups/configuration/versions")
        return json({
          versioning: "Suspended",
          objectLock: true,
          versions: options.versions ?? [{ versionId: "v2", lastModified: "2026-11-16T10:00:00Z", size: 512, isLatest: true, deleteMarker: false }],
        });
      if (path === "/api/v1/organization/backups/configuration/versions/v2/restore") {
        const asked = JSON.parse(await request.clone().text()) as { apply: boolean };
        return json({
          organization: "bbsk",
          versionId: "v2",
          takenAt: "2026-11-16T10:00:00Z",
          changes: 2,
          groups: [
            { name: "air-quality", created: ["Endpoint/air"], changed: ["Policy/open"], unchanged: 1, note: "", change: asked.apply ? "chg-00000041" : null },
            { name: "organization", created: [], changed: ["organization/settings.yaml"], unchanged: 2, note: "not changed from here" },
          ],
        });
      }
      if (path.startsWith("/api/v1/organization/backups/configuration/versions/")) return json(options.download ?? "");
      if (path === "/api/v1/organization/backups/data") return json({ target: DATA_TARGET, fingerprints: [], log: [] });
      if (path === "/api/v1/organization/backups/data/versions")
        return json({ versioning: "Enabled", objectLock: false, versions: [{ versionId: "d1", lastModified: "2026-11-16T09:00:00Z", size: 4096, isLatest: true, deleteMarker: false }] });
      if (path === "/api/v1/organization/backups/data/versions/d1") return json(options.download ?? "");
      if (path === "/api/v1/organization/backups/data/restores") return json(options.restores ?? []);
      if (path.endsWith("/decline")) return options.decline ? options.decline() : json({});
      return undefined;
    },
  });
  return { requests };
}

const configuration = async () => within(await screen.findByTestId("backup-configuration"));
const data = async () => within(await screen.findByTestId("backup-data"));

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("Administration → Backups, a saved backup's actions", () => {
  it("runs a backup on request and shows the run, its size and the log with a failed step's reason", async () => {
    const { requests } = render({ logAfterRun: [RUN_OK, PLAN_FAILED] });
    const section = await configuration();
    expect(await section.findByText(en.backups.runs.off)).toBeInTheDocument();
    expect(section.getByText(en.backups.runs.never)).toBeInTheDocument();
    fireEvent.click(section.getByRole("button", { name: en.backups.runs.now }));
    expect(await section.findByText(en.backups.runs.done)).toBeInTheDocument();
    expect(section.getByTestId("backup-last-run")).toHaveTextContent(/jana\.kovacova: 3 MiB uploaded/);
    expect(section.getByText("Who did what (2)")).toBeInTheDocument();
    expect(section.getByText(en.backups.log.action.plan)).toBeInTheDocument();
    expect(section.getByText(PLAN_FAILED.message)).toBeInTheDocument();
    expect(requests.filter((r) => r.path.endsWith("/configuration/run") && r.method === "POST")).toHaveLength(1);
    // The data backup is on its schedule.
    expect(await (await data()).findByText("Runs every 24 hours, and whenever you press Run now.")).toBeInTheDocument();
  });

  it("says why a run was refused and names a failed last run", async () => {
    render({ log: [{ ...RUN_OK, ok: false, message: "the bucket refused the upload" }], run: () => problem(503, "the bucket is unreachable") });
    const section = await configuration();
    expect(await section.findByTestId("backup-last-run")).toHaveTextContent(/failed: the bucket refused the upload/);
    fireEvent.click(section.getByRole("button", { name: en.backups.runs.now }));
    expect(await section.findByText(/the bucket is unreachable/)).toBeInTheDocument();
    expect(section.queryByText(en.backups.runs.done)).toBeNull();
  });

  it("deletes the current version after the confirmation and reads the versions again", async () => {
    const { requests } = render();
    const section = await configuration();
    expect(await section.findByText("512 B")).toBeInTheDocument();
    expect(section.getByText(en.backups.versions.versioningOff)).toBeInTheDocument();
    expect(section.getByText(en.backups.versions.lockOn)).toBeInTheDocument();
    fireEvent.click(section.getByRole("button", { name: en.backups.versions.delete }));
    const dialog = await screen.findByRole("alertdialog");
    fireEvent.click(within(dialog).getByRole("button", { name: en.backups.versions.delete }));
    await waitFor(() => expect(requests.some((r) => r.method === "DELETE" && r.path.endsWith("/configuration/bundle"))).toBe(true));
    await waitFor(() => expect(requests.filter((r) => r.path.endsWith("/configuration/versions")).length).toBeGreaterThan(1));
  });

  it("downloads a version from before encryption as a zip, and a data version as an age archive", async () => {
    const created: Blob[] = [];
    const saved: string[] = [];
    vi.stubGlobal("URL", Object.assign(URL, { createObjectURL: (blob: Blob) => (created.push(blob), "blob:backup"), revokeObjectURL: () => undefined }));
    vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(function (this: HTMLAnchorElement) {
      saved.push(this.download);
    });
    // "PK\x03\x04\n": a zip's first bytes.
    render({ download: "UEsDBAo=" });
    fireEvent.click(await (await configuration()).findByRole("button", { name: en.backups.versions.download }));
    await waitFor(() => expect(saved).toEqual(["configuration-v2.zip"]));
    expect(created[0].type).toBe("application/zip");
    cleanup();
    saved.length = 0;
    // "age": an encrypted archive.
    render({ download: "YWdl" });
    fireEvent.click(await (await data()).findByRole("button", { name: en.backups.versions.download }));
    await waitFor(() => expect(saved).toEqual(["data-d1.zip.age"]));
  });

  it("says the backup could not be read when a download answers no bytes", async () => {
    const click = vi.spyOn(HTMLAnchorElement.prototype, "click");
    render({ download: "" });
    const section = await configuration();
    fireEvent.click(await section.findByRole("button", { name: en.backups.versions.download }));
    expect(await section.findByText(en.backups.unreadable)).toBeInTheDocument();
    expect(click).not.toHaveBeenCalled();
  });

  it("proposes a configuration restore of the projects it would change, and shows the identity on request", async () => {
    const { requests } = render();
    fireEvent.click(await (await configuration()).findByRole("button", { name: en.backups.versions.restore }));
    const dialog = within(await screen.findByTestId("backup-restore-plan"));
    const identity = dialog.getByLabelText(en.backups.data.identity);
    expect(identity).toHaveAttribute("type", "password");
    fireEvent.click(dialog.getByRole("button", { name: en.backups.data.identityShow }));
    expect(identity).toHaveAttribute("type", "text");
    fireEvent.click(dialog.getByRole("button", { name: en.backups.data.identityHide }));
    expect(identity).toHaveAttribute("type", "password");
    fireEvent.change(identity, { target: { value: "AGE-SECRET-KEY-1TEST" } });
    fireEvent.click(dialog.getByRole("button", { name: en.backups.data.check }));
    expect(await dialog.findByText("Endpoint/air")).toBeInTheDocument();
    expect(dialog.getByText(en.backups.restore.organizationNote)).toBeInTheDocument();
    expect(dialog.getByText("1 to create, 1 to change, 1 as they are")).toBeInTheDocument();
    fireEvent.click(dialog.getByRole("button", { name: en.backups.restore.propose }));
    expect(await dialog.findByText(en.backups.restore.proposed)).toBeInTheDocument();
    expect(dialog.queryByRole("button", { name: en.backups.restore.propose })).toBeNull();
    const asks = requests.filter((r) => r.path.endsWith("/v2/restore")).map((r) => (JSON.parse(r.body ?? "{}") as { apply: boolean }).apply);
    expect(asks).toEqual([false, true]);
  });

  it("closes each dialog with Escape", async () => {
    render();
    fireEvent.click(await (await configuration()).findByRole("button", { name: en.backups.versions.restore }));
    fireEvent.keyDown(await screen.findByTestId("backup-restore-plan"), { key: "Escape" });
    await waitFor(() => expect(screen.queryByTestId("backup-restore-plan")).toBeNull());
    const section = await data();
    fireEvent.click(await section.findByRole("button", { name: en.backups.versions.restore }));
    fireEvent.keyDown(await screen.findByTestId("backup-data-restore"), { key: "Escape" });
    await waitFor(() => expect(screen.queryByTestId("backup-data-restore")).toBeNull());
    fireEvent.click(section.getByRole("button", { name: en.backups.copy.action }));
    fireEvent.keyDown(await screen.findByTestId("backup-project-copy"), { key: "Escape" });
    await waitFor(() => expect(screen.queryByTestId("backup-project-copy")).toBeNull());
  });

  it("declines another administrator's restore and withdraws one's own, and says why a decline failed", async () => {
    let declines = 0;
    const { requests } = render({
      restores: [restoreBy(7, "ada"), restoreBy(8, "jana.kovacova")],
      decline: () => (++declines === 1 ? json({}) : problem(409, "the restore was decided already")),
    });
    const list = within(await (await data()).findByRole("list", { name: en.backups.data.requestsTitle }));
    const [other, own] = list.getAllByRole("listitem");
    fireEvent.click(within(other).getByRole("button", { name: en.backups.data.decline }));
    await waitFor(() => expect(requests.some((r) => r.path.endsWith("/restores/7/decline"))).toBe(true));
    fireEvent.click(within(own).getByRole("button", { name: en.backups.data.withdraw }));
    expect(await within(own).findByText(/the restore was decided already/)).toBeInTheDocument();
    expect(requests.some((r) => r.path.endsWith("/restores/8/decline"))).toBe(true);
  });

  it("turns the schedule on in the form", async () => {
    render();
    const section = await configuration();
    const toggle = await section.findByRole("switch", { name: en.backups.form.enabled });
    expect(toggle).toHaveAttribute("aria-checked", "false");
    fireEvent.click(toggle);
    expect(toggle).toHaveAttribute("aria-checked", "true");
  });
});
