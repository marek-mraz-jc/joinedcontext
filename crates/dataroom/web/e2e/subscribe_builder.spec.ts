import { test, expect, type Page, type Route } from "@playwright/test";
import { readFileSync } from "node:fs";
import { join } from "node:path";

// T-6758: the Subscribe menu's calendar builder through the real jc-viz.js, on the page the
// `pages` test renders, with the Endpoint's facets and calendar answered here. By hand:
//   cargo test -p dataroom-web --test pages d3_dataset_page_builds_a   (writes $CARGO_TARGET_DIR/tmp/builder.html)
//   DATAROOM_PAGE_OUT=$CARGO_TARGET_DIR/tmp node <portal ui>/node_modules/@playwright/test/cli.js test \
//     -c crates/dataroom/web/e2e subscribe_builder
const PAGE = join(process.env.DATAROOM_PAGE_OUT ?? "test-results", "builder.html");
const ASSETS = join(__dirname, "..", "assets");
const ORIGIN = "https://opendata.dev.joinedcontext.com";
const SHOTS = process.env.DATAROOM_SHOTS ?? "test-results";

const TOWNS = [
  { value: "Banská Bystrica", count: 41 },
  { value: "Zvolen", count: 17 },
  { value: "Sliač", count: 6 },
];
// A genre with every q operator in it: the link must keep it one quoted value.
const GENRES = [
  { value: "Hudba", count: 22 },
  { value: 'Divadlo; "nové" | (pre deti)', count: 3 },
];

