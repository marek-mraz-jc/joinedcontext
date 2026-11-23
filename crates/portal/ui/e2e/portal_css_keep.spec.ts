import { readFileSync } from "node:fs";
import { expect, test } from "@playwright/test";

// T-7745: a portal's own stylesheet may restyle its open data portal, never take the licence and
// publisher lines out of sight. Each stylesheet below is one jc-core accepts, as jc-core writes it
// (vendor/.../jc-core/src/kinds/stylesheet_keep.json, kept current by its unit test), drawn over
// the open data portal's own theme (with the colour tokens a page's head sets) and the dataset
// page's about list and publisher card.

const root = new URL("../../../../", import.meta.url);
const theme = readFileSync(new URL("crates/dataroom/web/assets/public__jc-theme.css", root), "utf8");
const corpus = JSON.parse(readFileSync(new URL("vendor/joinedcontext-platform/crates/jc-core/src/kinds/stylesheet_keep.json", root), "utf8")) as { css: string; scoped: string }[];

const page = (sheet: string) => `<!doctype html><html lang="sk"><head><meta charset="utf-8">
<style>:root { --jc-primary: #0b5394; --jc-background: #ffffff; --jc-text: #0f172a; }</style>
<style>${theme}</style><style>${sheet}</style></head>
<body class="jc-portal-css jc-portal-test"><main class="container">
<section class="jc-publisher module" data-jc-keep><div class="jc-publisher-text"><a href="#" class="jc-publisher-name">Banskobystrický samosprávny kraj</a></div></section>
<section class="jc-about"><h2>O dátach</h2><dl class="jc-about-list">
<dt data-jc-keep>Licencia</dt><dd data-jc-keep><a href="#">Creative Commons Attribution 4.0</a></dd>
<dt data-jc-keep>Vydavateľ</dt><dd data-jc-keep>Banskobystrický samosprávny kraj</dd>
<dt>Aktualizované</dt><dd>2026-11-23</dd></dl></section>
<p>Ďalší obsah stránky</p></main></body></html>`;

/** Every `[data-jc-keep]` element and why it is out of sight, empty when each one reads. */
async function hidden(sheet: string, browserPage: import("@playwright/test").Page): Promise<string[]> {
  await browserPage.setContent(page(sheet));
  const keep = browserPage.locator("[data-jc-keep]");
  const faults: string[] = [];
  for (let i = 0; i < (await keep.count()); i += 1) {
    const element = keep.nth(i);
    await element.scrollIntoViewIfNeeded();
    faults.push(
      ...(await element.evaluate((el) => {
        // Any CSS colour (oklab and color-mix included) as RGBA through a 1x1 canvas.
        const context = document.createElement("canvas").getContext("2d", { willReadFrequently: true })!;
        const rgba = (value: string) => {
          context.clearRect(0, 0, 1, 1);
          context.fillStyle = "#00000000";
          context.fillStyle = value;
          context.fillRect(0, 0, 1, 1);
          const [r = 0, g = 0, b = 0, a = 0] = context.getImageData(0, 0, 1, 1).data;
          return [r, g, b, a / 255];
        };
        const lum = ([r = 0, g = 0, b = 0]: number[]) =>
          [r, g, b].map((c) => (c / 255 <= 0.03928 ? c / 255 / 12.92 : ((c / 255 + 0.055) / 1.055) ** 2.4)).reduce((sum, c, k) => sum + c * [0.2126, 0.7152, 0.0722][k]!, 0);
        // What the text is drawn on: the nearest box with a background, the page's white at the end.
        const backdrop = (from: Element) => {
          for (let at: Element | null = from; at; at = at.parentElement) {
            const colour = rgba(getComputedStyle(at).backgroundColor);
            if ((colour[3] ?? 0) > 0.5) return colour;
          }
          return [255, 255, 255, 1];
        };
        const out: string[] = [];
        const name = `${el.tagName.toLowerCase()} "${(el.textContent ?? "").trim().slice(0, 24)}"`;
        const style = getComputedStyle(el);
        const box = el.getBoundingClientRect();
        if (box.width < 1 || box.height < 1) out.push(`${name}: no size`);
        if (parseFloat(style.fontSize) < 12) out.push(`${name}: text ${style.fontSize}`);
        if (style.visibility !== "visible") out.push(`${name}: ${style.visibility}`);
        for (let at: Element | null = el; at; at = at.parentElement) {
          const s = getComputedStyle(at);
          if (s.display === "none" || parseFloat(s.opacity) < 1) out.push(`${name}: hidden by ${at.tagName.toLowerCase()}`);
        }
        const colour = rgba(style.color);
        const back = backdrop(el);
        if ((colour[3] ?? 1) < 1) out.push(`${name}: see-through text`);
        const [hi, lo] = [lum(colour), lum(back)].sort((a, b) => b - a) as [number, number];
        if ((hi + 0.05) / (lo + 0.05) < 4.5) out.push(`${name}: contrast ${style.color} on ${style.backgroundColor}`);
        const top = document.elementFromPoint(box.left + box.width / 2, box.top + Math.min(box.height / 2, 8));
        if (!top || !(el === top || el.contains(top))) out.push(`${name}: covered by ${top?.tagName.toLowerCase() ?? "nothing"}`);
        return out;
      })),
    );
  }
  return faults;
}

for (const [width, height] of [
  [1440, 900],
  [400, 860],
] as const) {
  test(`the licence and publisher lines read under every accepted stylesheet at ${width} px`, async ({ page: browserPage }) => {
    await browserPage.setViewportSize({ width, height });
    expect(await hidden("", browserPage), "the theme alone").toEqual([]);
    expect(corpus.length).toBeGreaterThan(5);
    for (const { css, scoped } of corpus) {
      expect(scoped, css).toContain("body.jc-portal-test ");
      expect(await hidden(scoped, browserPage), css).toEqual([]);
    }
  });
}
