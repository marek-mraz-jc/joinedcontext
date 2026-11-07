import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";

// My AI connectors (T-5951, AG-142): create from the tick tree, copy the setup line, revoke after an
// inline confirm, read the activity; and the refusal the server gives beside the field it names.
// `vite preview` has no Portal behind it, so the API is answered in the browser.

const IDENTITY = { subject: "b7c1e0f4", username: "jana.kovacova", name: "Jana Kováčová", email: "jana.kovacova@hel.fi", roles: [] };
const ID = "abcdefghijklmnopqrstuvwxyz";
const URL_OF = `https://portal.dev.example/c/${ID}`;
const OPTIONS = {
  segments: ["data-read", "context", "pipeline", "access", "publish", "apps", "operate", "knowledge", "admin"],
  segmentKinds: [
    { segment: "context", kinds: ["ContextSpace", "DataModel", "Endpoint", "Policy", "ModelProjection", "Subscription"] },
    { segment: "pipeline", kinds: ["Pipeline", "DataSource", "MqttBroker", "SyncSource", "ContextSpace", "DataModel", "Endpoint", "Subscription"] },
    { segment: "access", kinds: ["Role", "RoleBinding", "ServiceAccount", "Policy", "Group"] },
    { segment: "publish", kinds: ["Endpoint", "CkanInstance", "DataModel"] },
    { segment: "apps", kinds: ["App", "Dashboard", "Layer"] },
    { segment: "operate", kinds: ["Pipeline", "SyncSource"] },
    { segment: "knowledge", kinds: ["KnowledgeSource", "AssistantDeployment"] },
  ],
  ceiling: {
    segments: ["data-read", "knowledge", "context", "pipeline", "apps"],
    writes: false,
    maxDays: 30,
    // The banskabystrica story (T-6302): new Endpoints and Pipelines, App edits, no Endpoint edit.
    kinds: [
      { kind: "Endpoint", verbs: ["read", "create"] },
      { kind: "Pipeline", verbs: ["read", "create", "run"] },
      { kind: "App", verbs: ["read", "edit"] },
      { kind: "ContextSpace", verbs: ["read"] },
      { kind: "DataModel", verbs: ["read"] },
    ],
  },
  projects: [
    { project: "doprava", spaces: ["parking", "traffic"], endpoints: ["bike-counters", "parking-lots"], pipelines: ["load-counters"], allowed: true },
    { project: "ovzdusie", spaces: ["air"], endpoints: ["air-quality"], pipelines: [], allowed: true },
    // Read by the person, outside their DelegationPolicy (T-6307): greyed before Create.
    { project: "sharing", spaces: ["bikes"], endpoints: ["bike-share"], pipelines: [], allowed: false },
  ],
  clients: [
    { id: "mcp-mobile", isBoundary: false },
    { id: "mcp-hub", isBoundary: false },
    { id: "mcp-local", isBoundary: false },
  ],
};

// A member (ADR-N-067, T-6413): no project, two Endpoints of helsinki shared with her, read only.
const MEMBER = {
  ...OPTIONS,
  ceiling: { segments: ["data-read"], writes: false, maxDays: 30, kinds: [] },
  projects: [{ project: "helsinki", spaces: [], endpoints: ["air-quality", "traffic-counts"], pipelines: [], allowed: true, organizationData: true }],
};
const CLAUDE_AI = "https://claude.ai/oauth/mcp-oauth-client-metadata";
const CLAUDE_CODE = "https://claude.ai/oauth/claude-code-client-metadata";
const LISTED = "DelegationPolicy claude-only does not allow the client";
// A ceiling listing only the two claude.ai documents (T-6309): the realm clients greyed with why.
const CLAUDE_ONLY = {
  ...OPTIONS,
  clients: [
    ...OPTIONS.clients.map((one) => ({ ...one, refused: `${LISTED} ${one.id}` })),
    { id: CLAUDE_CODE, isBoundary: false },
    { id: CLAUDE_AI, isBoundary: true },
  ],
};

interface Stub {
  refuse?: boolean;
  refuseKinds?: boolean;
  options?: typeof CLAUDE_ONLY | typeof MEMBER;
  member?: boolean;
  refuseClient?: boolean;
}

