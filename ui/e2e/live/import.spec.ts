/**
 * T-2729 — importing a bundle on dev, as a person (MF-20, MF-23, MF-24, MF-33, UI-44).
 *
 * The steward gives the import page a bundle of one context space nobody has, checks it, reads
 * the report, and proposes it. The proposal is then rejected, so the journey proves the whole
 * door (upload, check, the one Change) and leaves helsinki as it found it: the space never
 * exists. The viewer meets Propose disabled with its reason, and the same upload sent to the
 * door answers 403 without a report.
 *
 * The page's shape in a browser is `e2e/import.spec.ts`; this run is the real Portal answering.
 */
import { expect, test } from "@playwright/test";
import { STEWARD, VIEWER, csrf, proposedChange, reject, signIn } from "./portal";

test.setTimeout(300_000);

const PROJECT = "helsinki";
const API = "joinedcontext.com/v1alpha1";

function bundle(name: string): { name: string; mimeType: string; buffer: Buffer } {
  const yaml = `apiVersion: ${API}
kind: ContextSpace
metadata:
  name: ${name}
  namespace: ${PROJECT}
spec:
  isSandbox: false
`;
  return { name: `${name}.yaml`, mimeType: "application/yaml", buffer: Buffer.from(yaml) };
}

test("a steward checks a bundle, reads what it would do, proposes it, and the rejected import leaves nothing", async ({
  browser,
}) => {
  const { context, page } = await signIn(browser, STEWARD, `/projects/${PROJECT}/import?lang=en`);
  const name = `t2729-imp-${Date.now().toString(36)}`;
  let change = "";
  try {
    await expect(page.getByRole("heading", { level: 1, name: "Import configuration" })).toBeVisible();
    await page.getByLabel("Archive or manifests").setInputFiles(bundle(name));
    const propose = page.getByRole("button", { name: "Propose the import" });
    await expect(propose, "nothing is proposed from an unchecked bundle (MF-33)").toBeDisabled();

    await page.getByRole("button", { name: "Check the bundle" }).click();
    const report = page.getByRole("heading", { name: "What this import would do" }).locator("xpath=ancestor::section[1]");
    await expect(report.getByText(/^1 created · 0 replaced/)).toBeVisible({ timeout: 120_000 });
    await report.getByText(/^1 created$/).click();
    await expect(report.getByText(`ContextSpace/${name}`)).toBeVisible();

    await expect(propose).toBeEnabled();
    await propose.click();
    change = await proposedChange(page);
    expect(change).toMatch(/^chg-[0-9a-f]{8}$/);
  } finally {
    if (change) await reject(page, PROJECT, change, "Rejected by the import journey: proposed only to prove the door.");
    const left = await page.request.get(`/api/v1/projects/${PROJECT}/contextspaces/${name}`);
    await context.close();
    expect(left.status(), "the rejected import created no space").toBe(404);
  }
});

test("a viewer meets Propose disabled with its reason, and the door refuses the same bundle", async ({ browser }) => {
  const { context, page } = await signIn(browser, VIEWER, `/projects/${PROJECT}/import?lang=en`);
  try {
    const name = `t2729-imp-${Date.now().toString(36)}`;
    await page.getByLabel("Archive or manifests").setInputFiles(bundle(name));
    const propose = page.getByRole("button", { name: "Propose the import" });
    await expect(propose).toHaveAttribute("aria-disabled", "true");
    await expect(propose).toHaveAccessibleDescription(/propose/);

    const answer = await page.request.post(`/api/v1/projects/${PROJECT}/import?dryRun=All`, {
      headers: { "x-csrf-token": await csrf(context) },
      multipart: { file: bundle(name), conflictPolicy: "fail" },
    });
    expect(answer.status(), "a viewer's import is refused at the door (PF-50)").toBe(403);
    expect(await answer.text()).not.toContain("created");
  } finally {
    await context.close();
  }
});

test("a credential written into a bundle is refused at the door, before a report, and never echoed", async ({ browser }) => {
  const { context, page } = await signIn(browser, STEWARD, `/projects/${PROJECT}/import?lang=en`);
  try {
    // Sent past the page, as a script would: the page's own check is a courtesy, this is the gate
    // (MF-24). Built from parts so no secret-shaped literal sits in the repository.
    const value = ["jc", "journey", Date.now().toString(36), "Zq7"].join("-");
    const name = `t2729-sec-${Date.now().toString(36)}`;
    const yaml = `apiVersion: ${API}
kind: DataSource
metadata:
  name: ${name}
  namespace: ${PROJECT}
spec:
  type: http
  url: https://example.org/feed.json
  password: ${value}
`;
    const answer = await page.request.post(`/api/v1/projects/${PROJECT}/import?dryRun=All`, {
      headers: { "x-csrf-token": await csrf(context) },
      multipart: {
        file: { name: `${name}.yaml`, mimeType: "application/yaml", buffer: Buffer.from(yaml) },
        conflictPolicy: "fail",
      },
    });
    expect(answer.status(), "a literal secret is refused, not checked").toBeGreaterThanOrEqual(400);
    expect(answer.status()).toBeLessThan(500);
    const body = await answer.text();
    expect(body, "the refusal names no value").not.toContain(value);
    expect(body).not.toContain('"created"');
  } finally {
    await context.close();
  }
});
