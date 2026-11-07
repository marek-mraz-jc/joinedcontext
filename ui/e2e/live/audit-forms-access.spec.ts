/**
 * T-4158 — UX photo audit 2/7: the forms and lists of access, endpoints, policies, MCP, the
 * Organization, project settings and workspaces. Every list (as demo.steward and as demo.viewer,
 * whose view shows the disabled controls and their reasons), every create and edit form as it
 * opens, the same form after it was checked empty (its validation messages), each dialog and
 * the detail pages, at 375, 768 and 1440 px in sk and en, saved by `shot` (shoot.ts, T-4156)
 * under `$SHOTS_DIR`, which then gets its contact sheet.
 *
 * Read-only: a form is validated with its Check button, which asks the server's dry run and
 * writes nothing; a dialog without one gets its submit ("Check and propose", Save) pressed only
 * while every field in it is empty, which the form refuses, and every dialog is closed with Escape.
 *
 * The Portal edge allows 300 requests a minute per address, shared by every agent on the server
 * (T-4158 body, 21:30Z): the run waits for `networkidle`, and a state whose page saw a 429 or a
 * failed script is retaken after a pause, never saved as a white page. One audit at a time:
 * whoever starts it holds `tasks/.audit.lock`. The forms are the ones `tests/controls.json`
 * records for these modules: the run fails when a module there has no entry in FORM_OF.
 *
 *   SHOTS_DIR=/workspace/evidence/<agent>/T-4158 PORTAL_PASSWORD=… VIEWER_PASSWORD=… \
 *     npx playwright test --config playwright.live.config.ts audit-forms-access
 * `AUDIT_ONLY=<regex>` narrows the run to the routes it matches.
 */
