/**
 * T-2730 — every page and every form of the Portal on dev, at 1440 and 2560 px (or `WALK_WIDTHS`,
 * in `WALK_LANGS`), as the steward
 * and as the viewer (UI-01, UI-15, UI-16, UI-44, UI-84).
 *
 * The walker opens every route of the router with one real item for each name in it, and every
 * create and edit form at its own address. It is read-only: it never presses a write control, so
 * a form is judged as it opens. Per page and width it asks what `tests/pageChecks.ts` asks (one
 * h1, a purpose line, an empty state that says what to do, a hint on every field, no raw i18n
 * key, JSON or bare URN where words belong, nothing overflowing sideways, three form columns on a
 * big screen), plus what only a real browser on a real Portal can tell: no console error, no
 * request answered >= 400 the person should not meet, axe with no serious or critical
 * violation. For the viewer, every write control is disabled and says why (UI-44).
 *
 * A screenshot is saved per page, width and person; with `SHOTS_DIR` set it also goes there under
 * the UX photo audit's name, with a contact sheet (`shoot.ts`, T-4156), and `WALK_ONLY=<regex>`
 * walks only the addresses it matches. The findings are attached as JSON; what is
 * known and owed is in `walker.allow.json`, each entry naming its task, and only shrinks. The
 * mocked copy of the same checks runs in the fast suite (`tests/walker_pages.test.tsx`).
 */
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { expect, test } from "@playwright/test";
import type { Page, Response } from "@playwright/test";
import { OWN_ROUTES, unexcused } from "../../tests/pageChecks";
import type { Excused } from "../../tests/pageChecks";
import { STEWARD, VIEWER, afterSelfReload, hiddenSections, inHiddenSection, journeyMarkerOnPortalOnly, judgeOpened, openToWalk, signIn } from "./portal";
import type { Opened } from "./portal";
import { secretMasks, shot, writeIndex } from "./shoot";
import { Tab } from "./tab";
import { P, visits } from "./walk-routes";

const here = dirname(fileURLToPath(import.meta.url));
const NAMESPACES = Object.keys(JSON.parse(readFileSync(join(here, "../../src/locales/en.json"), "utf8")) as object);
const allow = JSON.parse(readFileSync(join(here, "walker.allow.json"), "utf8")) as { excused: Excused[] };

/** `WALK_WIDTHS=375,1440` and `WALK_LANGS=en,sk` narrow or widen the walk (T-3848: phone and Slovak). */
const WIDTHS = (process.env.WALK_WIDTHS ?? "1440,2560")
  .split(",")
  .map(Number)
  .filter((width) => width > 0)
  .map((width) => ({ width, height: width < 768 ? 812 : width <= 1440 ? 900 : 1300 }));
const LANGS = (process.env.WALK_LANGS ?? "en").split(",").filter(Boolean);
const ONLY = process.env.WALK_ONLY ? new RegExp(process.env.WALK_ONLY) : undefined;

/**
 * PF-61, T-2877, UI-75: what a person who does not administer the organization meets at the
 * cross-project Endpoints — both addresses tell them the Administration page is for
 * administrators, no tab and no menu entry offers it, and the API answers `404`. Each line
 * returned is a finding.
 */
async function endpointsRefused(page: Page): Promise<string[]> {
  const found: string[] = [];
  for (const address of ["/endpoints", "/organization/endpoints"]) {
    await page.goto(`${address}?lang=en`, { waitUntil: "load" });
    const notice = page.getByText(/This page is for organization administrators/);
    await notice.waitFor({ timeout: 30_000 }).catch(() => undefined);
    if ((await notice.count()) === 0) {
      found.push(`admin-only: ${address} did not say the page is for administrators`);
    }
    if ((await page.getByRole("table", { name: "All endpoints" }).count()) > 0) {
      found.push(`admin-only: ${address} shows the table`);
    }
  }
  if ((await page.getByRole("tab", { name: "All endpoints" }).count()) > 0) found.push("admin-only: the tab is offered");
  if ((await page.getByRole("link", { name: "All endpoints" }).count()) > 0) found.push("admin-only: a link is offered");
  const answer = await page.request.get("/api/v1/endpoints");
  if (answer.status() !== 404) found.push(`admin-only: GET /api/v1/endpoints answered ${answer.status()}`);
  return found;
}

/** A 4xx a person may meet with nothing wrong (the same rule as `walk.spec.ts`). */
function expected(who: string, response: Response): boolean {
  const status = response.status();
  if (status === 403 && who === "viewer") return true;
  if (status === 404 && /\/cs\/[^/]+\/ngsi-ld\/v1\/entities\?/.test(response.url())) return true;
  // An edit form asks for the person's draft of the item first; none yet is the 404 (T-3852).
  if (status === 404 && response.request().method() === "GET" && /\/drafts\/[A-Za-z]+\/[^/?]+$/.test(new URL(response.url()).pathname))
    return true;
  return status === 410 || (status === 404 && /\/agent-runs\/[^/]+\/(preview|events)/.test(response.url()));
}

