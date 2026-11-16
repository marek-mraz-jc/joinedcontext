import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { axeViolations } from "./axe";
import { odataBody } from "../tests/odata";
import { manifestReadsAsRest } from "./manifestReads";

test.beforeEach(({ page }) => manifestReadsAsRest(page));

// T-7071, journey 3: an Organization administrator reads a Policy's list and tries a person
// against it (TryAPerson): what the gateway decides and which Policy decided it, a refusal with
// its next step, and the gateway not answering. Anyone else never sees the panel.

const API = "joinedcontext.com/v1alpha1";
const list = (items: unknown[]) => ({ apiVersion: API, kind: "List", items });
const PETRA_ID = "5f0c2a9e-4b1d-4c3e-9a7f-2d8e6b1c0a43";
const POLICY = {
  apiVersion: API,
  kind: "Policy",
  metadata: { name: "air-public-read", namespace: "helsinki" },
  spec: {
    contextSpaceRef: { kind: "ContextSpace", name: "air" },
    assigner: "did:web:hel.fi",
    assignee: { kind: "role", id: "public" },
    operations: ["retrieveOps"],
    information: [{ entities: [{ type: "AirQualityObserved" }] }],
  },
  status: { phase: "Live" },
};
const ENDPOINT = {
  apiVersion: API,
  kind: "Endpoint",
  metadata: { name: "air-public", namespace: "helsinki" },
  spec: { contextSpaceRef: "air", slug: "knimpgi7bnmz6khs7azlgfus7p", audience: "public" },
  status: { phase: "Live" },
};

type Answer = "allowed" | "refused" | "failed";

async function stub(page: Page, admin: boolean, answer: Answer, asked: string[]): Promise<void> {
  await page.route("**/api/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (path.includes("jc_access_simulate")) {
      // A bound function reads its parameters from the URL (OData), an action from the body.
      asked.push(decodeURIComponent(request.url()) + (request.postData() ?? ""));
      if (answer === "failed") {
        return route.fulfill({
          status: 503,
          contentType: "application/problem+json",
          body: JSON.stringify({ type: "about:blank", title: "Service Unavailable", status: 503, detail: "the gateway did not answer; try again shortly" }),
        });
      }
      const verdict =
        answer === "allowed"
          ? { decision: true, reason: "policy_grant_matched", policy: "air-public-read" }
          : { decision: false, reason: "no_grant", policy: null };
      return route.fulfill({ status: 200, contentType: "application/json", body: odataBody(verdict) });
    }
    if (path.endsWith("/auth/me")) return json({ subject: "a1", username: "ida", name: "Ida Admin", email: "ida@hel.fi", roles: [] });
    if (path === "/api/v1/projects") return json(list([{ name: "helsinki" }]));
    if (path === "/api/v1/preferences") return json({ firstRunDismissed: true });
    if (path.endsWith("/permissions/me")) {
      const rule = admin
        ? { kinds: ["RoleBinding", "Role", "Group", "Organization", "Project", "Person", "Policy"], verbs: ["read", "propose", "approve", "delete"] }
        : { kinds: ["Policy", "Endpoint"], verbs: ["read"] };
      return json({ project: "helsinki", bootstrap: false, grants: [{ role: admin ? "org-admin" : "viewer", binding: "b", scope: "organization", rule }] });
    }
    if (path === "/api/v1/organization/people") {
      return json({ items: [{ id: PETRA_ID, email: "petra.kral@example.org", firstName: "Petra", lastName: "Kráľ", enabled: true, emailVerified: true, requiredActions: [] }] });
    }
    if (path.endsWith("/policies")) return json(list([POLICY]));
    if (path.endsWith("/endpoints")) return json(list([ENDPOINT]));
    return json(list([]));
  });
}

async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.JC_EVIDENCE_DIR;
  if (!dir) return;
  mkdirSync(dir, { recursive: true });
  await page.screenshot({ path: join(dir, `${name}.png`), fullPage: true });
}

for (const width of [1440, 400]) {
  for (const answer of ["allowed", "refused", "failed"] as const) {
    test(`an administrator tries a person on a Policy at ${width} px: ${answer} (T-7071)`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      const asked: string[] = [];
      await stub(page, true, answer, asked);
      await page.goto("/projects/helsinki/policies?lang=en");
      await expect(page.getByRole("heading", { name: "Try a person" })).toBeVisible();
      await page.getByRole("button", { name: "Ask the gateway" }).click();
      if (answer === "allowed") {
        const verdict = page.getByTestId("try-verdict");
        await expect(verdict).toContainText("Allowed");
        await expect(verdict).toContainText("A Policy grants this action.");
        await expect(verdict.getByRole("link", { name: "air-public-read" })).toBeVisible();
      } else if (answer === "refused") {
        const verdict = page.getByTestId("try-verdict");
        await expect(verdict).toContainText("Refused");
        await expect(verdict).toContainText("To allow it, add a grant");
      } else {
        await expect(page.getByText("The gateway could not be asked. The reason and Retry are below.")).toBeVisible();
        await expect(page.getByText(/the gateway did not answer/)).toBeVisible();
      }
      // The question names the person by their id, never by a password or a token.
      expect(asked.join("")).toContain(PETRA_ID);
      expect(await axeViolations(page)).toEqual([]);
      await shot(page, `journey-try-${answer}-${width}`);
    });
  }

  test(`a person who does not administer the organization never sees Try a person at ${width} px (T-7071)`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    const asked: string[] = [];
    await stub(page, false, "allowed", asked);
    await page.goto("/projects/helsinki/policies?lang=en");
    await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
    await expect(page.getByRole("heading", { name: "Try a person" })).toHaveCount(0);
    expect(asked).toEqual([]);
    await shot(page, `journey-try-hidden-${width}`);
  });
}