import { readFileSync, readdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { expect, test } from "@playwright/test";
import type { Locator, Page } from "@playwright/test";
import { STEWARD, VIEWER, signIn } from "./portal";
import { shot, shotName, writeIndex } from "./shoot";
import type { Shot } from "./shoot";

const here = dirname(fileURLToPath(import.meta.url));
const LANGS = ["sk", "en"] as const;
type Lang = (typeof LANGS)[number];
type Who = "steward" | "viewer";
const WIDTHS = [375, 768, 1440];
/** The viewer's pages differ from the steward's only in what is disabled: the phone and the desk. */
const VIEWER_WIDTHS = [375, 1440];
const P = "/projects/helsinki";
const DIR = process.env.SHOTS_DIR;
const ONLY = process.env.AUDIT_ONLY ? new RegExp(process.env.AUDIT_ONLY) : null;
const MODULES = ["access", "endpoints", "policies", "mcp", "organization", "projectSettings", "workspaces"];

const locales = Object.fromEntries(
  LANGS.map((lang) => [lang, JSON.parse(readFileSync(join(here, `../../src/locales/${lang}.json`), "utf8")) as Record<string, unknown>]),
) as Record<Lang, Record<string, unknown>>;

function words(lang: Lang, key: string): string {
  const value = key.split(".").reduce<unknown>((node, part) => (node as Record<string, unknown>)?.[part], locales[lang]);
  if (typeof value !== "string") throw new Error(`no ${lang} text at ${key}`);
  return value;
}

const escaped = (text: string) => text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
const startsWith = (text: string) => new RegExp(`^\\s*${escaped(text)}`);
const exactly = (text: string) => new RegExp(`^\\s*${escaped(text)}\\s*$`);

/** A list or page: photographed as it opens, for both people. */
interface Listed {
  area: string;
  route: string;
  state: string;
}

const LISTS: Listed[] = [
  ...["general", "members", "roles", "service-accounts", "access", "explorer", "danger"].map((tab) => ({
    area: "projectSettings",
    route: `${P}/settings/${tab}`,
    state: "tab",
  })),
  { area: "endpoints", route: `${P}/endpoints`, state: "list" },
  { area: "endpoints", route: `${P}/endpoints/app-air-quality`, state: "detail" },
  { area: "policies", route: `${P}/policies`, state: "list" },
  { area: "mcp", route: `${P}/mcp`, state: "list" },
  { area: "workspaces", route: `${P}/workspaces`, state: "list" },
  ...["settings", "people", "members", "roles", "groups", "service-accounts", "applications", "endpoints", "mcp-servers"].map((tab) => ({
    area: "organization",
    route: `/organization/${tab}`,
    state: "tab",
  })),
  { area: "access", route: "/organization/groups/air-quality-steward", state: "detail" },
];

/**
 * A form: `route` is where it opens, `button` (a locale key) the control that opens it as a
 * dialog there, none for a form with an address of its own. `validate: false` for a form that
 * has nothing to validate (a confirmation, a file picker) and for an edit form, which opens
 * filled and so is never submitted: its kind's new form shows the validation messages.
 */
interface Form {
  id: string;
  area: string;
  route: string;
  button?: string;
  validate?: boolean;
}

const FORMS: Form[] = [
  { id: "project-edit", area: "projectSettings", route: `${P}/settings/general`, button: "resourceEdit.button", validate: false },
  { id: "project-member-new", area: "access", route: `${P}/settings/members/new` },
  { id: "project-member-edit", area: "access", route: `${P}/settings/members/janitors/edit`, validate: false },
  { id: "project-role-new", area: "access", route: `${P}/settings/roles/new` },
  { id: "project-serviceaccount-new", area: "access", route: `${P}/settings/service-accounts/new` },
  { id: "project-delete", area: "projectSettings", route: `${P}/settings/danger`, button: "projectDelete.button", validate: false },
  { id: "endpoint-new", area: "endpoints", route: `${P}/endpoints/new` },
  { id: "endpoint-edit", area: "endpoints", route: `${P}/endpoints/app-air-quality/edit`, validate: false },
  { id: "policy-new", area: "policies", route: `${P}/policies/new` },
  { id: "policy-edit", area: "policies", route: `${P}/policies/app-air-quality-1/edit`, validate: false },
  { id: "mcp-new", area: "mcp", route: `${P}/mcp`, button: "mcp.new" },
  // Opens with a name already suggested, so it is never empty and never submitted.
  { id: "copy-new", area: "workspaces", route: `${P}/workspaces`, button: "workspaces.new", validate: false },
  { id: "organization-edit", area: "organization", route: "/organization/settings", button: "resourceEdit.button", validate: false },
  { id: "person-new", area: "organization", route: "/organization/people", button: "organization.people.new" },
  { id: "org-member-new", area: "access", route: "/organization/members/new" },
  { id: "org-role-new", area: "access", route: "/organization/roles/new" },
  { id: "org-role-edit", area: "access", route: "/organization/roles/app-editor/edit", validate: false },
  { id: "group-new", area: "access", route: "/organization/groups/new" },
  { id: "org-serviceaccount-new", area: "access", route: "/organization/service-accounts/new" },
  { id: "app-import", area: "organization", route: "/organization/applications", button: "appImport.button", validate: false },
  { id: "org-mcp-new", area: "mcp", route: "/organization/mcp-servers", button: "mcp.new" },
];

/**
 * Every module `tests/controls.json` records under these pages, and the form that photographs
 * its controls, or why it has none of its own (a panel shown on a page the run already takes).
 */
const FORM_OF: Record<string, string> = {
  "src/pages/access/AccessExplorer.tsx": "page: project settings, explorer tab",
  "src/pages/access/GroupPage.tsx": "page: /organization/groups/air-quality-steward",
  "src/pages/access/Groups.tsx": "group-new",
  "src/pages/access/OrganizationDomain.tsx": "page: organization settings tab",
  "src/pages/access/RoleBindings.tsx": "project-member-new",
  "src/pages/access/Roles.tsx": "project-role-new",
  "src/pages/access/ServiceAccounts.tsx": "project-serviceaccount-new",
  "src/pages/endpoints/CatalogSection.tsx": "page: organization endpoints tab",
  "src/pages/endpoints/EndpointPage.tsx": "endpoint-edit",
  "src/pages/endpoints/FilterProof.tsx": "page: endpoint detail",
  "src/pages/endpoints/ModelPicker.tsx": "endpoint-new",
  "src/pages/endpoints/Playground.tsx": "page: endpoint detail",
  "src/pages/endpoints/SchemaProjectionPanel.tsx": "endpoint-new",
  "src/pages/endpoints/ShareFlow.tsx": "page: endpoint detail",
  "src/pages/mcp/McpServerDialog.tsx": "mcp-new",
  "src/pages/mcp/McpServerPanels.tsx": "page: project MCP list",
  "src/pages/mcp/McpServersPage.tsx": "page: project MCP list",
  "src/pages/organization/OrganizationApplications.tsx": "app-import",
  "src/pages/organization/OrganizationPage.tsx": "organization-edit",
  "src/pages/organization/People.tsx": "person-new",
  "src/pages/policies/TryAPerson.tsx": "page: policies list",
  "src/pages/workspaces/BringBackPage.tsx": "copy-new",
  "src/pages/workspaces/TryItPage.tsx": "copy-new",
};

const missed: string[] = [];
/** Forms whose empty check showed no message on a field and no alert: findings, not misses. */
const silent = new Set<string>();

/** Pages that saw a refused or failed request since their state began: retaken, not saved. */
const refused = new WeakMap<Page, string>();

function watch(page: Page): void {
  page.on("response", (response) => {
    if (response.status() === 429) refused.set(page, `429 on ${response.url()}`);
  });
  page.on("requestfailed", (request) => {
    // A chunk that did not load leaves a white page; a fetch a dialog or a navigation cancels is routine.
    if (["script", "stylesheet"].includes(request.resourceType()) && !request.failure()?.errorText.includes("ERR_ABORTED")) {
      refused.set(page, `failed ${request.url()}`);
    }
  });
}

async function settle(page: Page): Promise<void> {
  await page.waitForLoadState("networkidle", { timeout: 20_000 }).catch(() => undefined);
  await expect(page.locator('[aria-busy="true"]')).toHaveCount(0, { timeout: 15_000 }).catch(() => undefined);
  await page.waitForTimeout(1200);
}

async function open(page: Page, address: string, lang: Lang, wait = 60_000): Promise<void> {
  await page.goto(`${address}${address.includes("?") ? "&" : "?"}lang=${lang}`, { waitUntil: "load" });
  // A visible heading, not the navigation: on a phone the navigation is a closed drawer, and a
  // routed form's heading is outside `main`'s own.
  await expect(page.locator("h1:visible").first()).toBeVisible({ timeout: wait });
}

async function state(page: Page, what: Omit<Shot, "width">, reach: () => Promise<void>): Promise<void> {
  let last = "";
  for (let attempt = 0; attempt < 3; attempt += 1) {
    try {
      refused.delete(page);
      await reach();
      await settle(page);
      const why = refused.get(page);
      if (why) throw new Error(`retake: ${why}`);
      // A full-page picture of a scrolled page paints the sticky header where the scroll left it.
      await page.evaluate(() => window.scrollTo(0, 0));
      await shot(page, what);
      return;
    } catch (error) {
      last = String(error).split("\n")[0];
      console.log(`${what.route} ${what.state} ${what.who} ${what.lang} ${page.viewportSize()?.width}: ${last}`);
      if (process.env.AUDIT_DEBUG) await page.screenshot({ path: `${process.env.AUDIT_DEBUG}/${what.state}-${what.lang}-${attempt}.png` }).catch(() => undefined);
      // A refused request is retaken up to three times; anything else once (a control still
      // disabled while the person's rights load).
      if (!last.includes("retake:") && attempt > 0) break;
      await page.keyboard.press("Escape").catch(() => undefined);
      await page.waitForTimeout(30_000);
    }
  }
  missed.push(`${what.route} ${what.state} ${what.who} ${what.lang} ${page.viewportSize()?.width}: ${last}`);
}

/** Opens the form; returns what holds its fields: the dialog, or the form's page section. */
async function openForm(page: Page, form: Form, lang: Lang, wait = 30_000): Promise<Locator> {
  await open(page, form.route, lang, wait * 2);
  // An opener may name its target too ("Edit helsinki"), so its words only begin the name.
  if (form.button) await page.getByRole("button", { name: startsWith(words(lang, form.button)) }).first().click({ timeout: wait });
  // Under a FormRouteHost (`…/new`, `…/{name}/edit`, and a button on such a tab) the form is a
  // section of the page; elsewhere it is a dialog (FormRoute.tsx).
  // The assistant dock mounts a hidden dialog of its own later: only a visible frame counts.
  const frame = page.getByRole("dialog").or(page.getByTestId("form-page")).filter({ visible: true }).last();
  await expect(frame).toBeVisible({ timeout: wait });
  return frame;
}

/** Shows the form's validation without writing: Check where it has one, else submit while empty. */
async function checkEmpty(scope: Locator, lang: Lang): Promise<boolean> {
  const check = scope.getByRole("button", { name: exactly(words(lang, "form.check")) });
  if (await check.count()) {
    await check.first().click();
  } else {
    const filled = await scope
      .locator('input:visible:not([type=checkbox]):not([type=radio]):not([type=hidden]):not([readonly]), textarea:visible')
      .evaluateAll((fields) => fields.filter((field) => (field as HTMLInputElement).value.trim() !== "").length);
    if (filled) throw new Error(`${filled} fields hold a value: the form is not empty, so it is not submitted`);
    // The dialog's primary action is its last button: Save, or "Check and propose".
    const typed = scope.locator("button[type=submit]:visible");
    const submit = (await typed.count()) ? typed : scope.locator("button:visible").last();
    if (!(await submit.count())) throw new Error("no Check and no submit button");
    if (await submit.first().isDisabled()) return true;
    await submit.first().click();
  }
  return scope
    .locator('[aria-invalid="true"], [role="alert"]')
    .first()
    .waitFor({ timeout: 15_000 })
    .then(() => true)
    .catch(() => false);
}

async function walk(page: Page, who: Who, lang: Lang): Promise<void> {
  for (const item of LISTS.filter((entry) => !ONLY || ONLY.test(entry.route))) {
    await state(page, { area: item.area, route: item.route, state: item.state, who, lang }, () => open(page, item.route, lang));
  }
  for (const form of FORMS.filter((entry) => !ONLY || ONLY.test(entry.route))) {
    const base = { area: form.area, route: form.route, who, lang };
    // The viewer's view of a form is where its disabled controls give their reasons; nothing is pressed.
    let scope = null as Locator | null;
    await state(page, { ...base, state: `${form.id}-open` }, async () => {
      if (who === "steward") {
        scope = await openForm(page, form, lang);
        return;
      }
      // What the viewer gets is the picture, whatever it is: an opener disabled with its reason,
      // a form read-only, a refusal, or a page that never finishes loading (a finding).
      await openForm(page, form, lang, 10_000).catch(() => undefined);
    });
    if (who === "steward" && form.validate !== false && scope) {
      const opened: Locator = scope;
      await state(page, { ...base, state: `${form.id}-validation` }, async () => {
        if (!(await checkEmpty(opened, lang))) silent.add(form.id);
      });
    }
    await page.keyboard.press("Escape").catch(() => undefined);
    await page.keyboard.press("Escape").catch(() => undefined);
  }
}

test.describe.configure({ mode: "serial" });
test.setTimeout(5_400_000);

test("every module in controls.json under these pages has its form here", () => {
  const modules = Object.keys((JSON.parse(readFileSync(join(here, "../../tests/controls.json"), "utf8")) as { modules: object }).modules);
  const ours = modules.filter((file) => MODULES.some((module) => file.startsWith(`src/pages/${module}/`)));
  expect(ours.filter((file) => !FORM_OF[file]), "a module with controls and no form in this audit").toEqual([]);
  const ids = new Set(FORMS.map((form) => form.id));
  expect(Object.values(FORM_OF).filter((target) => !target.startsWith("page: ") && !ids.has(target))).toEqual([]);
});

for (const [who, person] of [
  ["steward", STEWARD],
  ["viewer", VIEWER],
] as const) {
  test(`forms and lists as demo.${who}`, async ({ browser }) => {
    test.skip(!DIR, "SHOTS_DIR is not set");
    const { context, page } = await signIn(browser, person, `${P}/home?lang=en`);
    watch(page);
    try {
      for (const width of who === "viewer" ? VIEWER_WIDTHS : WIDTHS) {
        await page.setViewportSize({ width, height: width < 768 ? 812 : 900 });
        for (const lang of LANGS) await walk(page, who, lang);
      }
    } finally {
      await context.close();
    }
  });
}

test.afterAll(() => {
  if (!DIR) return;
  const count = writeIndex(DIR);
  const files = new Set(readdirSync(DIR));
  console.log(`UX photo audit, forms and access: ${count} screenshots in ${DIR}`);
  if (silent.size) console.log(`checked empty with no message shown: ${[...silent].join(", ")}`);
  if (missed.length) console.log(`not reached (${missed.length}):\n${missed.join("\n")}`);
  if (ONLY) return;
  // Every form has its open and its validation picture, in both languages and at every width.
  const absent = FORMS.flatMap((form) =>
    (form.validate === false ? ["open"] : ["open", "validation"]).flatMap((kind) =>
      WIDTHS.flatMap((width) =>
        LANGS.map((lang) => shotName({ area: form.area, route: form.route, state: `${form.id}-${kind}`, who: "steward", lang, width })),
      ),
    ),
  ).filter((file) => !files.has(file));
  expect(absent, "every form's open and validation shots").toEqual([]);
  expect(count).toBeGreaterThanOrEqual(90);
});
