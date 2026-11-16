/**
 * Every page of a project's main navigation, reached the way a person does: the administrator signs
 * in, opens the project, clicks the link in the Main navigation, and the page draws its heading and
 * the imported data it lists (catalogue.json `nav`, `heading`, `text`). No page answers an error and
 * none shows the shell's "did not load" notice. 1440 and 400 shots of each.
 */
import { readFileSync } from "node:fs";
import { expect, ready, shoot, test } from "./fixtures";

type Journey = { id: string; file: string; title: string; nav?: string; heading?: string; text?: string; pages: string[] };
const catalogue = JSON.parse(readFileSync(new URL("./catalogue.json", import.meta.url), "utf8")) as { journeys: Journey[] };

for (const j of catalogue.journeys.filter((x) => x.file === "pages.spec.ts")) {
  test(`${j.id} ${j.title}`, async ({ as }, info) => {
    const page = await as("admin", "/projects/helsinki/home");
    const errors: string[] = [];
    page.on("response", (r) => {
      if (r.status() >= 500) errors.push(`${r.status()} ${new URL(r.url()).pathname}`);
    });
    const link = ready(page).getByRole("link", { name: j.nav, exact: true }).first();
    await link.click();
    await expect(page).toHaveURL((u) => u.pathname === j.pages[0]);
    const heading = page.getByRole("main").getByRole("heading", { level: 1 }).first();
    await expect(heading).toBeVisible();
    if (j.heading) await expect(heading).toHaveText(j.heading);
    if (j.text) await expect(page.getByRole("main").getByText(j.text).first()).toBeVisible();
    await expect(page.getByText("The Portal did not load")).toBeHidden();
    await shoot(page, info, "page");
    expect(errors).toEqual([]);
  });
}
