/**
 * T-3254 — "Use this data" on dev: the steward opens a public endpoint's type in the explorer, and
 * each snippet the dialog shows (curl, Python, JavaScript) is run here exactly as copied, from the
 * shell of the machine running the journey, against dev; each one returns rows. A public endpoint
 * needs no token, so no secret is involved. Read only.
 */
import { execFile } from "node:child_process";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { promisify } from "node:util";
import { expect, test } from "@playwright/test";
import { STEWARD, signIn } from "./portal";

const run = promisify(execFile);
test.setTimeout(240_000);

test("each snippet of a public endpoint's view runs as written against dev and returns rows", async ({ browser }) => {
  const { context, page } = await signIn(browser, STEWARD, "/projects/helsinki/explore?endpoint=helsinki-news&lang=en");
  const dir = await mkdtemp(join(tmpdir(), "use-this-data-"));
  try {
    const types = page.locator("#explore-type");
    await expect(types.locator("option[value]:not([value=''])").first()).toBeAttached({ timeout: 60_000 });
    const granted = await types
      .locator("option")
      .evaluateAll((options) =>
        options
          .filter((option) => option.getAttribute("value") && !option.textContent?.includes("not granted"))
          .map((option) => option.getAttribute("value") ?? ""),
      );
    expect(granted.length, "the public endpoint grants a type").toBeGreaterThan(0);
    await types.selectOption(granted[0]);
    await expect(page.locator("tbody tr").first()).toBeVisible({ timeout: 60_000 });

    await page.getByRole("button", { name: "Use this data" }).click();
    const dialog = page.getByRole("dialog", { name: "Use this data in a program" });
    await expect(dialog.getByText("This endpoint answers anyone: no token is needed.")).toBeVisible();

    for (const language of ["curl", "python", "javascript"]) {
      const code = (await dialog.locator(`[data-snippet="${language}"]`).textContent()) ?? "";
      expect(code).not.toContain("JC_TOKEN");
      let out: string;
      if (language === "curl") {
        out = (await run("sh", ["-c", code], { timeout: 60_000 })).stdout;
        expect(JSON.parse(out).length, "curl returned rows").toBeGreaterThan(0);
      } else {
        const file = join(dir, language === "python" ? "snippet.py" : "snippet.mjs");
        await writeFile(file, code);
        out = (await run(language === "python" ? "python3" : process.execPath, [file], { timeout: 60_000 })).stdout;
        const count = Number(out.split(" ")[0]);
        expect(count, `${language} printed: ${out}`).toBeGreaterThan(0);
      }
    }
  } finally {
    await rm(dir, { recursive: true, force: true });
    await context.close();
  }
});
