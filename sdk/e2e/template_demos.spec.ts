import { existsSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { expect, test } from "@playwright/test";
import { build } from "vite";
import { sourceDigest } from "./sampleDigest";

/**
 * The App templates' live demos (T-3306, AP-141): each sample is built into the demo bundle and
 * opened at `/templates/{name}/` with its fixtures. It must show its ready text, ask nothing of
 * any server, and look like its screenshots. `JC_WRITE_SCREENSHOTS=1` writes
 * `samples/{name}/screenshot-1440.png` and `-375.png` and the digest of the sources they show
 * into `samples/screenshots.json`; `tests/template_screenshots.test.ts` fails when a sample's
 * sources change and its screenshots do not.
 */
const HERE = dirname(fileURLToPath(import.meta.url));
const SAMPLES = join(HERE, "..", "samples");
const ORIGIN = "http://demo.test";
const files: Record<string, Buffer> = {};

test.beforeAll(async () => {
  test.setTimeout(240_000);
  await build({ configFile: join(HERE, "..", "vite.demos.config.ts"), logLevel: "silent" });
  const out = join(HERE, "..", "dist", "demos");
  const walk = (dir: string, prefix: string): void => {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      if (entry.isDirectory()) walk(join(dir, entry.name), `${prefix}${entry.name}/`);
      else files[`/templates/${prefix}${entry.name}`] = readFileSync(join(dir, entry.name));
    }
  };
  walk(out, "");
});

const names = readdirSync(SAMPLES, { withFileTypes: true })
  .filter((entry) => entry.isDirectory() && existsSync(join(SAMPLES, entry.name, "sample.json")))
  .map((entry) => entry.name)
  .sort();

for (const name of names) {
  test(`the ${name} demo runs on its fixtures alone`, async ({ page }) => {
    const elsewhere: string[] = [];
    const problems: string[] = [];
    page.on("pageerror", (error) => problems.push(error.message));
    await page.route("**/*", (route) => {
      const url = new URL(route.request().url());
      if (url.origin !== ORIGIN) {
        elsewhere.push(url.href);
        return route.abort();
      }
      const body = files[url.pathname] ?? (url.pathname.startsWith(`/templates/${name}`) ? files["/templates/index.html"] : undefined);
      if (body === undefined) return route.fulfill({ status: 404, body: "" });
      const type = url.pathname.endsWith(".css") ? "text/css" : url.pathname.endsWith(".js") ? "text/javascript" : "text/html";
      return route.fulfill({ contentType: type, body });
    });
    const ready = (JSON.parse(readFileSync(join(SAMPLES, name, "sample.json"), "utf8")) as { ready: string }).ready;
    const shots: Record<string, Buffer> = {};
    for (const width of [1440, 375]) {
      await page.setViewportSize({ width, height: width > 400 ? 900 : 760 });
      await page.goto(`${ORIGIN}/templates/${name}/`);
      await expect(page.getByText(ready).first()).toBeVisible({ timeout: 30_000 });
      await page.waitForTimeout(800);
      shots[width] = await page.screenshot();
    }
    expect(problems).toEqual([]);
    expect(elsewhere, "a demo asks no server").toEqual([]);
    if (process.env.JC_WRITE_SCREENSHOTS === "1") {
      for (const [width, png] of Object.entries(shots)) writeFileSync(join(SAMPLES, name, `screenshot-${width}.png`), png);
      const record = join(SAMPLES, "screenshots.json");
      const digests = existsSync(record) ? (JSON.parse(readFileSync(record, "utf8")) as Record<string, string>) : {};
      digests[name] = sourceDigest(join(SAMPLES, name));
      writeFileSync(record, JSON.stringify(Object.fromEntries(Object.entries(digests).sort()), null, 2) + "\n");
    }
  });
}