async function stubApi(page: Page, stub: Stub = {}) {
  const requests: { method: string; path: string; body?: unknown }[] = [];
  let items: Record<string, unknown>[] = [];
  await page.route("**/api/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: status >= 400 ? "application/problem+json" : "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path === "/api/v1/projects") return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ProjectList", items: stub.member ? [] : [{ name: "doprava" }] });
    if (path.startsWith("/api/v1/connectors")) {
      const body = request.method() === "POST" ? request.postDataJSON() : undefined;
      requests.push({ method: request.method(), path, body });
      if (path === "/api/v1/connectors/options") return json(stub.options ?? OPTIONS);
      if (path === "/api/v1/connectors" && request.method() === "GET") return json({ items });
      if (path === "/api/v1/connectors" && request.method() === "POST") {
        if (stub.refuseKinds) {
          return json(
            { type: "https://docs.joinedcontext.com/problems/forbidden", title: "Forbidden", status: 403, detail: "kinds[2].verbs: DelegationPolicy creators does not let a connector create Endpoint in project doprava", errors: ["kinds[2].verbs"] },
            403,
          );
        }
        if (stub.refuseClient) {
          return json(
            { type: "https://docs.joinedcontext.com/problems/forbidden", title: "Forbidden", status: 403, detail: `allowedClients[0]: ${LISTED} ${CLAUDE_CODE}`, errors: ["allowedClients[0]"] },
            403,
          );
        }
        if (stub.refuse) {
          return json(
            { type: "https://docs.joinedcontext.com/problems/unprocessable", title: "Unprocessable Content", status: 422, detail: "project doprava has no Endpoint parking-lots you may read", errors: ["scope[0].endpoint"] },
            422,
          );
        }
        const made = {
          id: ID,
          name: body.name,
          owner: IDENTITY.subject,
          segments: body.segments,
          scope: body.scope,
          kinds: body.kinds,
          writes: false,
          expiresAt: body.expiresAt,
          allowedClients: body.allowedClients,
          clientIsBoundary: false,
          state: "active",
          createdAt: "2026-10-28T12:00:00Z",
          lastUsedAt: null,
          url: URL_OF,
        };
        items = [made];
        return json(made, 201);
      }
      if (path.endsWith("/activity")) {
        return json({
          items: [
            { at: "2026-10-28T12:31:00Z", subject: IDENTITY.subject, client: "mcp-local", tool: "jc_resource_list", target: "doprava/Endpoint", verdict: "refused: a list in a narrowed project", durationMs: 3 },
            { at: "2026-10-28T12:30:00Z", subject: IDENTITY.subject, client: "mcp-local", tool: "jc_resource_get", target: "doprava/Endpoint/bike-counters", verdict: "allowed", durationMs: 12 },
          ],
        });
      }
      if (request.method() === "DELETE") {
        items = items.map((one) => ({ ...one, state: "revoked" }));
        return route.fulfill({ status: 204 });
      }
    }
    return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
  });
  return requests;
}

async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.JC_EVIDENCE_DIR;
  if (!dir) return;
  mkdirSync(dir, { recursive: true });
  await page.screenshot({ path: join(dir, `${name}.png`), fullPage: true });
}

