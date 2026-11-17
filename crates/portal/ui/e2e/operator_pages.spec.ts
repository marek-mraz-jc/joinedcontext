import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { axeViolations } from "./axe";
import { doorsAsRest, manifestReadsAsRest } from "./manifestReads";

test.beforeEach(({ page }) => {
  manifestReadsAsRest(page);
  doorsAsRest(page);
});

// Administration → the installation operators' sections (T-7140): a domain proved by its TXT
// record, its first host asked for, the Change a second operator approves, the host on its way to
// served with the day's certificate slots, the audit, and an Organization administrator who is no
// operator refused, at 1440 and 400 px. `vite preview` has no Portal behind it, so a small
// in-browser registry answers the operator doors with the server's shapes; the page is the real build.

const ME = "s1";
const OTHER = "a1b2c3";
const IDENTITY = { subject: ME, username: "jana.operator", name: "Jana Operator", email: "jana@joinedcontext.com", roles: [] };

interface Registry {
  organizations: Record<string, unknown>[];
  ids: Record<string, unknown>[];
  hosts: Record<string, unknown>[];
  changes: Record<string, unknown>[];
  audit: Record<string, unknown>[];
}

function fresh(): Registry {
  return {
    organizations: [
      { name: "bbsk", title: "Banskobystrický kraj", domain: "bbsk.sk", hosts: { served: 1, "dns-ok": 1, approved: 1 } },
      { name: "zilina", title: "Žilinský kraj", domain: null, hosts: {} },
    ],
    ids: [
      { id: "sk_bbsk", domain: "bbsk.sk", recordName: "_joinedcontext.bbsk.sk", recordValue: "jc-verify=4f1c9a", requestedAt: "2026-11-16T09:00:00Z", mintedAt: "2026-11-16T09:10:00Z", organizations: ["bbsk"] },
    ],
    hosts: [
      { host: "smartcity.bbsk.sk", organization: "bbsk", organizationId: "sk_bbsk", role: "working", state: "served", detail: null, detailAt: null, updatedAt: "2026-11-16T09:30:00Z" },
      { host: "opendata.bbsk.sk", organization: "bbsk", organizationId: "sk_bbsk", role: "opendata", state: "dns-ok", detail: "waiting for a certificate slot until 18 Nov 2026 09:00 UTC", detailAt: "2026-11-17T09:00:00Z", updatedAt: "2026-11-17T09:00:00Z" },
      { host: "api.bbsk.sk", organization: "bbsk", organizationId: "sk_bbsk", role: "api", state: "approved", detail: "its DNS answers 203.0.113.9, not the edge's address", detailAt: "2026-11-17T09:05:00Z", updatedAt: "2026-11-17T09:05:00Z" },
    ],
    changes: [
      { id: "1c0e3a5f-2b3c-4d4e-9fa0-112233445566", kind: "HostAdd", payload: { host: "signin.bbsk.sk", organization: "bbsk", organizationId: "sk_bbsk", role: "signin" }, state: "proposed", proposedBy: OTHER, proposedAt: "2026-11-17T08:30:00Z", decidedBy: null, decidedAt: null },
    ],
    audit: [
      { id: 3, at: "2026-11-17T09:00:00Z", actor: "reconciler", changeId: null, action: "certificate.created", before: null, after: { host: "opendata.bbsk.sk" }, sourceIp: null },
      { id: 2, at: "2026-11-16T09:30:00Z", actor: "reconciler", changeId: null, action: "host.served", before: { host: "smartcity.bbsk.sk", state: "cert-issued" }, after: { host: "smartcity.bbsk.sk", state: "served" }, sourceIp: null },
      { id: 1, at: "2026-11-16T09:00:00Z", actor: OTHER, changeId: null, action: "proposed HostAdd", before: null, after: { kind: "HostAdd", payload: { host: "smartcity.bbsk.sk" } }, sourceIp: "198.51.100.4" },
    ],
  };
}

function json(body: unknown, status = 200) {
  return { status, contentType: "application/json", body: JSON.stringify(body) };
}

const FORBIDDEN = { type: "about:blank", title: "Forbidden", status: 403, detail: "operator Changes are for a person of the installation's operators' group" };

