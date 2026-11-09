// Shoots the reference Grafana 12.4.14 (anonymous Viewer, the copies grafana-copies.mjs writes,
// provisioned from files) the way the Portal's dashboard e2e shoots the Portal: the same absolute
// range per dashboard, timezone Europe/Bratislava, 1440 and 400 px, Grafana's light theme. Writes
// a shot per dashboard and width, shots of the time and refresh pickers and the k6 variable, and
// the text Grafana draws in each panel (legend values, stat and bar gauge numbers, table cells) to
// tests/fixtures/grafana-reference/panels.json, which the vitests compare value by value.
//
//   GRAFANA=http://localhost:42161 SHOTS=<dir> node scripts/grafana-reference.mjs
/* global document */
import { mkdirSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "@playwright/test";
import { rangeOf } from "./panel-frames.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const GRAFANA = process.env.GRAFANA ?? "http://localhost:42161";
const SHOTS = process.env.SHOTS;
if (!SHOTS) throw new Error("set SHOTS to the evidence directory");
const DASHBOARDS = join(here, "../tests/fixtures/dashboards");
const OUT = join(here, "../tests/fixtures/grafana-reference");
mkdirSync(SHOTS, { recursive: true });
mkdirSync(OUT, { recursive: true });

const WIDTHS = { 1440: 2600, 400: 9000 };

/** Crops the empty page below the last panel. */
function trim(path) {
  try {
    execFileSync("magick", [path, "-gravity", "South", "-background", "white", "-splice", "0x1", "-trim", "+repage", path]);
  } catch {
    // An untrimmed shot is still a shot.
  }
}

const browser = await chromium.launch();
const panels = {};
const files = readdirSync(DASHBOARDS).filter((f) => f.endsWith(".json"));
for (const file of files) {
  const dashboard = JSON.parse(readFileSync(join(DASHBOARDS, file), "utf8"));
  const stem = file.replace(/\.json$/, "");
  const { from, to } = rangeOf(dashboard);
  const time = dashboard.time ? `&from=${from}&to=${to}` : "";
  for (const [width, height] of Object.entries(WIDTHS)) {
    const context = await browser.newContext({ viewport: { width: Number(width), height }, timezoneId: "Europe/Bratislava", locale: "en-GB" });
    const page = await context.newPage();
    // The kiosk view: the panels without Grafana's own navigation, as the Portal page is.
    await page.goto(`${GRAFANA}/d/${dashboard.uid}?orgId=1${time}&theme=light&kiosk`, { waitUntil: "networkidle" });
    await page.waitForTimeout(2500);
    const shot = join(SHOTS, `grafana-${stem}-${width}.png`);
    await page.screenshot({ path: shot });
    trim(shot);
    if (width === "1440") {
      panels[dashboard.uid] = await page.evaluate(() =>
        Object.fromEntries(
          [...document.querySelectorAll("[data-viz-panel-key]")].map((element) => {
            const rows = [...element.querySelectorAll('[role="row"], tr')].map((row) =>
              [...row.querySelectorAll('[role="cell"], [role="columnheader"], [role="gridcell"], td, th')].map((cell) => cell.innerText.trim()),
            );
            return [element.getAttribute("data-viz-panel-key").replace("panel-", ""), { text: element.innerText.trim(), rows }];
          }),
        ),
      );
    }
    // The dashboard's own view with its controls: the picker, refresh and variables.
    await page.goto(`${GRAFANA}/d/${dashboard.uid}?orgId=1${time}&theme=light`, { waitUntil: "networkidle" });
    await page.waitForTimeout(1500);
    const controls = join(SHOTS, `grafana-${stem}-controls-${width}.png`);
    await page.screenshot({ path: controls, clip: { x: 0, y: 0, width: Number(width), height: Math.min(height, 700) } });
    if (stem === "context-broker" || stem === "k6-load") {
      const picker = page.getByTestId("data-testid TimePicker Open Button");
      if (await picker.count()) {
        await picker.first().click();
        await page.waitForTimeout(800);
        await page.screenshot({ path: join(SHOTS, `grafana-${stem}-timepicker-${width}.png`), clip: { x: 0, y: 0, width: Number(width), height: Math.min(height, 900) } });
        if (stem === "context-broker" && width === "1440") {
          const quick = await page.evaluate(() => [...document.querySelectorAll('[data-testid="data-testid TimePicker Overlay Content"] li, [role="dialog"] li, [role="dialog"] [role="option"]')].map((item) => item.innerText.trim()).filter(Boolean));
          writeFileSync(join(OUT, "time-picker.json"), `${JSON.stringify({ quickRanges: quick }, null, 1)}\n`);
        }
        await page.keyboard.press("Escape");
      }
      const refresh = page.getByTestId("data-testid RefreshPicker interval button");
      if (await refresh.count()) {
        await refresh.first().click();
        await page.waitForTimeout(600);
        if (stem === "context-broker" && width === "1440") {
          const intervals = await page.evaluate(() => [...document.querySelectorAll('[role="menu"] [role="menuitemradio"], [role="menu"] button, [role="listbox"] [role="option"]')].map((item) => item.innerText.trim()).filter(Boolean));
          writeFileSync(join(OUT, "refresh-picker.json"), `${JSON.stringify({ intervals }, null, 1)}\n`);
        }
        await page.screenshot({ path: join(SHOTS, `grafana-${stem}-refresh-${width}.png`), clip: { x: 0, y: 0, width: Number(width), height: Math.min(height, 700) } });
        await page.keyboard.press("Escape");
      }
    }
    await context.close();
  }
}
writeFileSync(join(OUT, "panels.json"), `${JSON.stringify(panels, null, 1)}\n`);
await browser.close();
