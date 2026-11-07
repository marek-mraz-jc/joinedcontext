/**
 * T-3263 (AP-141): the steward finds every App template on the Apps page, each with what it is
 * for and the data it reads, and "create from this" opens the builder with the template's purpose
 * written and named; each card's screenshot loads and a demo runs on its fixtures (T-3306). The
 * journey stops before Generate: a build spends model tokens and is the deployer's recorded run.
 */
import { expect, test } from "@playwright/test";
import { csrf, signIn, STEWARD } from "./portal";

test.setTimeout(180_000);

test("every template is offered with its purpose, and create from this opens the builder ready", async ({ browser }) => {
  const steward = await signIn(browser, STEWARD, "/projects/helsinki/apps?lang=en");
  try {
    const page = steward.page;
    const listed = (await (await page.request.get("/api/v1/app-templates")).json()) as { templates: { name: string; title: string }[] };
    expect(listed.templates.length).toBeGreaterThanOrEqual(7);
    const gallery = page.getByTestId("app-templates");
    await gallery.getByText("Start from a template").click();
    for (const template of listed.templates) {
      await expect(gallery.getByRole("heading", { name: template.title })).toBeVisible({ timeout: 60_000 });
    }
    const first = listed.templates[0];
    await gallery.getByRole("heading", { name: first.title }).locator("xpath=ancestor::li").getByRole("button", { name: "Create from this" }).click();
    const dialog = page.getByRole("dialog", { name: `Create from ${first.title}` });
    await expect(dialog.getByRole("textbox", { name: /What should the app do/ })).toHaveValue(new RegExp(`\\(template: ${first.name}\\)$`));
    await dialog.getByRole("button", { name: "Close" }).click();
    // T-3306: every card's picture loads, and a demo runs on its fixtures.
    for (const template of listed.templates) {
      for (const width of [1440, 375]) {
        const png = await page.request.get(`/api/v1/app-templates/${template.name}/screenshot/${width}`);
        expect(png.status(), `${template.name} at ${width}`).toBe(200);
        expect(png.headers()["content-type"]).toBe("image/png");
      }
    }
    const demo = await steward.context.newPage();
    const answer = await demo.goto(`/templates/${first.name}/`);
    expect(answer?.headers()["content-security-policy"]).toContain("connect-src 'none'");
    await expect(demo.locator("#root")).not.toBeEmpty({ timeout: 30_000 });
    await expect(demo.getByText("No such template.")).toHaveCount(0);
  } finally {
    await steward.context.close();
  }
});

/**
 * T-5617: "use as is" needs no model. The dialog asks the App's name and an Endpoint from the
 * person's own list; the journey stops before a repository is created and sends a name the
 * server refuses, so dev keeps nothing and the refusal names its field.
 */
test("use as is asks a name and a readable endpoint, and the server names a refused field", async ({ browser }) => {
  const steward = await signIn(browser, STEWARD, "/projects/helsinki/apps?lang=en");
  try {
    const page = steward.page;
    const listed = (await (await page.request.get("/api/v1/app-templates")).json()) as {
      templates: { name: string; title: string }[];
      modelAvailable: boolean;
    };
    expect(typeof listed.modelAvailable).toBe("boolean");
    const first = listed.templates[0];
    const gallery = page.getByTestId("app-templates");
    if (listed.modelAvailable) await gallery.getByText("Start from a template").click();
    await gallery.getByRole("heading", { name: first.title }).locator("xpath=ancestor::li").getByRole("button", { name: "Use as is" }).click();
    const dialog = page.getByRole("dialog", { name: `Use ${first.title} as is` });
    const endpoint = dialog.getByLabel(/Endpoint/);
    await expect(endpoint.locator("option")).not.toHaveCount(1, { timeout: 30_000 });
    const chosen = await endpoint.locator("option").nth(1).getAttribute("value");
    const refused = await page.request.post(`/api/v1/projects/helsinki/app-templates/${first.name}/apps`, {
      data: { name: "Not_A_Name", endpointName: chosen },
      headers: { "x-csrf-token": await csrf(steward.context), "content-type": "application/json" },
    });
    expect(refused.status()).toBe(422);
    expect(((await refused.json()) as { errors: string[] }).errors.join(" ")).toContain("name:");
    await dialog.getByRole("button", { name: "Close" }).click();
  } finally {
    await steward.context.close();
  }
});