async function stubApi(page: Page, registry: Registry, operator = true): Promise<void> {
  let audit = registry.audit.length;
  const log = (action: string, after: unknown) =>
    registry.audit.unshift({ id: ++audit, at: `2026-11-17T10:00:${String(audit).padStart(2, "0")}Z`, actor: ME, changeId: null, action, before: null, after, sourceIp: "198.51.100.7" });
  await page.route("**/api/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const method = request.method();
    if (path.endsWith("/auth/me")) return route.fulfill(json(IDENTITY));
    if (path === "/api/v1/projects") return route.fulfill(json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ProjectList", items: [{ name: "bbsk" }] }));
    if (path.endsWith("/permissions/me"))
      return route.fulfill(
        json(
          operator
            ? { project: "org", bootstrap: true, grants: [] }
            : { project: "org", bootstrap: false, grants: [{ role: "org-admin", binding: "admins", rule: { kinds: ["Project", "RoleBinding"], verbs: ["read", "propose", "approve"] } }] },
        ),
      );
    if (path.startsWith("/api/v1/operator/") && !operator) return route.fulfill(json(FORBIDDEN, 403));
    if (path === "/api/v1/operator/organizations") return route.fulfill(json({ items: registry.organizations, ids: registry.ids }));
    if (path === "/api/v1/operator/hosts") {
      const day = registry.audit.filter((row) => row.action === "certificate.created").length + (registry.hosts.length > 0 ? 9 : 0);
      return route.fulfill(json({ items: registry.hosts, certificates: { usedToday: day, perDay: 10, perDomainWeek: 20, nextFreeAt: day >= 10 ? "2026-11-18T09:00:00Z" : null } }));
    }
    if (path === "/api/v1/operator/audit") return route.fulfill(json({ items: registry.audit }));
    if (path === "/api/v1/operator/changes" && method === "GET") return route.fulfill(json({ items: registry.changes }));
    if (path === "/api/v1/operator/changes" && method === "POST") {
      const input = request.postDataJSON() as Record<string, string>;
      if (input.kind === "HostAdd" && !input.host.endsWith(".zilina.sk") && !input.host.endsWith(".bbsk.sk")) {
        const detail = `host: '${input.host}' is not under the domain of Organization id ${input.organizationId ?? "sk_bbsk"}`;
        return route.fulfill(json({ type: "about:blank", title: "Unprocessable Content", status: 422, detail, errors: [detail] }, 422));
      }
      const { kind, ...payload } = input;
      const change = { id: `0b9d2f4e-1a2b-4c3d-8e9f-00112233445${registry.changes.length}`, kind, payload, state: "proposed", proposedBy: ME, proposedAt: "2026-11-17T10:00:00Z", decidedBy: null, decidedAt: null };
      registry.changes.unshift(change);
      log(`proposed ${kind}`, { kind, payload });
      return route.fulfill(json(change, 201));
    }
    const decide = /^\/api\/v1\/operator\/changes\/([^/]+)\/(approve|reject)$/.exec(path);
    if (decide) {
      const change = registry.changes.find((c) => c.id === decide[1]);
      if (!change) return route.fulfill(json({ title: "Not Found", status: 404 }, 404));
      if (change.proposedBy === ME && decide[2] === "approve")
        return route.fulfill(json({ ...FORBIDDEN, detail: "a second operator approves a Change: you asked for this one" }, 403));
      Object.assign(change, { state: decide[2] === "approve" ? "applied" : "rejected", decidedBy: ME, decidedAt: "2026-11-17T10:05:00Z" });
      if (decide[2] === "approve") {
        const p = change.payload as Record<string, string>;
        registry.hosts.push({ host: p.host, organization: p.organization, organizationId: p.organizationId, role: p.role, state: "approved", detail: null, detailAt: null, updatedAt: "2026-11-17T10:05:00Z" });
      }
      log(decide[2] === "approve" ? "applied HostAdd" : "rejected HostAdd", change.payload);
      return route.fulfill(json(change));
    }
    if (path === "/api/v1/organization-ids/challenge") {
      const { domain } = request.postDataJSON() as { domain: string };
      const id = `sk_${domain.split(".")[0]}`;
      registry.ids.push({ id, domain, recordName: `_joinedcontext.${domain}`, recordValue: "jc-verify=8d2e71", requestedAt: "2026-11-17T10:00:00Z", mintedAt: null, organizations: [] });
      return route.fulfill(json({ domain, organizationId: id, name: `_joinedcontext.${domain}`, value: "jc-verify=8d2e71", record: `_joinedcontext.${domain} TXT "jc-verify=8d2e71"` }));
    }
    if (path === "/api/v1/organization-ids" && method === "POST") {
      const { domain } = request.postDataJSON() as { domain: string };
      const id = registry.ids.find((row) => row.domain === domain);
      if (!id) return route.fulfill(json({ title: "Not Found", status: 404, detail: `no challenge for ${domain}: ask for its TXT record first` }, 404));
      id.mintedAt = "2026-11-17T10:02:00Z";
      return route.fulfill(json({ organizationId: id.id, domain, mintedAt: id.mintedAt }, 201));
    }
    return route.fulfill(json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] }));
  });
}