for (const width of [1440, 400]) {
  test.describe(`my AI connectors at ${width} px`, () => {
    test.use({ viewport: { width, height: 900 } });

    test("a person creates a connector from the tree, copies its setup and revokes it", async ({ page, context }) => {
      await context.grantPermissions(["clipboard-read", "clipboard-write"]);
      const requests = await stubApi(page);
      await page.goto("/ai-connectors?lang=en");
      await expect(page.getByText("No AI connectors yet")).toBeVisible();
      await shot(page, `ai-connectors-empty-${width}`);

      await page.getByRole("button", { name: "New AI connector" }).first().click();
      const dialog = page.getByRole("dialog");
      await dialog.getByRole("textbox", { name: /Name/ }).fill("doprava-in-claude");
      await dialog.getByRole("checkbox", { name: "bike-counters" }).check();
      await dialog.getByRole("checkbox", { name: /Claude Code, VS Code/ }).check();
      await dialog.getByRole("checkbox", { name: "Knowledge" }).check();
      await dialog.getByRole("textbox", { name: /Name/ }).scrollIntoViewIfNeeded();
      await shot(page, `ai-connectors-form-${width}`);
      const outside = dialog.getByRole("checkbox", { name: /sharing: the whole project/ });
      await expect(outside).toHaveAttribute("aria-disabled", "true");
      await expect(dialog.getByRole("checkbox", { name: /bike-share/ })).toHaveAttribute("aria-disabled", "true");
      await expect(dialog.locator("p", { hasText: /Your organization does not let a connector reach this project yet/ })).toBeVisible();
      await dialog.getByRole("checkbox", { name: /bike-share/ }).scrollIntoViewIfNeeded();
      await shot(page, `ai-connectors-greyed-${width}`);
      await dialog.getByRole("button", { name: "Create" }).click();

      const card = page.getByRole("region", { name: "doprava-in-claude" });
      await expect(card.getByText(`claude mcp add --transport http doprava-in-claude ${URL_OF}`)).toBeVisible();
      await card.getByRole("button", { name: "Copy: Claude Code" }).click();
      expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(`claude mcp add --transport http doprava-in-claude ${URL_OF}`);
      const created = requests.find((one) => one.method === "POST")?.body as { segments: string[]; scope: unknown; allowedClients: string[] };
      expect(created.segments).toContain("knowledge");
      expect(created.scope).toEqual([{ project: "doprava", endpoint: "bike-counters", access: "read" }]);
      expect(created.allowedClients).toEqual(["mcp-local"]);
      await card.getByRole("button", { name: "Activity" }).click();
      await expect(card.getByRole("table", { name: "Calls through doprava-in-claude" })).toBeVisible();
      await shot(page, `ai-connectors-filled-${width}`);

      await card.getByRole("button", { name: "Revoke AI connector doprava-in-claude" }).click();
      await expect(card.getByRole("alert")).toContainText("The next call through it is refused");
      await card.getByRole("button", { name: "Revoke now" }).click();
      await expect(card.getByText("Revoked", { exact: true })).toBeVisible();
      expect(requests.some((one) => one.method === "DELETE" && one.path === `/api/v1/connectors/${ID}`)).toBe(true);
      await expect(page.locator("body")).not.toContainText(/eyJ|Bearer /);
      await shot(page, `ai-connectors-revoked-${width}`);
    });

    test("an empty form is refused in place, and a server refusal names its field", async ({ page }) => {
      await stubApi(page, { refuse: true });
      await page.goto("/ai-connectors?lang=en");
      await page.getByRole("button", { name: "New AI connector" }).first().click();
      const dialog = page.getByRole("dialog");
      await dialog.getByRole("button", { name: "Create" }).click();
      await expect(dialog.getByText(/Give a name of lowercase letters/)).toBeVisible();
      await shot(page, `ai-connectors-error-${width}`);

      await dialog.getByRole("textbox", { name: /Name/ }).fill("parking");
      await dialog.getByRole("checkbox", { name: "parking-lots" }).check();
      await dialog.getByRole("button", { name: "Create" }).click();
      await expect(dialog.getByText("project doprava has no Endpoint parking-lots you may read")).toBeVisible();
      await dialog.getByText("project doprava has no Endpoint parking-lots you may read").scrollIntoViewIfNeeded();
      await expect(dialog.getByRole("checkbox", { name: /^Access/ })).toHaveAttribute("aria-disabled", "true");
      await shot(page, `ai-connectors-refusal-${width}`);
    });

    test("a person ticks create on Endpoint and Pipeline and edit on App, read on the rest (T-6302)", async ({ page }) => {
      const requests = await stubApi(page);
      await page.goto("/ai-connectors?lang=en");
      await page.getByRole("button", { name: "New AI connector" }).first().click();
      const dialog = page.getByRole("dialog");
      await dialog.getByRole("textbox", { name: /Name/ }).fill("banskabystrica-builder");
      await dialog.getByRole("checkbox", { name: /^doprava: the whole project/ }).check();
      await shot(page, `ai-connectors-verbs-empty-${width}`);
      for (const segment of ["Pipelines", "Apps"]) await dialog.getByRole("checkbox", { name: new RegExp(`^${segment}`) }).first().check();
      const verbs = (kind: string) => dialog.getByRole("group", { name: `What it may do on ${kind}` });
      await expect(verbs("Endpoints").getByRole("checkbox", { name: /edit/ })).toHaveAttribute("aria-disabled", "true");
      await shot(page, `ai-connectors-verbs-greyed-${width}`);
      await verbs("Endpoints").getByRole("checkbox", { name: /create/ }).check();
      await verbs("Pipelines").getByRole("checkbox", { name: /create/ }).check();
      await verbs("Applications").getByRole("checkbox", { name: /edit/ }).check();
      await shot(page, `ai-connectors-verbs-filled-${width}`);
      await dialog.getByRole("button", { name: "Create" }).click();
      await expect(page.getByRole("region", { name: "banskabystrica-builder" })).toContainText("Endpoint: create");
      const sent = requests.find((one) => one.method === "POST")?.body as { kinds: { kind: string; verbs: string[] }[] };
      const of = (kind: string) => sent.kinds.find((one) => one.kind === kind)?.verbs;
      expect([of("Endpoint"), of("Pipeline"), of("App"), of("DataModel")]).toEqual([["read", "create"], ["read", "create"], ["read", "edit"], ["read"]]);
    });

    test("a verb the ceiling withholds is refused beside the verbs", async ({ page }) => {
      await stubApi(page, { refuseKinds: true });
      await page.goto("/ai-connectors?lang=en");
      await page.getByRole("button", { name: "New AI connector" }).first().click();
      const dialog = page.getByRole("dialog");
      await dialog.getByRole("textbox", { name: /Name/ }).fill("too-wide");
      await dialog.getByRole("checkbox", { name: /^doprava: the whole project/ }).check();
      await dialog.getByRole("checkbox", { name: /^Context/ }).first().check();
      await dialog.getByRole("group", { name: "What it may do on Endpoints" }).getByRole("checkbox", { name: /create/ }).check();
      await dialog.getByRole("button", { name: "Create" }).click();
      const refusal = dialog.getByText("does not let a connector create Endpoint in project doprava");
      await expect(refusal).toBeVisible();
      await refusal.scrollIntoViewIfNeeded();
      await shot(page, `ai-connectors-verbs-refusal-${width}`);
    });

    test("under a ceiling of claude.ai clients the dialog offers them by name and greys the rest (T-6309)", async ({ page }) => {
      const requests = await stubApi(page, { options: CLAUDE_ONLY });
      await page.goto("/ai-connectors?lang=en");
      await page.getByRole("button", { name: "New AI connector" }).first().click();
      const dialog = page.getByRole("dialog");
      await dialog.getByRole("textbox", { name: /Name/ }).fill("air-in-claude");
      await dialog.getByRole("checkbox", { name: "bike-counters" }).check();
      await expect(dialog.getByRole("checkbox", { name: /The organization's MCP hub/ })).toHaveAttribute("aria-disabled", "true");
      await expect(dialog.getByText(/only the clients its policy lists/).first()).toBeVisible();
      await dialog.getByRole("checkbox", { name: /^claude\.ai/ }).check();
      await dialog.getByRole("checkbox", { name: /The organization's MCP hub/ }).scrollIntoViewIfNeeded();
      await shot(page, `ai-connectors-clients-${width}`);
      await dialog.getByRole("button", { name: "Create" }).click();
      await expect.poll(() => requests.find((one) => one.method === "POST")?.body).toMatchObject({ allowedClients: [CLAUDE_AI] });
    });

    test("a client the ceiling refused on create is named beside the clients (T-6309)", async ({ page }) => {
      await stubApi(page, { options: CLAUDE_ONLY, refuseClient: true });
      await page.goto("/ai-connectors?lang=en");
      await page.getByRole("button", { name: "New AI connector" }).first().click();
      const dialog = page.getByRole("dialog");
      await dialog.getByRole("textbox", { name: /Name/ }).fill("air-in-claude");
      await dialog.getByRole("checkbox", { name: "bike-counters" }).check();
      await dialog.getByRole("checkbox", { name: /^Claude Code(?!,)/ }).check();
      await dialog.getByRole("button", { name: "Create" }).click();
      const why = dialog.getByRole("alert").getByText("Your organization allows only these clients: claude.ai. Untick Claude Code.");
      await expect(why).toBeVisible();
      await expect(dialog.getByRole("checkbox", { name: /^Claude Code\s*not allowed/ })).toBeChecked();
      // Under its row, above the client after it, never at the dialog's foot.
      const under = await why.boundingBox();
      const next = await dialog.getByRole("checkbox", { name: /^claude\.ai/ }).boundingBox();
      expect(under!.y).toBeLessThan(next!.y);
      await shot(page, `ai-connectors-client-refusal-${width}`);
      await expect(dialog.getByRole("button", { name: "Create" })).toHaveAttribute("aria-disabled", "true");
      await dialog.getByRole("checkbox", { name: /^Claude Code\s*not allowed/ }).uncheck();
      await expect(why).toHaveCount(0);
      await expect(dialog.getByRole("button", { name: "Create" })).not.toHaveAttribute("aria-disabled", "true");
    });

    test("a member builds a read-only connector over the datasets shared with her (T-6413)", async ({ page }) => {
      const requests = await stubApi(page, { options: MEMBER, member: true });
      await page.goto("/ai-connectors?lang=en");
      await page.getByRole("button", { name: "New AI connector" }).first().click();
      const dialog = page.getByRole("dialog");
      const name = dialog.getByRole("textbox", { name: /Name/ });
      await expect(name).toBeVisible();
      // The dialog opens at its top: the first field is whole under the header (T-6424).
      expect(await name.evaluate((input) => (input.closest(".overflow-y-auto") as HTMLElement).scrollTop)).toBe(0);
      await name.fill("shared-in-claude");
      await expect(dialog.getByText("helsinki: organization data, read")).toBeVisible();
      await expect(dialog.getByRole("checkbox", { name: /reads the Endpoints shared with you/ })).toBeChecked();
      await expect(dialog.getByRole("checkbox", { name: /^Pipelines/ })).toHaveCount(0);
      await expect(dialog.getByText(/other presets need your organization's approval/)).toBeVisible();
      await expect(dialog.getByRole("checkbox", { name: /the whole project/ })).toHaveCount(0);
      await dialog.getByRole("checkbox", { name: "traffic-counts" }).check();
      await expect(dialog.getByRole("combobox", { name: /Access for traffic-counts/ })).toHaveCount(0);
      await dialog.getByRole("checkbox", { name: /Claude Code, VS Code/ }).check();
      await expect(dialog.getByText(/other local programs can name it too/)).toHaveCount(1);
      await name.evaluate((input) => (input.closest(".overflow-y-auto") as HTMLElement).scrollTo(0, 0));
      await shot(page, `ai-connectors-member-form-${width}`);
      await dialog.getByRole("button", { name: "Create" }).click();
      const card = page.getByRole("region", { name: "shared-in-claude" });
      await expect(card).toBeVisible();
      await expect(card.getByText(/organization data, read/)).toBeVisible();
      await expect(card.getByRole("button", { name: /Connect this tab/ })).toHaveCount(0);
      const created = requests.find((one) => one.method === "POST")?.body as { segments: string[]; scope: unknown; writes?: boolean };
      expect(created.scope).toEqual([{ project: "helsinki", endpoint: "traffic-counts", access: "read" }]);
      expect(created.segments).toEqual(["data-read"]);
      await shot(page, `ai-connectors-member-created-${width}`);
    });

    test("a member with nothing shared reads what to do next (T-6413)", async ({ page }) => {
      await stubApi(page, { options: { ...MEMBER, projects: [] }, member: true });
      await page.goto("/ai-connectors?lang=en");
      await page.getByRole("button", { name: "New AI connector" }).first().click();
      const dialog = page.getByRole("dialog");
      await expect(dialog.getByText(/nothing is shared with you yet/)).toBeVisible();
      await expect(dialog.getByRole("button", { name: "Create" })).toHaveAttribute("aria-disabled", "true");
      await expect(dialog.getByRole("textbox", { name: /Name/ })).toHaveCount(0);
      await shot(page, `ai-connectors-member-empty-${width}`);
    });

    test("the old connectors address opens My AI connectors (T-5970)", async ({ page }) => {
      await stubApi(page);
      await page.goto("/connectors?lang=en");
      await expect(page).toHaveURL(/\/ai-connectors/);
      await expect(page.getByRole("heading", { name: "My AI connectors", level: 1 })).toBeVisible();
    });
  });
}
