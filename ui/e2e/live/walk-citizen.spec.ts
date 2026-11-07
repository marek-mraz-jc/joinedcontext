/**
 * T-5540 — a citizen finds and uses public data without an account, timed, on dev. One person,
 * no session and no token, walks from the installation's front door to a downloaded file and an
 * AI client's answer, in Slovak and in English, on a phone (400 px) and a desktop (1440 px):
 *
 *   1. land on the apex and on the open-data portal (CKAN)
 *   2. search "parkovanie"/"parking" and "ovzdušie"/"air quality" in the Portal catalogue and in CKAN
 *   3. open an air-quality dataset: licence and publisher are on the page
 *   4. look for a map of it (Portal dataset page, its developer page, CKAN's dataset page)
 *   5. download its CSV and its GeoJSON (OGC API - Features items): bytes and seconds
 *   6. take the "use with AI" path: the public MCP's OAuth metadata, then a scripted client lists
 *      the Endpoints and asks one search
 *   7. try a write and a private dataset: both refused, in words, with no title leaked
 *
 * Every page is photographed (SHOTS_DIR), checked by axe, and checked for the dead ends a citizen
 * cannot get past: a login wall, raw JSON, a URN in a table, English words on the Slovak page.
 * Lap times, axe counts and dead ends go to `$SHOTS_DIR/walk-citizen-<lang>.json`; the run fails on any
 * dead end (expect.soft), so the report is complete before it does.
 *
 * Anonymous reads only: no account is created, the OAuth path stops at the public metadata, the
 * write carries no credential. The edge limits anonymous callers per address; a 429 is waited out
 * once (Retry-After, at most 65 s) and recorded, never retried in a loop.
 *
 *   SHOTS_DIR=/workspace/evidence/<agent>/T-5540 npx playwright test --config playwright.live.config.ts walk-citizen
 */
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { expect, test } from "@playwright/test";
import type { APIRequestContext, Browser, Page } from "@playwright/test";
import { axeViolations } from "../axe";
import { shot } from "./shoot";

const LANGS = ["sk", "en"] as const;
type Lang = (typeof LANGS)[number];
const WIDTHS = [400, 1440];
const PORTAL = new URL(
  process.env.PORTAL_URL ?? "https://portal.dev.joinedcontext.com",
);
const DOMAIN = PORTAL.hostname.replace(/^portal\./, "");
const APEX = `https://${DOMAIN}`;
const CKAN = `https://opendata.${DOMAIN}`;
const API = `https://api.${DOMAIN}`;
const MCP = `${API}/api/mcp`;
const DIR = process.env.SHOTS_DIR;
// A download a citizen waits longer for than this is a dead end: they leave before the file comes.
const SLOW_MS = 10_000;
const SEARCHES: Record<Lang, string[]> = {
  sk: ["parkovanie", "ovzdušie"],
  en: ["parking", "air quality"],
};
// The air-quality dataset the walk opens: the second search's first hit, so a renamed dataset
// never breaks the walk. A private Endpoint of the seed (helsinki's Alerts desk App Endpoint,
// audience project-list) is what a citizen must not reach.
const PRIVATE = {
  slug: process.env.WALK_PRIVATE_SLUG ?? "ed22hwvdlgbhrpawv72rgfoyi4",
  name: "app-alerts-desk",
  title: "Alerts desk",
};

interface Lap {
  lang: Lang;
  width: number;
  step: string;
  ms: number;
  note?: string;
}
interface Audit {
  lang: Lang;
  width: number;
  page: string;
  url: string;
  serious: number;
  violations: string[];
}
interface DeadEnd {
  lang: Lang;
  width: number;
  step: string;
  what: string;
}
const report = {
  laps: [] as Lap[],
  axe: [] as Audit[],
  deadEnds: [] as DeadEnd[],
  limited: [] as string[],
};

// English words of the public pages that differ from their Slovak text: one on the Slovak page is
// an untranslated string a citizen reads.
const locale = (lang: Lang) =>
  JSON.parse(
    readFileSync(
      new URL(`../../src/locales/${lang}.json`, import.meta.url),
      "utf8",
    ),
  ) as Record<string, unknown>;