async function serve(page: Page, opts: { facets: boolean }) {
  const asked: string[] = [];
  await page.route(`${ORIGIN}/**`, (route: Route) => {
    const url = new URL(route.request().url());
    if (url.pathname === "/sk/dataset/events") {
      return route.fulfill({ contentType: "text/html; charset=utf-8", body: readFileSync(PAGE, "utf8") });
    }
    const name = url.pathname.replace(/^\/m\//, "");
    try {
      // jc-theme.css is base.css then the theme, as JC_THEME_CSS serves it.
      const body = name === "jc-theme.css"
        ? Buffer.concat([readFileSync(join(ASSETS, "base.css")), readFileSync(join(ASSETS, "public__jc-theme.css"))])
        : readFileSync(join(ASSETS, `public__${name}`));
      const type = name.endsWith(".css") ? "text/css" : name.endsWith(".svg") ? "image/svg+xml" : "text/javascript";
      return route.fulfill({ contentType: type, body });
    } catch {
      return route.fulfill({ status: 404, body: "" });
    }
  });
  await page.route("https://api.dev.joinedcontext.com/**", (route: Route) => {
    const url = new URL(route.request().url());
    asked.push(url.pathname + url.search);
    if (url.pathname.endsWith("/facets")) {
      const attr = url.searchParams.get("attr");
      const body = !attr
        ? { facets: opts.facets ? ["address.addressLocality", "eventCategory"] : [] }
        : { attr, values: attr === "eventCategory" ? GENRES : TOWNS, truncated: false };
      return route.fulfill({ contentType: "application/json", body: JSON.stringify(body) });
    }
    if (url.pathname.endsWith("/ngsi-ld/v1/entities")) {
      // The events of one town, located: Sliač's two venues and a stray one; Banská Bystrica's none.
      const q = url.searchParams.get("q") ?? "";
      const point = (lon: number, lat: number) => ({ id: `urn:ngsi-ld:Event:${lon}`, type: "Event", location: { type: "Point", coordinates: [lon, lat] } });
      const body = q === 'address[addressLocality]=="Sliač"' && url.searchParams.get("attrs") === "location"
        ? [point(19.15, 48.64), point(19.16, 48.65), point(17.1, 48.1)]
        : [];
      return route.fulfill({ contentType: "application/json", body: JSON.stringify(body) });
    }
    if (url.pathname.endsWith(".ics")) {
      const filter = decodeURIComponent(url.pathname);
      const year = new Date().getFullYear() + 1;
      const event = filter.includes("Zvolen")
        ? `BEGIN:VEVENT\r\nDTSTART:${year}0115T180000\r\nSUMMARY:Koncert na námestí vo Zvolene\r\nEND:VEVENT\r\n`
        : `BEGIN:VEVENT\r\nDTSTART:${year}0110\r\nSUMMARY:Jarmok\r\nEND:VEVENT\r\n`;
      return route.fulfill({ contentType: "text/calendar", body: `BEGIN:VCALENDAR\r\n${event}END:VCALENDAR\r\n` });
    }
    return route.fulfill({ status: 404, body: "" });
  });
  return asked;
}

const VIEWPORTS = [
  { width: 1440, height: 1000 },
  { width: 400, height: 900 },
] as const;

for (const viewport of VIEWPORTS) {
  test(`a town and a genre build one link (${viewport.width}px)`, async ({ page }) => {
    await page.setViewportSize(viewport);
    const asked = await serve(page, { facets: true });
    await page.goto(`${ORIGIN}/sk/dataset/events`);
    await page.locator(".jc-subscribe > summary").click();
    const town = page.getByLabel("Obec", { exact: true });
    const genre = page.getByLabel("Žáner", { exact: true });
    await expect(town).toBeVisible();
    await expect(page.locator("#jc-subscribe-no-facets")).toBeHidden();
    await page.screenshot({ path: `${SHOTS}/builder-empty-${viewport.width}.png`, fullPage: true });

    await town.fill("zvolen");
    await genre.fill(GENRES[1].value);
    const url = page.locator("#jc-subscribe-mine-url");
    const base = "https://api.dev.joinedcontext.com/api/endpoint/esbxdrr7snorjsp7uus2luihr6vl5vv6";
    const q = 'address[addressLocality]=="Zvolen";eventCategory=="Divadlo; \\"nové\\" | (pre deti)"';
    const segment = "q=" + encodeURIComponent(q).replace(/[!'()*]/g, (c) => "%" + c.charCodeAt(0).toString(16).toUpperCase());
    await expect(url).toHaveValue(`${base}/feeds/${segment}/vsetky.ics`);
    // One path segment: no raw / ; | ( ) " in it, so the gateway reads it as one filter.
    expect(segment).not.toMatch(/[/;|()"]/);
    await expect(page.locator("#jc-subscribe-mine-webcal")).toHaveAttribute("href", `webcal:${base.slice(6)}/feeds/${segment}/vsetky.ics`);
    await expect(page.locator("#jc-subscribe-mine-csv")).toHaveAttribute("href", `${base}/entities.csv?${segment}`);
    await expect(page.locator("#jc-subscribe-mine-google")).toHaveAttribute(
      "href",
      "https://calendar.google.com/calendar/r?cid=" + encodeURIComponent(`webcal:${base.slice(6)}/feeds/${segment}/vsetky.ics`),
    );
    await expect(page.locator("#jc-subscribe-preview")).toContainText("Koncert na námestí vo Zvolene");
    expect(asked).toContain(`/api/endpoint/esbxdrr7snorjsp7uus2luihr6vl5vv6/feeds/${segment}/vsetky.ics`);
    // The ? form of the same filter is what the gateway forwards: the two links say the same.
    expect(new URLSearchParams(segment).get("q")).toBe(q);
    await page.screenshot({ path: `${SHOTS}/builder-filled-${viewport.width}.png`, fullPage: true });

    await town.fill("Martin");
    await expect(town).toHaveAttribute("aria-invalid", "true");
    await expect(page.locator("#jc-subscribe-facet-1-hint")).toContainText("Martin");
    await expect(url).not.toHaveValue(/Martin/);
    await expect(page.locator(".jc-subscribe-query > .jc-subscribe-copy")).toBeHidden();
    await expect(page.locator(".jc-subscribe-query > .jc-subscribe-links")).toBeHidden();
    await page.screenshot({ path: `${SHOTS}/builder-refused-${viewport.width}.png`, fullPage: true });
  });

  test(`a genre within 5 km of a town builds one link (${viewport.width}px)`, async ({ page }) => {
    await page.setViewportSize(viewport);
    const asked = await serve(page, { facets: true });
    await page.goto(`${ORIGIN}/sk/dataset/events`);
    await page.locator(".jc-subscribe > summary").click();
    const place = page.getByLabel("V okolí obce", { exact: true });
    await expect(place).toBeVisible();
    await expect(page.getByLabel("Vzdialenosť", { exact: true })).toHaveValue("5");
    await expect(page.locator("#jc-subscribe-near-hint")).toContainText("Napríklad Zvolen");
    await page.getByLabel("Žáner", { exact: true }).fill("Hudba");
    await place.fill("sliač");
    const base = "https://api.dev.joinedcontext.com/api/endpoint/esbxdrr7snorjsp7uus2luihr6vl5vv6";
    // The centre is the median of the town's located events: the stray venue does not move it.
    const query = new URLSearchParams();
    query.set("q", 'eventCategory=="Hudba"');
    query.set("georel", "near;maxDistance==5000");
    query.set("geometry", "Point");
    query.set("coordinates", "[19.15,48.64]");
    const url = page.locator("#jc-subscribe-mine-url");
    // The link waits for the town's centre: it names the distance once the centre is read.
    await expect(url).toHaveValue(new RegExp(`^${base.replace(/[.]/g, "\\.")}/feeds/[^/]*georel=[^/]+/vsetky\\.ics$`));
    const segment = (await url.inputValue()).split("/feeds/")[1].split("/")[0];
    // The path form and the ? form carry one query: the same names and values, in one segment.
    expect(segment).not.toMatch(/[/;|()"\[\],]/);
    expect([...new URLSearchParams(segment)]).toEqual([...query]);
    await expect(page.locator("#jc-subscribe-mine-csv")).toHaveAttribute("href", `${base}/entities.csv?${segment}`);
    await expect.poll(() => asked).toContain(`/api/endpoint/esbxdrr7snorjsp7uus2luihr6vl5vv6/feeds/${segment}/vsetky.ics`);
    expect(asked.some((a) => a.startsWith("/api/endpoint/esbxdrr7snorjsp7uus2luihr6vl5vv6/ngsi-ld/v1/entities?"))).toBe(true);
    await page.screenshot({ path: `${SHOTS}/builder-near-${viewport.width}.png`, fullPage: true });

    await page.getByLabel("Vzdialenosť", { exact: true }).selectOption("25");
    await expect(url).toHaveValue(/georel=near%3BmaxDistance%3D%3D25000&/);

    // A town without a located event cannot be a centre: no link until the field changes.
    await place.fill("Banská Bystrica");
    await expect(place).toHaveAttribute("aria-invalid", "true");
    await expect(page.locator("#jc-subscribe-near-hint")).toContainText("Banská Bystrica");
    await expect(page.locator(".jc-subscribe-query > .jc-subscribe-copy")).toBeHidden();
    await page.screenshot({ path: `${SHOTS}/builder-near-unknown-${viewport.width}.png`, fullPage: true });
    await place.fill("");
    await expect(place).not.toHaveAttribute("aria-invalid", "true");
    await expect(url).not.toHaveValue(/georel/);
  });

  test(`no facets: dates still narrow the calendar (${viewport.width}px)`, async ({ page }) => {
    await page.setViewportSize(viewport);
    await serve(page, { facets: false });
    await page.goto(`${ORIGIN}/sk/dataset/events`);
    await page.locator(".jc-subscribe > summary").click();
    await expect(page.locator("#jc-subscribe-no-facets")).toBeVisible();
    await page.locator("#jc-subscribe-from").fill("2026-12-01");
    await expect(page.locator("#jc-subscribe-mine-url")).toHaveValue(/feeds\/q=startDate%3E%3D2026-12-01T00%3A00%3A00Z\/vsetky\.ics$/);
    await page.screenshot({ path: `${SHOTS}/builder-nofacets-${viewport.width}.png`, fullPage: true });
  });
}
