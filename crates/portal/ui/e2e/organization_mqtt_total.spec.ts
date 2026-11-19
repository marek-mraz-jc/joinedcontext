import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { axeViolations } from "./axe";
import { manifestReadsAsRest } from "./manifestReads";

// The pages read their manifests through the OData service; this file stubs the REST reads.
test.beforeEach(({ page }) => manifestReadsAsRest(page));

// T-7407: every Organization runs on the deployment's MQTT default of ten brokers over all its
// projects together. Settings shows the number in force, what it counts and what lowering it does
// (Edit proposes it as a Change of `spec.limits.mqtt.brokers`), and a steward's eleventh broker is
// refused with the limit, its origin and the count. `vite preview` has no Portal API behind it, so
// the browser answers as the server answers Helsinki's administrator and steward. Set
// JC_EVIDENCE_DIR to keep the screenshots.

const API = "joinedcontext.com/v1alpha1";
const PROJECT = "helsinki";
const list = (items: unknown[]) => ({ apiVersion: API, kind: "List", items });
const HEL = {
  apiVersion: API,
  kind: "Organization",
  metadata: { name: "hel", namespace: "org" },
  spec: { domain: "hel.fi", locales: ["fi", "en"], defaultLocale: "fi" },
};
const ADMIN = {
  project: "org",
  bootstrap: false,
  grants: [{ role: "org-admin", binding: "hel-admins", scope: "organization", organization: "hel", rule: { kinds: ["Organization"], verbs: ["propose", "approve"] } }],
};
const STEWARD = {
  project: PROJECT,
  bootstrap: false,
  grants: [{ role: "steward", binding: "stewards", rule: { kinds: ["MqttBroker", "ServiceAccount"], verbs: ["read", "propose"] } }],
};
// The server's 422 past the Organization's total (crates/portal/src/mqtt_allowance.rs).
const ELEVENTH = {
  type: "https://joinedcontext.com/problems/unprocessable",
  title: "Unprocessable Content",
  status: 422,
  detail:
    "The Organization may run 10 MQTT brokers over all its projects and 10 run already. An org admin can raise the Organization's limit (spec.limits.mqtt.brokers) in Organization settings > MQTT (PF-118).",
  errors: ["metadata.name: vents would be MQTT broker 11 of the Organization's 10 (limit spec.limits.mqtt.brokers, origin Organization)"],
};
const meter = (limit: number, used: number) => ({ limit, used, left: Math.max(limit - used, 0) });

