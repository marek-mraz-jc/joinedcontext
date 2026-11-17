import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { axeViolations } from "./axe";
import { doorsAsRest, manifestReadsAsRest } from "./manifestReads";
import { odataBody } from "../tests/odata";

test.beforeEach(({ page }) => {
  manifestReadsAsRest(page);
  doorsAsRest(page);
});

// Administration → Setup and → Installation (T-7152): an operator follows "Set it" from the setup
// checklist to the Installation page, asks for the e-mail server (a refused host first), a second
// operator approves it, and the test e-mail goes out; an Organization administrator reads that
// their operator sets it and meets no Installation page, at 1440 and 400 px. `vite preview` has no
// Portal behind it, so the operator doors answer with the server's shapes; the page is the real build.

const ME = "s1";
const IDENTITY = { subject: ME, username: "jana.operator", name: "Jana Operator", email: "jana@joinedcontext.com", roles: [] };
const STEPS = ["organization", "domain", "people", "project", "publishers", "policies"];
const OPERATOR_ITEMS = ["branding", "loginTheme", "smtp", "backups"];
const MEMBER = { subject: "s2", username: "eva.admin", name: "Eva Admin", email: "eva@bbsk.sk", roles: [] };
const FORBIDDEN = { type: "about:blank", title: "Forbidden", status: 403, detail: "operator Changes are for a person of the installation's operators' group" };

interface World {
  smtp: Record<string, unknown> | null;
  smtpChange: string | null;
  tested: number;
  branding?: Record<string, unknown> | null;
  brandingChange?: string | null;
}

function json(body: unknown, status = 200) {
  return { status, contentType: "application/json", body: JSON.stringify(body) };
}

async function stubApi(page: Page, world: World, operator = true): Promise<void> {
  await page.route("**/api/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const method = request.method();
    if (path.endsWith("/auth/me")) return route.fulfill(json(operator ? IDENTITY : MEMBER));
    if (path === "/api/v1/projects") return route.fulfill(json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ProjectList", items: [{ name: "bbsk" }] }));
    if (path.endsWith("/permissions/me"))
      return route.fulfill(
        json(
          operator
            ? { project: "org", bootstrap: true, grants: [] }
            : { project: "org", bootstrap: false, grants: [{ role: "org-admin", binding: "admins", scope: "organization", rule: { kinds: ["Organization", "Project", "RoleBinding"], verbs: ["read", "propose", "approve", "delete"] } }] },
        ),
      );
    // The setup checklist is the Organization reading `setup` (jc_organization_read, T-7098).
    if (decodeURIComponent(path).startsWith("/api/v1/Organization/JoinedContext.Portal.jc_organization_read(")) {
      const steps = STEPS.map((id, i) => ({ id, done: i < 3 }));
      const items = OPERATOR_ITEMS.map((id) => ({ id, done: id === "smtp" ? world.smtp !== null : id === "branding" }));
      return route.fulfill({ status: 200, contentType: "application/json", body: odataBody({ complete: false, steps, operator: items, operates: operator }) });
    }
    if (path.startsWith("/api/v1/operator/") && !operator) return route.fulfill(json(FORBIDDEN, 403));
    if (path === "/api/v1/operator/installation")
      return route.fulfill(json({ smtp: world.smtp, smtpChange: world.smtpChange, branding: world.branding ?? undefined, brandingChange: world.brandingChange ?? null }));
    if (path === "/api/v1/operator/changes" && method === "POST" && (request.postDataJSON() as { kind: string }).kind === "BrandingSet") {
      const input = request.postDataJSON() as { branding: Record<string, unknown> };
      const primary = (input.branding.colours as Record<string, string> | undefined)?.primary;
      if (primary !== undefined && !/^#[0-9a-f]{6}$/i.test(primary)) {
        const detail = "branding.colours.primary: branding.colours.primary must be a hex colour such as #0b5394";
        return route.fulfill(json({ type: "about:blank", title: "Unprocessable Content", status: 422, detail: `the Change cannot be taken: ${detail}`, errors: [detail] }, 422));
      }
      world.brandingChange = "9a8b7c6d-5e4f-4a3b-8c2d-1e0f9a8b7c6d";
      return route.fulfill(json({ id: world.brandingChange, kind: "BrandingSet", payload: { branding: input.branding }, state: "proposed", proposedBy: ME, proposedAt: "2026-11-17T10:00:00Z", decidedBy: null, decidedAt: null }, 201));
    }
    if (path === "/api/v1/operator/changes" && method === "POST") {
      const input = request.postDataJSON() as { kind: string; smtp: Record<string, unknown> };
      if (String(input.smtp.host).match(/^[0-9.]+$/)) {
        const detail = `smtp.host: '${String(input.smtp.host)}' is no public host name of an SMTP server; give one, e.g. smtp.example.org`;
        return route.fulfill(json({ type: "about:blank", title: "Unprocessable Content", status: 422, detail: `the Change cannot be taken: ${detail}`, errors: [detail] }, 422));
      }
      world.smtpChange = "7d1e2f30-4a5b-4c6d-8e9f-a0b1c2d3e4f5";
      const { password, ...settings } = input.smtp;
      const change = { id: world.smtpChange, kind: "SmtpSet", payload: { smtp: { ...settings, newPassword: password !== undefined } }, state: "proposed", proposedBy: ME, proposedAt: "2026-11-17T10:00:00Z", decidedBy: null, decidedAt: null };
      return route.fulfill(json(change, 201));
    }
    if (path === "/api/v1/operator/installation/test-email") {
      world.tested += 1;
      return route.fulfill(json({ sent: true }));
    }
    return route.fulfill(json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] }));
  });
}

