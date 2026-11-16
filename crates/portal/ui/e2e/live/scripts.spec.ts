/**
 * T-7084, PL-118: a stored Script on dev, end to end by two people. The steward writes version
 * 1.0.0 on the Scripts page, runs its tests on helsinki's runner and proposes it; the approver
 * approves; the steward edits the code, sees the changed output, proposes 1.1.0 and the approver
 * approves that too. Both versions are then on the Script, side by side. The move of pipeline pins
 * runs against stubs in e2e/scripts.spec.ts; afterAll removes the Script.
 */
import { expect, test } from "@playwright/test";
import type { BrowserContext, Page } from "@playwright/test";
import { APPROVER, STEWARD, approve, cleanUp, proposedChange, signIn } from "./portal";

const PROJECT = "helsinki";
const NAME = `t7084-${Date.now().toString(36)}`;
const PAGE = `/projects/${PROJECT}/scripts?lang=en`;
const V1 = "const record = bento.v0_msg_as_structured();\nrecord.checked = true;\nbento.v0_msg_set_structured(record);\n";
const V2 = "const record = bento.v0_msg_as_structured();\nrecord.checked = true;\nrecord.source = \"t7084\";\nbento.v0_msg_set_structured(record);\n";
const TESTS = (expected: Record<string, unknown>) => JSON.stringify([{ name: "marks-one", input: { id: "a" }, expected }]);

test.describe.configure({ mode: "serial" });
test.setTimeout(600_000);

let steward: { context: BrowserContext; page: Page };
let approver: { context: BrowserContext; page: Page };

test.beforeAll(async ({ browser }) => {
  steward = await signIn(browser, STEWARD, PAGE);
  approver = await signIn(browser, APPROVER, `/projects/${PROJECT}/approvals?lang=en`);
});

test.afterAll(async () => {
  await cleanUp(steward, PROJECT, "scripts", NAME, "scripts.spec.ts");
  await steward.context.close();
  await approver.context.close();
});

async function writeCode(page: Page, code: string): Promise<void> {
  // Monaco's own input is a zero-size edit context: a person clicks the lines.
  await page.locator(".monaco-editor .view-lines").click();
  await page.keyboard.press("ControlOrMeta+a");
  await page.keyboard.press("Delete");
  await page.keyboard.insertText(code);
}

async function runAndPropose(page: Page, version: string): Promise<string> {
  await page.getByRole("button", { name: "Run tests" }).click();
  const results = page.getByRole("table", { name: "Test results" });
  await expect(results.getByText("Pass").first()).toBeVisible({ timeout: 120_000 });
  await expect(page.getByRole("textbox", { name: "Version" })).toHaveValue(version);
  await page.getByRole("button", { name: `Propose version ${version}` }).click();
  return proposedChange(page);
}

test("a script version is written, tested on the runner, proposed and approved by a second person", async () => {
  const { page } = steward;
  await page.goto(PAGE);
  await expect(page.getByRole("heading", { name: "Script workbench" })).toBeVisible({ timeout: 60_000 });
  await page.getByRole("textbox", { name: "Name" }).fill(NAME);
  await writeCode(page, V1);
  await page.getByRole("textbox", { name: "Tests" }).fill(TESTS({ id: "a", checked: true }));
  await approve(approver.page, PROJECT, await runAndPropose(page, "1.0.0"));

  await expect
    .poll(async () => (await page.request.get(`/api/v1/projects/${PROJECT}/scripts/${NAME}`)).status(), { timeout: 180_000, intervals: [5_000] })
    .toBe(200);
  await page.goto(PAGE);
  await expect(page.getByRole("option", { name: NAME })).toBeAttached({ timeout: 60_000 });
  await page.getByRole("combobox", { name: "Script", exact: true }).selectOption(NAME);
  await writeCode(page, V2);
  await page.getByRole("textbox", { name: "Tests" }).fill(TESTS({ id: "a", checked: true, source: "t7084" }));
  await page.getByRole("button", { name: "Run tests" }).click();
  await expect(page.getByRole("table", { name: "Test results" }).getByText("Changed").first()).toBeVisible({ timeout: 120_000 });
  await approve(approver.page, PROJECT, await runAndPropose(page, "1.1.0"));

  await expect
    .poll(
      async () => {
        const answer = await page.request.get(`/api/v1/projects/${PROJECT}/scripts/${NAME}`);
        const script = (await answer.json()) as { spec?: { versions?: { version: string }[] } };
        return (script.spec?.versions ?? []).map((v) => v.version);
      },
      { timeout: 180_000, intervals: [5_000] },
    )
    .toEqual(["1.0.0", "1.1.0"]);
});