const leaves = (node: unknown, out: string[] = []): string[] => {
  if (typeof node === "string") out.push(node);
  else if (node && typeof node === "object")
    Object.values(node).forEach((child) => leaves(child, out));
  return out;
};
interface Words {
  search: { placeholder: string };
  results: { label: string };
}
const words = Object.fromEntries(
  LANGS.map((lang) => [lang, locale(lang).catalogue as Words]),
) as Record<Lang, Words>;
const sk = new Set(leaves(words.sk));
const ENGLISH = leaves(words.en).filter(
  (text) =>
    !sk.has(text) &&
    !text.includes("{") &&
    /^[A-Z][a-z]+(?: [a-z]+){1,4}$/.test(text),
);

const height = (width: number) => (width < 768 ? 860 : 900);
const withLang = (address: string, lang: Lang) =>
  `${address}${address.includes("?") ? "&" : "?"}lang=${lang}`;

async function timed<T>(
  lap: Omit<Lap, "ms">,
  work: () => Promise<T>,
): Promise<T> {
  const start = Date.now();
  try {
    return await work();
  } finally {
    report.laps.push({ ...lap, ms: Date.now() - start });
  }
}

/**
 * One request. A 429, or a 503 with Retry-After (the gateway builds a bounded number of downloads
 * at once), is waited out as the answer asks, at most five times, and recorded.
 */
async function call(
  request: APIRequestContext,
  method: "GET" | "POST",
  url: string,
  init: { data?: unknown; headers?: Record<string, string> } = {},
) {
  const send = () =>
    request.fetch(url, {
      method,
      data: init.data,
      headers: init.headers,
      failOnStatusCode: false,
      maxRedirects: 0,
      timeout: 90_000,
    });
  let answer = await send();
  for (let retry = 0; retry < 5; retry += 1) {
    const after = answer.headers()["retry-after"];
    if (answer.status() !== 429 && !(answer.status() === 503 && after)) break;
    const wait = Math.min(Number(after ?? "60") || 60, 65);
    report.limited.push(
      `${method} ${url}: ${answer.status()}, waited ${wait} s`,
    );
    await new Promise((done) => setTimeout(done, wait * 1000));
    answer = await send();
  }
  return answer;
}

/** RFC 4180 rows: quoted fields may hold commas, quotes and line breaks. */
function parseCsv(text: string): string[][] {
  const rows: string[][] = [];
  let row: string[] = [];
  let field = "";
  let quoted = false;
  const endField = () => {
    row.push(field);
    field = "";
  };
  const endRow = () => {
    endField();
    rows.push(row);
    row = [];
  };
  for (let i = 0; i < text.length; i += 1) {
    const c = text[i];
    if (quoted && c === '"' && text[i + 1] === '"') {
      field += '"';
      i += 1;
    } else if (c === '"') quoted = !quoted;
    else if (quoted) field += c;
    else if (c === ",") endField();
    else if (c === "\n") endRow();
    else if (c !== "\r") field += c;
  }
  if (field || row.length) endRow();
  return rows;
}