async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.JC_EVIDENCE_DIR;
  if (!dir) return;
  mkdirSync(dir, { recursive: true });
  const height = await page.evaluate(() => document.documentElement.scrollHeight);
  await page.setViewportSize({ width: page.viewportSize()?.width ?? 1440, height });
  await page.screenshot({ path: join(dir, `${name}.png`) });
  await page.setViewportSize({ width: page.viewportSize()?.width ?? 1440, height: 900 });
}

test.describe("Administration → the installation operators (T-7140)", () => {
  for (const width of [1440, 400]) {
    test(`an operator proves a domain, asks for its host, decides a Change and follows the host at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      const registry = fresh();
      await stubApi(page, registry);

      await page.goto("/organization/operator-organizations?lang=en");
      await expect(page.getByRole("heading", { name: "Organizations", level: 2 })).toBeVisible();
      await expect(page.getByText("Banskobystrický kraj").filter({ visible: true }).first()).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
      await shot(page, `operator-organizations-${width}`);

      await page.getByLabel("Domain", { exact: true }).fill("zilina.sk");
      await page.getByRole("button", { name: "Get the TXT record" }).click();
      await expect(page.getByText('_joinedcontext.zilina.sk TXT "jc-verify=8d2e71"').filter({ visible: true }).first()).toBeVisible();
      await page.getByRole("button", { name: "Check and mint the id" }).click();
      await expect(page.getByText("The domain is proved. Its Organization id is sk_zilina.")).toBeVisible();

      await page.getByLabel("Host", { exact: true }).fill("smartcity.example.org");
      await page.getByLabel("Organization", { exact: true }).selectOption("zilina");
      await page.getByRole("button", { name: "Ask for the host" }).click();
      await expect(page.getByText(/is not under the domain/)).toBeVisible();
      await shot(page, `operator-add-refused-${width}`);

      await page.getByLabel("Host", { exact: true }).fill("smartcity.zilina.sk");
      await page.getByRole("button", { name: "Ask for the host" }).click();
      await expect(page.getByText("The host is asked for. A second operator approves it.")).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
      await shot(page, `operator-add-proposed-${width}`);

      await page.goto("/organization/operator-changes?lang=en");
      await expect(page.getByText("Another operator approves this.").filter({ visible: true }).first()).toBeVisible();
      await page.getByRole("button", { name: "Approve" }).filter({ visible: true }).first().click();
      await expect(page.getByText("The Change for signin.bbsk.sk is applied.")).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
      await shot(page, `operator-changes-${width}`);

      await page.goto("/organization/operator-hosts?lang=en");
      await expect(page.getByText("signin.bbsk.sk").filter({ visible: true }).first()).toBeVisible();
      await expect(page.getByText(/new certificates requested in the last 24 hours/)).toBeVisible();
      await expect(page.getByText("waiting for a certificate slot until 18 Nov 2026 09:00 UTC").filter({ visible: true }).first()).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
      await shot(page, `operator-hosts-${width}`);
      await page.getByRole("button", { name: "Remove" }).filter({ visible: true }).first().click();
      await expect(page.getByRole("alertdialog")).toBeVisible();
      await shot(page, `operator-hosts-remove-${width}`);
      await page.getByRole("button", { name: "Ask for the removal" }).click();
      await expect(page.getByText("The removal is asked for. A second operator approves it.")).toBeVisible();

      await page.goto("/organization/operator-audit?lang=en");
      await expect(page.getByText("proposed HostRemove").filter({ visible: true }).first()).toBeVisible();
      await expect(page.getByText("The Portal (reconciler)").filter({ visible: true }).first()).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
      await shot(page, `operator-audit-${width}`);
    });

    test(`an empty registry says where to start at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await stubApi(page, { organizations: [], ids: [], hosts: [], changes: [], audit: [] });
      await page.goto("/organization/operator-hosts?lang=en");
      await expect(page.getByText("No host yet. Add a domain and ask for its first host under Organizations.")).toBeVisible();
      await shot(page, `operator-hosts-empty-${width}`);
      await page.goto("/organization/operator-organizations?lang=en");
      await expect(page.getByText("No Organization exists yet", { exact: false })).toBeVisible();
      await shot(page, `operator-organizations-empty-${width}`);
    });

    test(`an Organization administrator who is no operator meets no operator section at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await stubApi(page, fresh(), false);
      await page.goto("/organization/operator-hosts?lang=en");
      await expect(page.getByText("These sections belong to the installation's administrators, its operators.", { exact: false })).toBeVisible();
      await expect(page.getByRole("link", { name: "Hosts and certificates" })).toHaveCount(0);
      expect(await axeViolations(page)).toEqual([]);
      await shot(page, `operator-refused-${width}`);
    });
  }
});
