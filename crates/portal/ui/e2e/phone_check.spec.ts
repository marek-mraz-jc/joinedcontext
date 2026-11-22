import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { expect, test } from "@playwright/test";
import { phoneHits, writePhoneReport } from "./phoneCheck.cts";

// T-7512: two judges argued over ~10 px labels on a picture; the page itself answers that now.
test.use({ viewport: { width: 400, height: 800 }, hasTouch: true, isMobile: true });

const page400 = (body: string) =>
  `<!doctype html><html lang="en"><head><meta name="viewport" content="width=device-width"><style>
    body { margin: 0; font: 16px sans-serif; color: #111; background: #fff }
    button, input, select { min-width: 44px; min-height: 44px; font: inherit }
    .sr { position: absolute; width: 1px; height: 1px; overflow: hidden }
  </style></head><body>${body}</body></html>`;

test("one 10 px label and one 30 px button are the only two hits", async ({ page }) => {
  await page.setContent(page400(`
    <label style="display:block;font-size:10px">Tiny label</label>
    <button style="min-width:0;min-height:0;width:30px;height:30px;padding:0">X</button>`));
  expect(await phoneHits(page)).toEqual([
    { kind: "font", what: 'label "Tiny label"', detail: "10.0 px, under 12" },
    { kind: "target", what: 'button "X"', detail: "30x30 px, under 44x44" },
  ]);
});

test("a fixed page is clean: hidden words, an inline link and a labelled checkbox included", async ({ page }) => {
  await page.setContent(page400(`
    <label>Readable label <input value="value"></label>
    <span class="sr" style="font-size:9px"><span>For a screen reader</span></span>
    <p>Read <a href="#more">the source</a> first.</p>
    <label style="display:flex;min-height:44px"><input type="checkbox" style="min-width:0;min-height:0"> Show all</label>
    <div hidden style="font-size:8px">Closed</div>
    <button aria-label="Columns"><span style="font-size:0">C</span></button>`));
  expect(await phoneHits(page)).toEqual([]);
});

test("a page wider than the phone and grey on white are hits", async ({ page }) => {
  await page.setContent(page400(`<p style="color:#aaa">Pale words</p><div style="width:520px;height:8px"></div>`));
  expect((await phoneHits(page)).map((hit) => hit.kind)).toEqual(["overflow", "contrast"]);
});

test("a small field and a password are named without the password", async ({ page }) => {
  await page.setContent(page400(`<input aria-label="Password" type="password" value="hunter" style="font-size:9px">`));
  const hits = await phoneHits(page);
  expect(hits).toEqual([{ kind: "font", what: 'input "(password)"', detail: "9.0 px, under 12" }]);
});

test("a visible frame is measured, a sandboxed one too, and a hidden one is not", async ({ page }) => {
  const small = page400('<p style="font-size:9px">In a frame</p>').replace(/"/g, "&quot;");
  await page.setContent(page400(`<iframe sandbox="allow-scripts" style="width:380px;height:90px;border:0" srcdoc="${small}"></iframe>
    <iframe style="display:none" srcdoc="${small}"></iframe>`));
  await expect(page.frameLocator("iframe >> nth=0").getByText("In a frame")).toBeVisible();
  expect(await phoneHits(page)).toEqual([{ kind: "font", what: 'p "In a frame"', detail: "9.0 px, under 12" }]);
});

test("the report is bound to its shot", async ({ page }, info) => {
  await page.setContent(page400('<p style="font-size:10px">Tiny</p>'));
  const shot = info.outputPath("list-400.png");
  await page.screenshot({ path: shot });
  await writePhoneReport(page, shot);
  const report = JSON.parse(readFileSync(info.outputPath("list-400.phone.json"), "utf8"));
  expect(report.sha256).toBe(createHash("sha256").update(readFileSync(shot)).digest("hex"));
  expect(report.hits).toHaveLength(1);
});

test("a desktop page is refused, never reported clean", async ({ browser }) => {
  const desktop = await browser.newPage({ viewport: { width: 1440, height: 900 }, hasTouch: false, isMobile: false });
  await expect(phoneHits(desktop)).rejects.toThrow("needs a phone");
  await desktop.close();
});