async function stubApi(page: Page, admin: boolean): Promise<void> {
  await page.route("**/api/**", async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const path = url.pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json({ subject: "a1", username: "aino", name: "Aino Virtanen", roles: [] });
    if (path === "/api/v1/projects") {
      return json({
        ...list([{ name: PROJECT, sample: false, organization: "hel", title: "Helsinki" }]),
        organizations: [{ name: "hel", title: { en: "City of Helsinki" } }],
      });
    }
    // Settings reads its catalog through the OData function `jc_organization_read(aspect='limits')`.
    const reading = decodeURIComponent(path).includes(".jc_organization_read(") ? url.searchParams.get("@aspect") : null;
    if (reading !== null && reading !== "'limits'") return json({ value: reading === "'widgetOptions'" ? { options: [] } : { items: [] } });
    if (reading === "'limits'" || path === "/api/v1/organization/limits") {
      const limits = {
        // The catalog's MQTT section with the deployment's default in force (org_settings.rs).
        entries: [
          ["spec.limits.mqtt.brokers", 10, 20],
          ["spec.limits.mqtt.connections", 1000, 20000],
          ["spec.limits.mqtt.messagesPerSecond", 10000, 100000],
          ["spec.limits.mqtt.maxMessageBytes", 65536, 1048576],
        ].map(([path, value, max]) => ({ path, section: "mqtt", security: false, default: value, min: 0, max, value: null, origin: "default" })),
        projects: [],
        secondFactorFloor: "optional",
      };
      return json(reading === null ? limits : { value: limits });
    }
    if (path.endsWith("/permissions/me")) return json(admin ? ADMIN : STEWARD);
    if (path === "/api/v1/projects/org/organizations") return json(list([HEL]));
    if (path.endsWith("/mqtt/allowance")) {
      return json({
        enabled: true,
        origin: "default",
        raise: `an org admin can raise this in Organization settings > Projects > ${PROJECT}`,
        modes: ["ingest"],
        addresses: ["platform"],
        ports: [],
        brokers: meter(10, 4),
        connections: meter(1000, 400),
        messagesPerSecond: meter(10000, 400),
        maxMessageBytes: { limit: 65536 },
        portsInUse: [],
      });
    }
    if (/\/drafts\//.test(path)) {
      if (request.method() !== "PUT") return route.fulfill({ status: 204 });
      const { manifest } = request.postDataJSON() as { manifest: unknown };
      return json({ project: "org", kind: "Organization", name: "hel", manifest, version: 1, updatedAt: new Date().toISOString() });
    }
    if (path === "/api/v1/projects/org/organizations/hel") {
      if (request.method() === "GET") return json(HEL);
      if (url.searchParams.get("dryRun")) return json({ valid: true });
      return json({ apiVersion: API, kind: "Change", metadata: { name: "chg-mqtt12", namespace: "org" }, spec: {}, status: { phase: "Proposed" } }, 201);
    }
    if (request.method() === "PUT" || request.method() === "POST") {
      const body = request.postDataJSON() as { kind?: string };
      if (body.kind === "MqttBroker") {
        return route.fulfill({ status: 422, contentType: "application/problem+json", body: JSON.stringify(ELEVENTH) });
      }
      return json({ valid: true });
    }
    if (path.endsWith("/serviceaccounts")) {
      return json(list([{ apiVersion: API, kind: "ServiceAccount", metadata: { name: "vent-1", namespace: PROJECT }, spec: {} }]));
    }
    return json(list([]));
  });
}

async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.JC_EVIDENCE_DIR;
  if (!dir) return;
  mkdirSync(dir, { recursive: true });
  await page.screenshot({ path: join(dir, `${name}.png`), animations: "disabled" });
}

test.describe("an Organization's MQTT total", () => {
  for (const width of [1440, 400]) {
    test(`shows ten brokers over all projects and what lowering does at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await stubApi(page, true);
      await page.goto("/organization/settings?lang=en");
      const row = page.getByRole("row", { name: /MQTT brokers in the organization/ });
      await expect(row).toContainText("10");
      await expect(row).toContainText("the existing brokers stay and new ones are refused");
      await row.evaluate((element) => element.scrollIntoView({ block: "center" }));
      await shot(page, `mqtt-total-settings-${width}`);

      expect(await axeViolations(page)).toEqual([]);
      await expect(page.getByRole("button", { name: "Edit hel.fi" })).toBeVisible();
    });

    test(`refuses the eleventh broker with the Organization's limit at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await stubApi(page, false);
      await page.goto(`/projects/${PROJECT}/mqttbrokers/new?lang=en`);
      await page.getByRole("textbox", { name: /^Name/ }).first().fill("vents");
      await page.getByRole("textbox", { name: /^Role/ }).first().fill("vents");
      await page.getByRole("combobox", { name: /Service accounts/ }).first().selectOption("vent-1");
      await page.getByRole("button", { name: "Check", exact: true }).click();
      const reason = page.getByText(/The Organization may run 10 MQTT brokers over all its projects and 10 run already\. An org admin can raise the Organization's limit/).first();
      await expect(reason).toBeVisible();
      await reason.evaluate((element) => element.scrollIntoView({ block: "center" }));
      await shot(page, `mqtt-total-eleventh-${width}`);
    });
  }
});