/** Photographs and audits the page as it stands; records every dead end it shows. */
async function look(
  page: Page,
  lang: Lang,
  step: string,
  name: string,
): Promise<void> {
  const width = page.viewportSize()?.width ?? 0;
  const deadEnd = (what: string) =>
    report.deadEnds.push({
      lang,
      width,
      step,
      what: `${name} (${page.url()}): ${what}`,
    });
  if (
    /^idm\./.test(new URL(page.url()).hostname) ||
    (await page.locator("input[type=password]").count()) > 0
  )
    deadEnd("asks for a login");
  const body = (await page.locator("body").innerText()).trim();
  if (/^[[{]/.test(body)) deadEnd("shows raw JSON");
  const urns = await page
    .locator("td, th")
    .filter({ hasText: /^\s*urn:ngsi-ld:/ })
    .count();
  if (urns > 0) deadEnd(`${urns} table cells show a URN`);
  if (lang === "sk") {
    const english = ENGLISH.filter((text) => body.includes(text));
    if (english.length)
      deadEnd(`English on the Slovak page: ${english.slice(0, 5).join(" | ")}`);
  }
  const violations = await axeViolations(page).catch((error: Error) => [
    `axe did not run: ${error.message}`,
  ]);
  const serious = violations.filter((line) =>
    /\((serious|critical)\)/.test(line),
  );
  report.axe.push({
    lang,
    width,
    page: name,
    url: page.url(),
    serious: serious.length,
    violations,
  });
  await shot(page, {
    area: "citizen",
    route: name,
    state: step,
    who: "anonymous",
    lang,
  });
}

async function walk(
  browser: Browser,
  lang: Lang,
  width: number,
): Promise<void> {
  const context = await browser.newContext({
    viewport: { width, height: height(width) },
    locale: lang === "sk" ? "sk-SK" : "en-GB",
  });
  const page = await context.newPage();
  const request = context.request;
  const lap = (step: string) => ({ lang, width, step });
  try {
    // 1. The front doors.
    await timed(lap("1 apex"), async () => {
      await page.goto(withLang(APEX, lang), { waitUntil: "load" });
      await look(page, lang, "landing", "apex");
    });
    await timed(lap("1 open-data portal"), async () => {
      await page.goto(`${CKAN}/${lang}/`, { waitUntil: "load" });
      await look(page, lang, "landing", "ckan-home");
    });

    // 2. Search, in the Portal catalogue and in CKAN.
    let dataset = "";
    for (const query of SEARCHES[lang]) {
      await timed(lap(`2 search portal "${query}"`), async () => {
        await page.goto(withLang("/catalogue", lang), { waitUntil: "load" });
        const box = page.getByPlaceholder(words[lang].search.placeholder);
        await box.fill(query);
        await box.press("Enter");
        await expect(page).toHaveURL(/[?&]q=/);
        const hits = page
          .getByRole("region", { name: words[lang].results.label })
          .getByRole("heading", { level: 2 })
          .getByRole("link");
        await expect(
          hits.first(),
          `the catalogue finds "${query}"`,
        ).toBeVisible({ timeout: 60_000 });
        await look(page, lang, `search-${query}`, "catalogue");
        if (query === SEARCHES[lang][1])
          dataset =
            new URL(
              (await hits.first().getAttribute("href")) ?? "",
              PORTAL,
            ).pathname
              .split("/")
              .pop() ?? "";
      });
      await timed(lap(`2 search ckan "${query}"`), async () => {
        await page.goto(
          `${CKAN}/${lang}/dataset/?q=${encodeURIComponent(query)}`,
          { waitUntil: "load" },
        );
        const found = await page
          .locator(".dataset-item, .dataset-heading")
          .count();
        if (found === 0)
          report.deadEnds.push({
            lang,
            width,
            step: "2",
            what: `CKAN finds nothing for "${query}"`,
          });
        await look(page, lang, `search-${query}`, "ckan-search");
      });
    }
    expect(dataset, "the air-quality search opens a dataset").not.toBe("");

    // 3. The dataset: licence and publisher.
    const detail = (await (
      await call(
        request,
        "GET",
        `${PORTAL.origin}/api/v1/catalogue/datasets/${dataset}`,
      )
    ).json()) as {
      title: string;
      licence?: { title: string };
      publisher?: { title: string };
      endpoint?: string;
      resources: { name: string; format: string; url: string }[];
    };
    await timed(lap("3 open dataset"), async () => {
      await page.goto(withLang(`/catalogue/${dataset}`, lang), {
        waitUntil: "load",
      });
      await expect(
        page.getByRole("heading", { level: 1, name: detail.title }),
      ).toBeVisible({ timeout: 60_000 });
      expect
        .soft(detail.licence?.title, "the dataset names its licence")
        .toBeTruthy();
      expect
        .soft(detail.publisher?.title, "the dataset names its publisher")
        .toBeTruthy();
      if (detail.licence)
        await expect
          .soft(
            page.getByText(detail.licence.title).first(),
            "the licence is on the page",
          )
          .toBeVisible();
      if (detail.publisher)
        await expect
          .soft(
            page.getByText(detail.publisher.title).first(),
            "the publisher is on the page",
          )
          .toBeVisible();
      await look(page, lang, "detail", "dataset");
    });

    // 4. A map of it: on the dataset page, its developer page or CKAN's page.
    await timed(lap("4 map"), async () => {
      const map =
        "canvas.maplibregl-canvas, .leaflet-container, [role=region][aria-label*=map i], [role=application][aria-label*=map i]";
      const where: string[] = [];
      if ((await page.locator(map).count()) > 0) where.push("dataset page");
      const developer = page.locator('a[href^="/d/"]').first();
      if ((await developer.count()) > 0) {
        await page.goto(
          withLang((await developer.getAttribute("href")) ?? "/", lang),
          { waitUntil: "load" },
        );
        await page.waitForTimeout(2_000);
        if ((await page.locator(map).count()) > 0) where.push("developer page");
        await look(page, lang, "detail", "developer");
      }
      await page.goto(`${CKAN}/${lang}/dataset/${dataset}`, {
        waitUntil: "load",
      });
      if ((await page.locator(map).count()) > 0)
        where.push("CKAN dataset page");
      await look(page, lang, "detail", "ckan-dataset");
      if (!where.length)
        report.deadEnds.push({
          lang,
          width,
          step: "4",
          what: `no map of ${dataset} on the dataset page, its developer page or CKAN's page`,
        });
      report.laps.push({
        ...lap("4 map found on"),
        ms: 0,
        note: where.join(", ") || "nowhere",
      });
    });

    // 5. Downloads: CSV and GeoJSON, each non-empty and parsed.
    const csv = detail.resources.find(
      (resource) => resource.format === "CSV" && resource.url.startsWith(API),
    );
    expect(csv, "the dataset offers a CSV").toBeTruthy();
    await timed(lap("5 download CSV"), async () => {
      const started = Date.now();
      const answer = await call(request, "GET", csv?.url ?? "");
      expect(answer.status(), "the CSV downloads").toBe(200);
      const rows = parseCsv(await answer.text()).filter((row) =>
        row.some((cell) => cell !== ""),
      );
      expect(
        rows.length,
        "the CSV has a header and at least one row",
      ).toBeGreaterThan(1);
      expect(
        rows.every((row) => row.length === rows[0].length),
        "every CSV row has the header's columns",
      ).toBe(true);
      report.laps.push({
        ...lap("5 CSV size"),
        ms: 0,
        note: `${(await answer.body()).length} bytes, ${rows.length - 1} rows`,
      });
      const took = Date.now() - started;
      if (took > SLOW_MS)
        report.deadEnds.push({
          lang,
          width,
          step: "5",
          what: `the CSV of ${dataset} (${csv?.url}) took ${took} ms for ${rows.length - 1} rows`,
        });
    });
    const ogc = detail.resources.find(
      (resource) => resource.format === "OGCFEAT",
    );
    expect(
      ogc,
      "the dataset offers OGC API - Features (its GeoJSON)",
    ).toBeTruthy();
    await timed(lap("5 download GeoJSON"), async () => {
      const collections = (await (
        await call(request, "GET", `${ogc?.url}collections?f=json`)
      ).json()) as { collections: { id: string }[] };
      const items = await call(
        request,
        "GET",
        `${ogc?.url}collections/${collections.collections[0].id}/items?f=json&limit=1000`,
      );
      expect(items.status(), "the GeoJSON downloads").toBe(200);
      const geojson = JSON.parse(await items.text()) as {
        type: string;
        features: unknown[];
      };
      expect(geojson.type).toBe("FeatureCollection");
      expect(
        geojson.features.length,
        "the GeoJSON holds at least one feature",
      ).toBeGreaterThan(0);
      report.laps.push({
        ...lap("5 GeoJSON size"),
        ms: 0,
        note: `${(await items.body()).length} bytes, ${geojson.features.length} features`,
      });
    });

    // 6. Use with AI: the public MCP, anonymous. Discovery only: no client is registered.
    await timed(lap("6 MCP"), async () => {
      const meta = await call(
        request,
        "GET",
        `${API}/.well-known/oauth-protected-resource/api/mcp`,
      );
      expect
        .soft(meta.status(), "the public MCP publishes its OAuth metadata")
        .toBe(200);
      const rpc = async (id: number, method: string, params: unknown) => {
        const answer = await call(request, "POST", MCP, {
          headers: {
            "Content-Type": "application/json",
            Accept: "application/json, text/event-stream",
          },
          data: { jsonrpc: "2.0", id, method, params },
        });
        expect(answer.status(), `${method} answers an anonymous client`).toBe(
          200,
        );
        return (await answer.json()) as {
          result?: { content?: { text: string }[]; tools?: { name: string }[] };
          error?: unknown;
        };
      };
      await rpc(1, "initialize", {
        protocolVersion: "2025-06-18",
        capabilities: {},
        clientInfo: { name: "t5540-walk", version: "1" },
      });
      // Paged: a page ends in "total N; nextCursor C" while more follow.
      let listed = "";
      for (let cursor: string | undefined, id = 2; id < 12; id += 1) {
        const text =
          (
            await rpc(id, "tools/call", {
              name: "list_endpoints",
              arguments: cursor ? { cursor, limit: 100 } : { limit: 100 },
            })
          ).result?.content?.[0]?.text ?? "";
        listed += `${text}\n`;
        cursor = /nextCursor (\S+)/.exec(text)?.[1];
        if (!cursor) break;
      }
      const rows = listed
        .split("\n")
        .map((line) => line.split(" | "))
        .filter((row) => row.length > 4 && row[0] !== "slug");
      expect(rows.length, "the public MCP lists Endpoints").toBeGreaterThan(0);
      report.laps.push({
        ...lap("6 MCP endpoints listed"),
        ms: 0,
        note: `${rows.length}`,
      });
      expect(
        rows.filter((row) => row[3] !== "public").map((row) => row[0]),
        "the public MCP lists only public Endpoints",
      ).toEqual([]);
      expect(listed, "the private Endpoint is not listed").not.toContain(
        PRIVATE.slug,
      );
      expect(listed).not.toContain(PRIVATE.title);
      const answer =
        (
          await rpc(20, "tools/call", {
            name: "search",
            arguments: { query: SEARCHES[lang][1] },
          })
        ).result?.content?.[0]?.text ?? "";
      expect(answer.length, "the search answers").toBeGreaterThan(0);
      expect(answer).not.toContain(PRIVATE.slug);
    });

    // 7. Refusals: a write and a private dataset, in words, with nothing leaked.
    await timed(lap("7 write refused"), async () => {
      const endpoint = csv?.url.replace(/\/file\.csv$/, "") ?? "";
      const write = await call(
        request,
        "POST",
        `${endpoint}/ngsi-ld/v1/entities`,
        {
          headers: { "Content-Type": "application/json" },
          data: {
            id: "urn:ngsi-ld:AirQualityObserved:t5540-walk",
            type: "AirQualityObserved",
          },
        },
      );
      // A read-only view answers 405 with an Allow that names no write; a writable one 401/403.
      expect(
        [401, 403, 405],
        `an anonymous write is refused (got ${write.status()})`,
      ).toContain(write.status());
      if (write.status() === 405)
        expect(
          write.headers()["allow"] ?? "",
          "a 405 names the methods allowed",
        ).not.toMatch(/POST/);
      report.laps.push({
        ...lap("7 write answer"),
        ms: 0,
        note: `${write.status()} ${(await write.text()).slice(0, 120)}`,
      });
      expect(
        (await write.text()).length,
        "the refusal says why",
      ).toBeGreaterThan(0);
    });
    await timed(lap("7 private refused"), async () => {
      const read = await call(
        request,
        "GET",
        `${API}/api/endpoint/${PRIVATE.slug}/ngsi-ld/v1/types`,
      );
      expect(
        [401, 403, 404],
        `the private Endpoint refuses an anonymous read (got ${read.status()})`,
      ).toContain(read.status());
      expect(await read.text()).not.toContain(PRIVATE.title);
      const listed = await call(
        request,
        "GET",
        `${PORTAL.origin}/api/v1/catalogue/datasets/${PRIVATE.name}`,
      );
      expect(
        [401, 404],
        `the catalogue does not serve the private dataset (got ${listed.status()})`,
      ).toContain(listed.status());
      expect(await listed.text()).not.toContain(PRIVATE.title);
      await page.goto(withLang(`/catalogue/${PRIVATE.name}`, lang), {
        waitUntil: "load",
      });
      await expect(page.getByRole("heading", { level: 1 })).toBeVisible({
        timeout: 60_000,
      });
      await expect(page.getByText(PRIVATE.title)).toHaveCount(0);
      await look(page, lang, "private", "dataset-missing");
    });
  } finally {
    await context.close();
  }
}

test.setTimeout(900_000);

// Playwright starts a new worker after a failed test, so each test writes its own language's report.
function save(lang: Lang): void {
  const text = JSON.stringify(report, null, 2);
  if (!DIR) return console.log(text);
  mkdirSync(DIR, { recursive: true });
  writeFileSync(join(DIR, `walk-citizen-${lang}.json`), text);
}

for (const lang of LANGS) {
  test(`a citizen with no account finds, maps, downloads and asks about public data (${lang})`, async ({
    browser,
  }) => {
    try {
      for (const width of WIDTHS) await walk(browser, lang, width);
    } finally {
      save(lang);
    }
    for (const end of report.deadEnds.filter((one) => one.lang === lang))
      expect
        .soft(end.what, `dead end at step ${end.step}, ${end.width} px`)
        .toBe("");
  });
}
