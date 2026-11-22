import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import type { Frame, Page } from "@playwright/test";

export interface PhoneHit {
  kind: "font" | "target" | "overflow" | "contrast";
  what: string;
  detail: string;
}

// A .cts file, so that crates/dataroom/web/e2e (CommonJS specs) imports it as well as this suite does;
// hence its own read of axe-core, as axe.ts finds its bundle through import.meta.
const axeSource = readFileSync(require.resolve("axe-core/axe.min.js"), "utf8");
const check = readFileSync(join(__dirname, "phoneCheck.browser.js"), "utf8").replace(/^\/\*[\s\S]*?\*\/\s*/, "").trim();

async function onScreen(frame: Frame): Promise<boolean> {
  for (let inner = frame; inner.parentFrame(); inner = inner.parentFrame()!) {
    const element = await inner.frameElement();
    const box = (await element.isVisible()) ? await element.boundingBox() : null;
    if (!box || box.width <= 1 || box.height <= 1) return false;
  }
  return true;
}

/** The mechanical 400 px check over the page and every frame it shows (T-7512); empty when clean. */
export async function phoneHits(page: Page): Promise<PhoneHit[]> {
  const width = page.viewportSize()?.width ?? Infinity;
  if (width > 430 || !(await page.evaluate(() => matchMedia("(pointer: coarse)").matches))) {
    throw new Error("the phone check needs a phone: viewport 400 px wide, hasTouch and isMobile");
  }
  const hits: PhoneHit[] = [];
  for (const frame of page.frames()) {
    if (!(await onScreen(frame))) continue;
    await frame.evaluate(axeSource);
    const own = frame === page.mainFrame() ? width : null;
    hits.push(...(await frame.evaluate<PhoneHit[]>(`(${check})(${own})`)));
  }
  return hits;
}

/**
 * Writes `<shot>.phone.json` beside a 400 px shot just taken of this page, bound to the picture by
 * its sha256. `tasks/uijudge` reads it: a hit, or a 400 px shot without its report, is a high finding.
 */
export async function writePhoneReport(page: Page, shot: string): Promise<PhoneHit[]> {
  const hits = await phoneHits(page);
  const sha256 = createHash("sha256").update(readFileSync(shot)).digest("hex");
  writeFileSync(shot.replace(/\.png$/i, ".phone.json"), `${JSON.stringify({ sha256, hits }, null, 1)}\n`);
  return hits;
}
