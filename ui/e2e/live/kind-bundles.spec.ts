/**
 * T-1538 — MF-16, MF-20, MF-22, MF-33, UI-44, PF-50: a Bundle, end to end on dev by a person.
 *
 * A Bundle is what an export records and an import reads, never a manifest a person types (owner,
 * 2026-09-25), so its journey is export then import. The steward creates one DataAgreement in
 * helsinki through its form, downloads helsinki from the project's Export dialog as YAML, keeps the
 * agreement's document from that download, and imports it into helsinki-mobility through the Import
 * page: Check, the report says one created, Propose, and the approver approves. The agreement then
 * stands in helsinki-mobility as it was exported, rewritten into that namespace (MF-22). Both copies
 * are removed whichever way the run ends.
 *
 * The agreement is never finalized and names a participant on example.org (RFC 2606), so neither
 * copy lets the gateway serve anything. A viewer meets the import's Propose disabled with its reason.
 */
import { readFileSync } from "node:fs";
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { parseAllDocuments } from "yaml";
import { journeyName, openedForm, proposeFrom } from "./kindJourney";
import { APPROVER, STEWARD, VIEWER, approve, proposedChange, removeCompletely, signIn } from "./portal";

test.setTimeout(900_000);

const SOURCE = "helsinki";
const TARGET = "helsinki-mobility";
const PLURAL = "dataagreements";

/** The one document of a YAML export that is this agreement, as the file a person would keep. */
function documentOf(exported: string, name: string): string {
  const found = parseAllDocuments(exported).find((document) => {
    const manifest = document.toJS() as { kind?: string; metadata?: { name?: string } } | null;
    return manifest?.kind === "DataAgreement" && manifest.metadata?.name === name;
  });
  expect(found, `the export of ${SOURCE} holds DataAgreement/${name}`).toBeDefined();
  return String(found);
}

async function stored(page: Page, project: string, name: string): Promise<{ status: number; spec?: Record<string, unknown> }> {
  const answer = await page.request.get(`/api/v1/projects/${project}/${PLURAL}/${name}`);
  return { status: answer.status(), spec: answer.ok() ? ((await answer.json()) as { spec: Record<string, unknown> }).spec : undefined };
}

test("a steward exports helsinki, imports one agreement from it into helsinki-mobility, and both copies are removed", async ({
  browser,
}) => {
  const steward = await signIn(browser, STEWARD, `/projects/${SOURCE}/${PLURAL}?lang=en`);
  const approver = await signIn(browser, APPROVER, `/projects/${TARGET}/approvals?lang=en`);
  const name = journeyName("t1538");
  let created = false;
  let imported = false;
  try {
    // The thing to carry: one agreement, created through its form and approved by another person.
    await steward.page.getByRole("main").getByRole("button", { name: "New data agreement" }).first().click();
    const form = await openedForm(steward.page);
    await form.locator("#root_name").fill(name);
    await form.locator("#root_remoteParticipant").fill("did:web:partner.example.org");
    await form.locator("#root_agreementId").fill(`urn:uuid:${name}`);
    await proposeFrom(form);
    const creation = await proposedChange(steward.page);
    created = true;
    await approve(approver.page, SOURCE, creation, name);
    await expect.poll(async () => (await stored(steward.page, SOURCE, name)).status, { timeout: 180_000 }).toBe(200);
    const original = (await stored(steward.page, SOURCE, name)).spec;

    // MF-16: the project's Export dialog, YAML, downloaded as a person downloads it.
    await steward.page.goto(`/projects/${SOURCE}/${PLURAL}?lang=en`, { waitUntil: "load" });
    await steward.page.getByRole("button", { name: "Export", exact: true }).first().click();
    const dialog = steward.page.getByRole("dialog", { name: "Download configuration" });
    await dialog.getByRole("radio", { name: "YAML" }).check();
    const [download] = await Promise.all([
      steward.page.waitForEvent("download", { timeout: 120_000 }),
      dialog.getByRole("button", { name: "Download" }).click(),
    ]);
    const exported = readFileSync(await download.path(), "utf8");
    expect(exported, "an export carries no status (MF-04)").not.toMatch(/^status:/m);
    const kept = documentOf(exported, name);

    // MF-20, MF-22, MF-33: the Import page of the other project checks the file, then proposes it.
    await steward.page.goto(`/projects/${TARGET}/import?lang=en`, { waitUntil: "load" });
    await expect(steward.page.getByRole("heading", { level: 1, name: "Import configuration" })).toBeVisible();
    await steward.page
      .getByLabel("Archive or manifests")
      .setInputFiles({ name: `${name}.yaml`, mimeType: "application/yaml", buffer: Buffer.from(kept) });
    const propose = steward.page.getByRole("button", { name: "Propose the import" });
    await expect(propose, "nothing is proposed from an unchecked bundle (MF-33)").toBeDisabled();
    await steward.page.getByRole("button", { name: "Check the bundle" }).click();
    const report = steward.page
      .getByRole("heading", { name: "What this import would do" })
      .locator("xpath=ancestor::section[1]");
    await expect(report.getByText(/^1 created · 0 replaced/)).toBeVisible({ timeout: 120_000 });
    await expect(propose).toBeEnabled();
    await propose.click();
    const importing = await proposedChange(steward.page);
    imported = true;
    await approve(approver.page, TARGET, importing, name);

    await expect
      .poll(async () => (await stored(steward.page, TARGET, name)).status, {
        timeout: 180_000,
        message: `DataAgreement/${name} stands in ${TARGET}`,
      })
      .toBe(200);
    expect((await stored(steward.page, TARGET, name)).spec, "the import carried the agreement as exported").toEqual(original);
  } finally {
    // The copy in the target first, then the original: as every live journey removes what it made.
    for (const [project, made] of [
      [TARGET, imported],
      [SOURCE, created],
    ] as const) {
      if (!made) continue;
      await removeCompletely(steward, project, PLURAL, name);
      await expect
        .poll(async () => (await stored(steward.page, project, name)).status, {
          timeout: 120_000,
          message: `DataAgreement/${name} is removed from ${project}`,
        })
        .toBe(404);
    }
    await approver.context.close();
    await steward.context.close();
  }
});

// UI-44, PF-50: the viewer may read the project, and meets the import's Propose disabled with why.
test("a viewer meets the import's Propose disabled with its reason", async ({ browser }) => {
  const { context, page } = await signIn(browser, VIEWER, `/projects/${SOURCE}/import?lang=en`);
  try {
    const name = journeyName("t1538v");
    const yaml = `apiVersion: joinedcontext.com/v1alpha1
kind: DataAgreement
metadata:
  name: ${name}
  namespace: ${SOURCE}
spec:
  role: consumer
  remoteParticipant: did:web:partner.example.org
  agreementId: urn:uuid:${name}
  state: requested
`;
    await page
      .getByLabel("Archive or manifests")
      .setInputFiles({ name: `${name}.yaml`, mimeType: "application/yaml", buffer: Buffer.from(yaml) });
    const propose = page.getByRole("button", { name: "Propose the import" });
    await expect(propose).toHaveAttribute("aria-disabled", "true");
    await expect(propose).toHaveAccessibleDescription(/propose/);
  } finally {
    await context.close();
  }
});