async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.JC_EVIDENCE_DIR;
  if (!dir) return;
  mkdirSync(dir, { recursive: true });
  await page.mouse.move(0, 0);
  const height = await page.evaluate(() => document.documentElement.scrollHeight);
  await page.setViewportSize({ width: page.viewportSize()?.width ?? 1440, height });
  await page.screenshot({ path: join(dir, `${name}.png`) });
  await page.setViewportSize({ width: page.viewportSize()?.width ?? 1440, height: 900 });
}

test.describe("Administration → the installation's e-mail server (T-7152)", () => {
  for (const width of [1440, 400]) {
    test(`an operator sets the e-mail server from the setup checklist and sends a test at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      const world: World = { smtp: null, smtpChange: null, tested: 0 };
      await stubApi(page, world);

      await page.goto("/organization/setup?lang=en");
      await expect(page.getByText("You are one of the installation's operators", { exact: false })).toBeVisible();
      await expect(page.getByText(/keycloak/i)).toHaveCount(0);
      expect(await axeViolations(page)).toEqual([]);
      await shot(page, `installation-setup-operator-${width}`);
      await page.getByRole("listitem").filter({ hasText: "An SMTP server" }).getByRole("link", { name: "Set it" }).click();

      await expect(page.getByRole("heading", { name: "Installation", level: 2 })).toBeVisible();
      await expect(page.getByText("No e-mail server yet", { exact: false })).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
      await shot(page, `installation-empty-${width}`);

      await page.getByLabel("SMTP host", { exact: false }).fill("10.0.0.1");
      await page.getByLabel("From address", { exact: false }).fill("noreply@bbsk.sk");
      await page.getByLabel("User", { exact: true }).fill("mailer@bbsk.sk");
      await page.getByLabel("Password", { exact: true }).fill("app-password");
      await page.getByRole("button", { name: "Ask for this change" }).click();
      await expect(page.getByText(/is no public host name of an SMTP server/)).toBeVisible();
      await shot(page, `installation-error-${width}`);

      await page.getByLabel("SMTP host", { exact: false }).fill("smtp.bbsk.sk");
      await page.getByRole("button", { name: "Ask for this change" }).click();
      await expect(page.getByText("Waiting for approval", { exact: true })).toBeVisible();
      await expect(page.getByLabel("SMTP host", { exact: false })).toBeDisabled();
      await expect(page.getByText("A change of the e-mail server waits for a second operator.", { exact: false })).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
      await shot(page, `installation-waiting-${width}`);

      // A second operator approved it: the sign-in service holds it, the password only as "stored".
      world.smtp = { host: "smtp.bbsk.sk", port: 587, tls: "starttls", from: "noreply@bbsk.sk", user: "mailer@bbsk.sk", passwordSet: true };
      world.smtpChange = null;
      await page.reload();
      await expect(page.getByText("Set", { exact: true })).toBeVisible();
      await expect(page.getByLabel("Password", { exact: true })).toHaveValue("");
      await expect(page.getByLabel("Password", { exact: true })).toHaveAttribute("placeholder", "Stored; type a new one to replace it");
      await expect(page.getByRole("button", { name: "Ask for this change" })).toHaveAttribute("aria-disabled", "true");
      await page.getByRole("button", { name: "Send a test e-mail" }).click();
      await expect(page.getByText("The server took the test e-mail.", { exact: false })).toBeVisible();
      expect(world.tested).toBe(1);
      expect(await axeViolations(page)).toEqual([]);
      await shot(page, `installation-set-${width}`);
    });

    test(`an operator sets the installation's name, colours and logo for the Portal and the sign-in pages at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      const world: World = { smtp: null, smtpChange: null, tested: 0, branding: null, brandingChange: null };
      await stubApi(page, world);
      await page.goto("/organization/setup?lang=en");
      await page.getByRole("listitem").filter({ hasText: "How the sign-in pages look" }).getByRole("link", { name: "Set it" }).click();
      await expect(page.getByText("Nothing set here yet", { exact: false })).toBeVisible();
      await expect(page.getByRole("img", { name: "Sign-in page preview, Phone width" })).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
      await shot(page, `branding-empty-${width}`);

      await page.getByLabel("Logo", { exact: true }).setInputFiles({ name: "logo.svg", mimeType: "image/svg+xml", buffer: Buffer.from('<svg xmlns="http://www.w3.org/2000/svg" onload="alert(1)"/>') });
      await expect(page.getByText("The SVG holds script or an event handler.", { exact: false })).toBeVisible();
      await page.getByLabel("Full name", { exact: true }).fill("Smart City Banská Bystrica");
      await page.getByLabel("Primary colour", { exact: true }).fill("#0b539");
      await page.getByRole("button", { name: "Ask for this branding change" }).click();
      await expect(page.getByText("Must be a hex colour such as #0b5394")).toBeVisible();
      await shot(page, `branding-error-${width}`);

      const logo = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 120 32"><rect width="32" height="32" rx="6" fill="#0b5394"/><text x="40" y="22" font-size="16" fill="#0b5394">Smart BB</text></svg>';
      await page.getByLabel("Logo", { exact: true }).setInputFiles({ name: "logo.svg", mimeType: "image/svg+xml", buffer: Buffer.from(logo) });
      await expect(page.getByRole("img", { name: "Logo", exact: true })).toBeVisible();
      await page.getByLabel("Primary colour", { exact: true }).fill("#0b5394");
      await page.getByRole("button", { name: "Ask for this branding change" }).click();
      await expect(page.getByText("A branding change waits for a second operator.", { exact: false })).toBeVisible();
      await expect(page.getByLabel("Full name", { exact: true })).toBeDisabled();
      expect(await axeViolations(page)).toEqual([]);
      await shot(page, `branding-waiting-${width}`);

      // A second operator approved it: the Portal and the sign-in pages show it from the next request.
      world.branding = { instanceName: "Smart City Banská Bystrica", colours: { primary: "#0b5394" }, logoSvg: logo };
      world.brandingChange = null;
      await page.reload();
      await expect(page.getByLabel("Full name", { exact: true })).toHaveValue("Smart City Banská Bystrica");
      await expect(page.getByRole("button", { name: "Ask for this branding change" })).toHaveAttribute("aria-disabled", "true");
      expect(await axeViolations(page)).toEqual([]);
      await shot(page, `branding-set-${width}`);
    });

    test(`an Organization administrator reads that the operator sets it and meets no Installation page at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await stubApi(page, { smtp: null, smtpChange: null, tested: 0 }, false);
      await page.goto("/organization/setup?lang=en");
      await expect(page.getByText("Your operator sets this").first()).toBeVisible();
      await expect(page.getByRole("link", { name: "Set it" })).toHaveCount(0);
      await expect(page.getByText(/keycloak|branding_file|JC_BRANDING/i)).toHaveCount(0);
      await expect(page.getByRole("link", { name: "Installation" })).toHaveCount(0);
      await shot(page, `installation-setup-member-${width}`);
      await page.goto("/organization/operator-installation?lang=en");
      await expect(page.getByText("These sections belong to the installation's operators.", { exact: false })).toBeVisible();
      await expect(page.getByLabel("SMTP host", { exact: false })).toHaveCount(0);
      expect(await axeViolations(page)).toEqual([]);
      await shot(page, `installation-refused-${width}`);
    });
  }
});
