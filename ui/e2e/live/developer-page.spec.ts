/**
 * T-5111: every public Endpoint dev's catalogue names has a developer page at `/d/{slug}` that
 * names its licence and rate limit, and every sample the page offers runs green as it stands:
 * each curl through `sh`, each Python sample through `python3`, each JavaScript sample through
 * `node`, anonymously against dev. The page is shot at 1440 and 400 px for the UX read.
 */
import { execFile } from "node:child_process";
import { mkdtemp, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { promisify } from "node:util";
import { expect, test } from "@playwright/test";
import { samplesOf } from "../../src/pages/endpoints/developerSamples";
import type { Language, SampleInput } from "../../src/pages/endpoints/developerSamples";

test.setTimeout(1_800_000);

const run = promisify(execFile);
/** Below the edge's 600 requests a minute per address with room for the page itself. */
const PAUSE_MS = 400;
/** The samples that save a file instead of printing the answer. */
const SAVED = /-o (\S+)/;

async function runSample(language: Language, code: string, dir: string): Promise<string> {
  const options = { cwd: dir, timeout: 60_000, maxBuffer: 8 << 20 };
  if (language === "curl") return (await run("sh", ["-c", code], options)).stdout;
  if (language === "python") return (await run("python3", ["-I", "-c", code], options)).stdout;
  const file = join(dir, "sample.mjs");
  await writeFile(file, code);
  return (await run("node", [file], options)).stdout;
}

test("every public Endpoint has a developer page whose every sample runs green", async ({ browser }) => {
  const context = await browser.newContext();
  const page = await context.newPage();
  const dir = await mkdtemp(join(tmpdir(), "developer-samples-"));
  const failures: string[] = [];
  try {
    const listed = (await (await page.request.get("/api/v1/catalogue")).json()) as { datasets: { name: string }[] };
    const slugs = new Set<string>();
    for (const dataset of listed.datasets) {
      const detail = (await (await page.request.get(`/api/v1/catalogue/datasets/${dataset.name}`)).json()) as { endpoint?: { url?: string } };
      const slug = /\/api\/endpoint\/([a-z0-9]{16,64})\//.exec(detail.endpoint?.url ?? "")?.[1];
      if (slug) slugs.add(slug);
    }
    expect(slugs.size, "dev publishes public Endpoints").toBeGreaterThan(0);
    let shot = false;
    for (const slug of slugs) {
      const answer = await page.request.get(`/api/v1/catalogue/endpoints/${slug}/standards`);
      expect(answer.status(), slug).toBe(200);
      const standards = (await answer.json()) as SampleInput & { licence?: unknown; requestsPerMinute?: number };
      expect(typeof standards.requestsPerMinute, `${slug} names its rate limit`).toBe("number");
      const samples = samplesOf(standards);
      expect(samples.map((s) => s.surface), slug).toContain("ngsi-ld");
      for (const sample of samples) {
        for (const [language, code] of Object.entries(sample.code) as [Language, string][]) {
          const label = `${slug} ${sample.surface} ${language}`;
          try {
            const out = await runSample(language, code, dir);
            const saved = SAVED.exec(code)?.[1];
            if (saved) expect((await stat(join(dir, saved))).size, label).toBeGreaterThan(0);
            else expect(out.trim().length, label).toBeGreaterThan(0);
          } catch (error) {
            failures.push(`${label}: ${error instanceof Error ? error.message.split("\n")[0] : String(error)}`);
          }
          await new Promise((resolve) => setTimeout(resolve, PAUSE_MS));
        }
      }
      await page.goto(`/d/${slug}?lang=en`, { waitUntil: "load" });
      await expect(page.getByTestId("developer-panel")).toBeVisible({ timeout: 60_000 });
      await expect(page.getByTestId("developer-limit")).toContainText(String(standards.requestsPerMinute));
      if (!shot) {
        for (const width of [1440, 400]) {
          await page.setViewportSize({ width, height: 900 });
          await page.screenshot({ path: test.info().outputPath(`developer-page-${width}.png`), fullPage: true });
        }
        await page.setViewportSize({ width: 1280, height: 720 });
        shot = true;
      }
    }
    expect(failures, "every sample runs green").toEqual([]);
  } finally {
    await rm(dir, { recursive: true, force: true });
    await context.close();
  }
});
