/**
 * T-3264: every Endpoint of the project answers from its own playground, for the steward and for
 * a viewer, with only the reads each may call. Read-only: the playground sends GETs alone.
 */
import { expect, test } from "@playwright/test";
import { signIn, STEWARD, VIEWER } from "./portal";

test.setTimeout(600_000);

const PROJECT = "helsinki";

for (const who of [STEWARD, VIEWER]) {
  test(`${who.user}: every endpoint of ${PROJECT} answers its playground's first read`, async ({ browser }) => {
    const session = await signIn(browser, who, `/projects/${PROJECT}/endpoints?lang=en`);
    try {
      const listed = await session.page.request.get(`/api/v1/projects/${PROJECT}/endpoints`);
      expect(listed.status()).toBe(200);
      const names = ((await listed.json()) as { items: { metadata: { name: string } }[] }).items.map((e) => e.metadata.name);
      expect(names.length).toBeGreaterThan(0);
      const answered: string[] = [];
      for (const name of names) {
        await session.page.goto(`/projects/${PROJECT}/endpoints/${name}?lang=en`);
        const playground = session.page.getByTestId("endpoint-playground");
        const nothing = session.page.getByText("Your access lets you call nothing through this endpoint.");
        await expect(playground.or(nothing)).toBeVisible({ timeout: 60_000 });
        if (await nothing.isVisible()) continue;
        const send = playground.getByRole("button", { name: "Send" });
        // A grant to one entity by id alone needs an id the journey does not know.
        if ((await send.getAttribute("aria-disabled")) === "true") continue;
        await send.click();
        await expect(playground.getByRole("status")).toHaveText(/The endpoint answered (200|204)\./, { timeout: 60_000 });
        await expect(playground.getByRole("tablist", { name: "The same call as code" })).toBeVisible();
        answered.push(name);
      }
      expect(answered.length, "some endpoint answers the person").toBeGreaterThan(0);
    } finally {
      await session.context.close();
    }
  });
}
