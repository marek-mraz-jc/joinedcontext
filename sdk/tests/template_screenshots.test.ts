/**
 * T-3306 (AP-141): every App template carries a screenshot at 1440 and at 375, taken of the
 * sources it has now. A sample whose files change while its screenshots stay fails here;
 * `JC_WRITE_SCREENSHOTS=1 pnpm exec playwright test e2e/template_demos.spec.ts` takes them again.
 */
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { sourceDigest } from "../e2e/sampleDigest";

const SAMPLES = join(__dirname, "..", "samples");
const names = readdirSync(SAMPLES, { withFileTypes: true })
  .filter((entry) => entry.isDirectory() && existsSync(join(SAMPLES, entry.name, "sample.json")))
  .map((entry) => entry.name);
const recorded = JSON.parse(readFileSync(join(SAMPLES, "screenshots.json"), "utf8")) as Record<string, string>;

describe("the App templates' screenshots (T-3306)", () => {
  it.each(names)("%s has both screenshots, taken of its sources as they are", (name) => {
    for (const width of [1440, 375]) {
      const png = join(SAMPLES, name, `screenshot-${width}.png`);
      expect(existsSync(png), `${png} is missing`).toBe(true);
      // A PNG, not an empty or a stray file.
      expect(readFileSync(png).subarray(1, 4).toString()).toBe("PNG");
    }
    expect(recorded[name], `${name}: its sources changed since its screenshots were taken`).toBe(sourceDigest(join(SAMPLES, name)));
  });

  it("records no template that no longer exists", () => {
    expect(Object.keys(recorded).sort()).toEqual([...names].sort());
  });
});
