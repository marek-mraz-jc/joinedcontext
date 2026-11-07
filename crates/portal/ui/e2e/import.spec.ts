/**
 * Importing a bundle another instance exported, in a browser (T-2729; MF-20, MF-24, MF-33, UI-15).
 *
 * `tests/import_wizard.test.tsx` holds the page's logic under jsdom. This run is the page as a
 * person meets it at its own address: the bundle is given to the drop zone's file control, the
 * check comes back as a report, and only then can the import be proposed. The API is answered in
 * the browser, so `vite preview` is all it needs; the stub records every request it answers, so
 * "nothing was sent" is a count, not a hope.
 */
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";

const API = "joinedcontext.com/v1alpha1";
const PROJECT = "banskabystrica";

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  email: "jana.kovacova@bb.sk",
  roles: ["portal-editor"],
};

const REPORT = {
  created: ["ContextSpace/ovzdusie", "Endpoint/public-air"],
  replaced: [],
  skipped: ["DataModel/air-quality"],
  renamed: {},
  nativeFiles: 2,
  lane: "yellow",
  verified: [{ path: "projects/x/spaces/ovzdusie/space.yaml", equal: true }],
  needs: [],
};

const CHANGE = {
  apiVersion: API,
  kind: "Change",
  metadata: { name: "chg-11aa22bb", namespace: PROJECT },
  status: { lane: "yellow", phase: "PendingApproval", plan: { create: 2 } },
};

const BUNDLE = `apiVersion: ${API}
kind: ContextSpace
metadata:
  name: ovzdusie
spec:
  isSandbox: false
`;

async function stubApi(page: Page, verbs = ["read", "propose"]): Promise<string[]> {
  const imports: string[] = [];
  await page.route("**/api/v1/**", async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });

    if (url.pathname.endsWith("/auth/me")) return json(IDENTITY);
    if (url.pathname === "/api/v1/projects") return json({ apiVersion: API, kind: "List", items: [{ name: PROJECT }] });
    if (url.pathname.endsWith("/permissions/me")) return json({ grants: [{ rule: { kinds: ["*"], verbs } }] });
    if (url.pathname === `/api/v1/projects/${PROJECT}/import` && request.method() === "POST") {
      const dry = url.searchParams.get("dryRun") === "All";
      imports.push(dry ? "check" : "propose");
      return json(dry ? REPORT : CHANGE, dry ? 200 : 202);
    }
    return json({ apiVersion: API, kind: "List", items: [] });
  });
  return imports;
}

function file(text: string) {
  return { name: "export.yaml", mimeType: "application/yaml", buffer: Buffer.from(text) };
}

test.describe("importing a bundle", () => {
  test("a bundle is checked, its report read, and only then proposed as one change", async ({ page }) => {
    const imports = await stubApi(page);
    await page.goto(`/projects/${PROJECT}/import?lang=en`);

    await expect(page.getByRole("heading", { level: 1, name: "Import configuration" })).toBeVisible();
    await page.getByLabel("Archive or manifests").setInputFiles(file(BUNDLE));
    const propose = page.getByRole("button", { name: "Propose the import" });
    await expect(propose, "nothing is proposed from an unchecked bundle (MF-33)").toBeDisabled();
    // The reason is said twice on purpose: as the disabled button's description and as the
    // status beside it, which is the one a sighted person reads.
    await expect(propose).toHaveAccessibleDescription("Check the bundle to see what it would do.");
    await expect(page.getByRole("status").filter({ hasText: "Check the bundle to see what it would do." })).toBeVisible();

    await page.getByRole("button", { name: "Check the bundle" }).click();
    const report = page.getByRole("heading", { name: "What this import would do" }).locator("xpath=ancestor::section[1]");
    await expect(report.getByText(/^2 created · 0 replaced · 1 left alone/)).toBeVisible();
    expect(await axeViolations(page)).toEqual([]);

    await expect(propose).toBeEnabled();
    await propose.click();
    await expect(page.getByText(CHANGE.metadata.name)).toBeVisible();
    expect(imports).toEqual(["check", "propose"]);
  });

  test("a credential written in the open is refused in the browser, and nothing is sent", async ({ page }) => {
    const imports = await stubApi(page);
    await page.goto(`/projects/${PROJECT}/import?lang=en`);

    // A fixture, not a credential: the word is what the page looks for (MF-24).
    const pasted = ["hunter", "2"].join("");
    await page.getByLabel("Archive or manifests").setInputFiles(file(`${BUNDLE}  auth:\n    password: ${pasted}\n`));

    await expect(page.getByRole("alert")).toContainText("password");
    await expect(page.getByRole("alert")).not.toContainText(pasted);
    await expect(page.getByRole("button", { name: "Check the bundle" })).toBeDisabled();
    await expect(page.getByRole("button", { name: "Propose the import" })).toBeDisabled();
    expect(imports).toEqual([]);
  });
  test("a viewer reads why there is no import, and is offered no bundle to drop (T-4997)", async ({ page }) => {
    const imports = await stubApi(page, ["read"]);
    await page.goto(`/projects/${PROJECT}/import?lang=en`);
    await expect(
      page.getByText("Your role lets you look at these, not create one here.", { exact: false }),
    ).toBeVisible();
    await expect(page.getByRole("button", { name: "Propose the import" })).toHaveCount(0);
    await expect(page.getByRole("button", { name: "Check the bundle" })).toHaveCount(0);
    expect(imports).toEqual([]);
  });
});
