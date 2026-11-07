/**
 * T-5523 — UI critic 3/6: policy and access visualization. Each persona walks the screens that
 * should answer "who may write `status` on this Endpoint?", "why can't I edit `status`?" and
 * "what does this Policy allow?": the project's access tab, the Endpoint page, the explorer's
 * access panel over one type, the Policies list and one Policy. The steward (an organization
 * administrator) also opens the access explorer and "Try a person". demo.zilina (another
 * organization) opens the same addresses, and no helsinki Policy name may show on them.
 *
 * It records, it does not judge taste: per stop the time to the page's first heading, the clicks
 * it took, whether the page names the attribute and a reason, axe's serious/critical findings
 * (those fail the run), and 1440 + 400 shots under $SHOTS_DIR, all into $SHOTS_DIR/critic-policy.json
 * for the task's score table. Read-only: no write, no proposal; the seeded bikes-ops Endpoint and
 * its Policies (deployment seed/helsinki: bikes-ops-steward grants `status` to the steward alone).
 *
 *   SHOTS_DIR=/workspace/evidence/<agent>/T-5523 VIEWER_PASSWORD=… EDITOR_PASSWORD=… \
 *     PORTAL_PASSWORD=… PORTAL_TOTP=… ZILINA_PASSWORD=… \
 *     npx playwright test --config playwright.live.config.ts critic-policy
 */
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "../axe";
import { EDITOR, STEWARD, VIEWER, openToJudge, signIn } from "./portal";

const P = "/projects/helsinki";
const ENDPOINT = "helsinki-bikes-ops";
const TYPE = "BikeHireDockingStation";
const ATTRIBUTE = "status";
const POLICY = "bikes-ops-steward";
const ZILINA = { user: "demo.zilina@hel.fi", password: process.env.ZILINA_PASSWORD ?? "" };
const DIR = process.env.SHOTS_DIR ?? "test-results/critic-policy";
const WIDTHS = [1440, 400];

interface Stop {
  id: string;
  address: string;
  /** Clicks a person needs from the Portal's home to reach it (counted by hand from the navigation). */
  clicks: number;
  stewardOnly?: boolean;
}

const STOPS: Stop[] = [
  { id: "your-access", address: `${P}/settings/access`, clicks: 2 },
  { id: "endpoint", address: `${P}/endpoints/${ENDPOINT}`, clicks: 2 },
  { id: "explore-access", address: `${P}/explore?space=helsinki&endpoint=${ENDPOINT}&type=${TYPE}`, clicks: 3 },
  { id: "policies", address: `${P}/policies`, clicks: 1 },
  { id: "policy", address: `${P}/policies/${POLICY}`, clicks: 2 },
  { id: "explorer", address: `${P}/settings/explorer`, clicks: 3, stewardOnly: true },
];

interface Record {
  persona: string;
  stop: string;
  ms: number;
  clicks: number;
  namesAttribute: boolean;
  namesReason: boolean;
  namesPolicy: boolean;
  seriousAxe: string[];
  shots: string[];
}

const records: Record[] = [];

async function visit(page: Page, persona: string, stop: Stop): Promise<Record> {
  const started = Date.now();
  await openToJudge(page, `${stop.address}${stop.address.includes("?") ? "&" : "?"}lang=en`);
  await page.getByRole("heading").first().waitFor({ timeout: 30_000 });
  const ms = Date.now() - started;
  const text = (await page.locator("main").first().innerText().catch(() => "")) ?? "";
  const shots: string[] = [];
  for (const width of WIDTHS) {
    await page.setViewportSize({ width, height: width === 400 ? 860 : 900 });
    const file = join(DIR, `${persona}-${stop.id}-${width}.png`);
    await page.screenshot({ path: file, fullPage: true });
    shots.push(file);
  }
  await page.setViewportSize({ width: 1440, height: 900 });
  const seriousAxe = (await axeViolations(page)).filter((v) => /\((serious|critical)\)/.test(v));
  return {
    persona,
    stop: stop.id,
    ms,
    clicks: stop.clicks,
    namesAttribute: new RegExp(`\\b${ATTRIBUTE}\\b`).test(text),
    namesReason: /\b(because|no grant|not granted|ask|policy|refus|denied|may not|cannot)\b/i.test(text),
    namesPolicy: text.includes(POLICY),
    seriousAxe,
    shots,
  };
}

test.describe("critic 3/6: who may do what on an Endpoint, and why", () => {
  test.describe.configure({ mode: "serial" });
  test.setTimeout(10 * 60_000);

  test.beforeAll(() => mkdirSync(DIR, { recursive: true }));
  test.afterAll(() => writeFileSync(join(DIR, "critic-policy.json"), `${JSON.stringify(records, null, 2)}\n`));

  const people = [
    { persona: "viewer", who: VIEWER, admin: false },
    { persona: "editor", who: EDITOR, admin: false },
    { persona: "steward", who: STEWARD, admin: true },
  ];
  for (const { persona, who, admin } of people) {
    test(`${persona} walks the access screens`, async ({ browser }) => {
      test.skip(!who.password || (admin && !STEWARD.totp), `needs the ${persona}'s password${admin ? " and PORTAL_TOTP" : ""}`);
      const { context, page } = await signIn(browser, who, `${P}/home?lang=en`);
      for (const stop of STOPS.filter((s) => admin || !s.stewardOnly)) {
        const record = await visit(page, persona, stop);
        records.push(record);
        expect(record.seriousAxe, `${persona} ${stop.id}: axe serious/critical`).toEqual([]);
      }
      await context.close();
    });
  }

  test("demo.zilina sees no helsinki Policy", async ({ browser }) => {
    test.skip(!ZILINA.password, "needs ZILINA_PASSWORD");
    const { context, page } = await signIn(browser, ZILINA, "/home?lang=en");
    for (const stop of STOPS) {
      await openToJudge(page, `${stop.address}${stop.address.includes("?") ? "&" : "?"}lang=en`);
      const text = await page.locator("body").innerText();
      expect(text, `zilina ${stop.id}`).not.toContain(POLICY);
      expect(text, `zilina ${stop.id}`).not.toContain("public-air-quality");
      await page.screenshot({ path: join(DIR, `zilina-${stop.id}-1440.png`), fullPage: true });
    }
    await context.close();
  });
});