for (const [who, person] of [
  ["steward", STEWARD],
  ["viewer", VIEWER],
] as const) {
  test(`every page and form, at ${WIDTHS.map((size) => size.width).join(" and ")} px in ${LANGS.join(", ")}, for the ${who}`, async ({ browser }) => {
    test.setTimeout(2_400_000);
    const { context, page: first } = await signIn(browser, person, `${P}/spaces?lang=en`);
    // A page's read of another origin (the API host's MCP route on /connect) failed its CORS
    // preflight on the marker and the walk reported a console error no person meets (T-5025).
    await journeyMarkerOnPortalOnly(context, new URL(first.url()).host);
    const report: { route: string; address: string; width: number; lang: string; findings: string[]; screenshot: string }[] = [];
    let current: string[] = [];
    let refused: string[] = [];
    // A renderer crash opens a fresh tab with the same listeners (T-5730).
    const tab = new Tab(context, first, (page) => {
      page.on("console", (message) => {
        if (message.type() === "error" && !message.text().startsWith("Failed to load resource")) {
          current.push(`console: ${message.text().slice(0, 200)}`);
        }
      });
      page.on("pageerror", (error) => current.push(`console: ${error.message.slice(0, 200)}`));
      page.on("response", (response) => {
        if (response.url().includes("/api/") && response.status() >= 400 && !expected(who, response)) {
          current.push(`http: ${response.status()} ${response.request().method()} ${new URL(response.url()).pathname}`);
        }
      });
    });

    try {
      // A section the installation hides is no page to walk (T-2874).
      const hidden = await hiddenSections(tab.page.request);
      // The routes are gathered on list pages a crash names no route of: once more on a fresh tab.
      const gathered = await tab.twice((page) => visits(page, who));
      if (gathered.crashed) throw new Error("the browser tab crashed twice while the walker gathered the routes to walk");
      const all = gathered.value.filter((visit) => !inHiddenSection(visit.address, hidden) && (!ONLY || ONLY.test(visit.address)));
      // Each address loads once per language and is judged at every width by resizing the open page:
      // a reload per width doubled the walk, and the walk after a rollout has 15 minutes (T-4950).
      for (const lang of LANGS) for (const visit of all) {
        let opened: Opened = { edgeRefused: false, blank: false, document: [] };
        for (const [index, size] of WIDTHS.entries()) {
          // A crash is judged again on a fresh tab, which loads the address anew at any width (T-6009).
          const judged = await tab.twice(async (page, again) => {
            const load = index === 0 || again;
            current = [];
            await page.setViewportSize(size);
            if (load) {
              // A load the edge refused is judged again after its window, without what it logged; a
              // blank or non-2xx document once more after 5 s, as one `document` finding (T-5857).
              opened = await openToWalk(page, `${visit.address}${visit.address.includes("?") ? "&" : "?"}lang=${lang}`, undefined, () => {
                current = [];
              });
            } else {
              await page.waitForTimeout(300);
              if (!opened.edgeRefused) await afterSelfReload(page);
            }
            // A blank load is reported once, at the first width: the others resize the same empty tab.
            const viewer = who === "viewer" && !OWN_ROUTES.has(visit.route);
            const findings =
              !load && opened.blank
                ? current
                : [...current, ...(await judgeOpened(page, load ? opened : { ...opened, document: [] }, { namespaces: NAMESPACES, viewer }))];
            const screenshot = test.info().outputPath(`${who}-${lang}-${size.width}${visit.address.replace(/[^a-z0-9]+/gi, "-")}.png`);
            await page.screenshot({ path: screenshot, fullPage: true, mask: secretMasks(page) });
            await shot(page, { area: "portal", route: visit.address, state: "open", who, lang });
            return { findings, screenshot };
          });
          if (judged.crashed) {
            // Crashed on two tabs: a person meets this as a dead tab. Reported on the page, and the walk goes on.
            report.push({ ...visit, width: size.width, lang, findings: ["crash: the browser tab crashed on this page"], screenshot: "" });
            break;
          }
          const { findings, screenshot } = judged.value;
          report.push({ ...visit, width: size.width, lang, findings: unexcused(visit.route, findings, allow.excused), screenshot });
        }
      }
      if (who === "viewer") {
        const checked = await tab.run(endpointsRefused);
        refused = checked.crashed ? ["admin-only: the browser tab crashed on the cross-project Endpoints"] : checked.value;
      }
    } finally {
      const body = JSON.stringify(report, null, 2);
      if (process.env.SHOTS_DIR) writeIndex(process.env.SHOTS_DIR);
      // walk-rows.ts fills no row from a page this walker found anything on (T-3986); the walk after
      // a rollout files a task per finding from it (rollout-walk.sh, T-4950).
      const reports = process.env.WALK_REPORT_DIR ?? process.env.SHOTS_DIR;
      if (reports) writeFileSync(join(reports, `walker-${who}.json`), body);
      await test.info().attach(`walker-${who}.json`, { body, contentType: "application/json" });
      await context.close();
    }
    expect(refused, "the cross-project Endpoints reach no one but an administrator").toEqual([]);
    expect(report.length, "the walker opened the pages").toBeGreaterThan(ONLY ? 0 : 40);
    expect(
      report.filter((entry) => entry.findings.length > 0).map(({ address, width, lang, findings }) => ({ address, width, lang, findings })),
    ).toEqual([]);
  });
}
