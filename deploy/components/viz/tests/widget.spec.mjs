// T-5192, DM-133: the catalogue's live data widget as the owner met it on bbsk-podujatia: the frame as tall
// as its content (map included, no inner scrollbar), header cells matching body cells, readable
// filter selects, columns capped in width, titles and order from the published model, and events
// from today soonest first. Matrix: width {1440, 400} x language {sk, en} x filter {closed, open}
// x sort {default, by name}. Chromium only: the image ships no Firefox or WebKit build.
//   PLAYWRIGHT_BROWSERS_PATH=/ms-playwright npx playwright test -c components/viz/tests widget
import { test, expect } from "@playwright/test";
import path from "node:path";
import { shots, CATALOGUE, serve } from "./explore-hosts.mjs";

const frame = (page) => page.frameLocator("#explore");
// The id and the type lead every grid, then the name (T-6210), then the model's order.
const LABELS = { sk: ["ID", "Typ", "Názov", "Začiatok", "Miesto", "Adresa", "Organizátor", "Web"], en: ["ID", "Type", "Name", "Start", "Venue", "Address", "Organizer", "Web"] };

for (const width of [1440, 400]) {
  for (const lang of ["sk", "en"]) {
    for (const filter of ["closed", "open"]) {
      for (const sort of ["default", "name"]) {
        test(`events ${width} ${lang} filter ${filter} sort ${sort}`, async ({ page, context }) => {
          const errors = [];
          page.on("pageerror", (e) => errors.push(String(e)));
          const requests = await serve(context);
          await page.setViewportSize({ width, height: 900 });
          await page.goto(`https://${CATALOGUE}/dataset/events?lang=${lang}`);
          const f = frame(page);
          const grid = f.getByRole("grid");
          await expect(grid).toBeVisible({ timeout: 20000 });
          await expect(grid.locator("tbody tr")).toHaveCount(20);
          // Titles and order from the model; the GeoProperty and the unranked provider stay out.
          const heads = (await grid.locator("thead tr").first().locator("th").allInnerTexts()).map((t) => t.replace(/[⋮↑↓]/g, "").trim());
          expect(heads).toEqual(LABELS[lang]);
          // Upcoming first: the read asks for today on, ordered by the start.
          const read = requests.map((r) => r.url).find((u) => u.pathname.endsWith("/entities") && u.searchParams.get("orderBy") === "startDate");
          expect(read?.searchParams.get("q")).toMatch(/^!startDate\|startDate>="\d{4}-\d{2}-\d{2}"$/);
          if (sort === "name") {
            await f.getByRole("button", { name: new RegExp(`${LABELS[lang][2]}$`) }).first().click();
            await expect.poll(() => requests.some((r) => r.url.searchParams.get("orderBy") === "name")).toBe(true);
          }
          // The model's opening query is folded under the Query button (T-6303), which shows it as text.
          const query = f.getByRole("button", { name: lang === "sk" ? "Dopyt" : "Query", exact: true });
          await expect(query).toHaveAttribute("aria-pressed", "false");
          await query.click();
          await expect(f.getByRole("textbox").first()).toHaveValue(/^!startDate\|startDate>=/);
          if (filter === "open") {
            // The conditions sit in the Filter panel (T-6303); on a phone it opens under the toolbar, in the reader's window.
            await f.locator("summary").filter({ hasText: /^Filter/ }).click();
            await f.getByRole("button", { name: lang === "sk" ? "Pridať podmienku" : "Add condition", exact: true }).click();
            const field = f.getByRole("combobox", { name: lang === "sk" ? "Pole" : "Field", exact: true });
            await field.selectOption({ index: 1 });
            await expect(field).toBeInViewport();
          }
          // Every body row has as many cells as the header row.
          const cells = await grid.locator("tbody tr").evaluateAll((trs) => trs.map((tr) => tr.children.length));
          const headCount = await grid.locator("thead tr").first().locator("th").count();
          expect(new Set(cells)).toEqual(new Set([headCount]));
          // Readable filter controls.
          const fonts = await f.locator("details[open] select").evaluateAll((all) => all.map((s) => parseFloat(getComputedStyle(s).fontSize)));
          if (filter === "open") expect(fonts.length).toBeGreaterThan(0);
          for (const size of fonts) expect(size).toBeGreaterThanOrEqual(14);
          // A 500-character name or venue never widens its column past its cap (40ch, 28ch), plus padding.
          const widths = await grid.evaluate((table) => {
            const probe = document.createElement("span");
            probe.style.cssText = "position:absolute;visibility:hidden;width:1ch";
            table.querySelector("tbody td .jc-grid-cell").appendChild(probe);
            const ch = probe.getBoundingClientRect().width;
            probe.remove();
            // The cap holds on each cell's box: the header also takes its share of the width the
            // table spreads over its columns to fill the frame, which depends on how many it has.
            const widest = (at) => Math.max(...[...table.querySelectorAll("tbody tr")].map((tr) => tr.children[at]?.querySelector(".jc-grid-cell")?.getBoundingClientRect().width ?? 0));
            return { ch, name: widest(2), venue: widest(4), web: widest(7) };
          });
          expect(widths.name).toBeLessThanOrEqual(widths.ch * 40 + 48);
          expect(widths.venue).toBeLessThanOrEqual(widths.ch * 28 + 48);
          expect(widths.web).toBeLessThanOrEqual(widths.ch * 28 + 48);
          // A link column holds a short link that opens a new tab without the referrer.
          const link = grid.locator("tbody tr").first().getByRole("link").first();
          await expect(link).toHaveAttribute("rel", "noopener noreferrer");
          await expect(link).toHaveAttribute("href", /^https:\/\//);
          // The frame is as tall as its content: the map shows and nothing scrolls inside.
          await expect(f.locator("#map-section")).toBeVisible();
          await f.locator("#map").scrollIntoViewIfNeeded();
          await expect(f.locator("#map .maplibregl-canvas")).toHaveCount(1);
          await expect.poll(async () => {
            const outer = await page.locator("#explore").evaluate((e) => e.getBoundingClientRect().height);
            const inner = await f.locator("html").evaluate(() => Math.ceil(document.querySelector("main").getBoundingClientRect().bottom + window.scrollY));
            return Math.abs(outer - inner);
          }, { timeout: 10000 }).toBeLessThanOrEqual(2);
          const box = await page.locator("#explore").boundingBox();
          // The whole map lies inside the frame's box: nothing of it is cut off at the frame's end.
          const mapBottom = await f.locator("#map").evaluate((m) => m.getBoundingClientRect().bottom);
          expect(mapBottom).toBeLessThanOrEqual(box.height);
          const scrolls = await f.locator("html").evaluate((html) => html.scrollHeight - window.innerHeight);
          expect(scrolls).toBeLessThanOrEqual(1);
          expect(errors).toEqual([]);
          await page.waitForTimeout(400);
          await page.screenshot({ path: path.join(shots, `widget-${width}-${lang}-filter-${filter}-sort-${sort}.png`), fullPage: true });
        });
      }
    }
  }
}

// Critic A2: the row dialog opens on the reader's screen, not in the middle of a 2400 px frame.
for (const width of [1440, 400]) {
  test(`events ${width}: a row's dialog opens where the reader is, Escape closes it`, async ({ page, context }) => {
    await serve(context);
    await page.setViewportSize({ width, height: 800 });
    await page.goto(`https://${CATALOGUE}/dataset/events?lang=sk`);
    const f = frame(page);
    const rows = f.getByRole("grid").locator("tbody tr");
    await expect(rows).toHaveCount(20);
    const last = rows.nth(18).getByRole("button", { name: /^Otvoriť:/ });
    await last.scrollIntoViewIfNeeded();
    await page.waitForTimeout(300);
    await last.click();
    const dialog = f.locator("dialog[open]");
    await expect(dialog).toBeVisible();
    await page.waitForTimeout(300);
    const box = await dialog.boundingBox();
    expect(box.y).toBeGreaterThanOrEqual(-1);
    expect(box.y + Math.min(box.height, 200)).toBeLessThanOrEqual(800);
    await page.screenshot({ path: path.join(shots, `widget-${width}-dialog.png`) });
    await page.keyboard.press("Escape");
    await expect(f.locator("dialog[open]")).toHaveCount(0);
  });
}

// T-5134: in the row's card the organizer reads as its name linked to its web site (new tab, no opener), and
// as plain text when it names none. The table cell's link is the Portal SDK grid's (sdk/tests/grid_cells), which
// reaches this page with the next explorer bundle built from a Portal commit that holds it.
for (const width of [1440, 400]) {
  test(`events ${width}: the card links the organizer to its web site, or names it when it has none`, async ({ page, context }) => {
    await serve(context);
    await page.setViewportSize({ width, height: 900 });
    await page.goto(`https://${CATALOGUE}/dataset/events?lang=sk`);
    const f = frame(page);
    await expect(f.getByRole("grid").locator("tbody tr")).toHaveCount(20);
    await page.screenshot({ path: path.join(shots, `organizer-${width}-table.png`), fullPage: true });
    // Podujatie 4 (i = 3) names the organizer's web site, Podujatie 2 (i = 1) does not (explore-hosts.mjs).
    const open = f.getByRole("button", { name: /^Otvoriť: Podujatie 4$/ });
    await open.scrollIntoViewIfNeeded();
    await open.click();
    const dialog = f.locator("dialog[open]");
    await expect(dialog).toBeVisible();
    const link = dialog.getByRole("link", { name: "Mestské kultúrne stredisko" });
    await expect(link).toHaveAttribute("href", "https://www.mksbb.sk/");
    await expect(link).toHaveAttribute("rel", "noopener noreferrer");
    await expect(link).toHaveAttribute("target", "_blank");
    await expect(dialog.getByText(/name: |url: /)).toHaveCount(0);
    await page.waitForTimeout(300);
    await page.screenshot({ path: path.join(shots, `organizer-${width}-card.png`) });
    await page.keyboard.press("Escape");
    await expect(f.locator("dialog[open]")).toHaveCount(0);
    const plain = f.getByRole("button", { name: /^Otvoriť: Podujatie 2$/ });
    await plain.scrollIntoViewIfNeeded();
    await plain.click();
    await expect(dialog).toBeVisible();
    await expect(dialog.getByText("Mestské kultúrne stredisko", { exact: true })).toBeVisible();
    await expect(dialog.getByRole("link", { name: "Mestské kultúrne stredisko" })).toHaveCount(0);
    await page.waitForTimeout(300);
    await page.screenshot({ path: path.join(shots, `organizer-${width}-card-no-url.png`) });
  });
}

// T-5196, DM-134: a model's default filter opens as the Filter panel's condition (T-6303) the reader can clear, with its page
// length and a numbered pager; nothing in the widget names the type.
for (const width of [1440, 400]) {
  test(`presented ${width}: the default filter fills the inputs, reset clears them`, async ({ page, context }) => {
    const requests = await serve(context);
    await page.setViewportSize({ width, height: 900 });
    await page.goto(`https://${CATALOGUE}/dataset/presented?lang=sk`);
    const f = frame(page);
    const grid = f.getByRole("grid");
    await expect(grid.locator("tbody tr")).toHaveCount(10, { timeout: 20000 });
    await expect.poll(() => requests.some((r) => r.url.searchParams.get("limit") === "10")).toBe(true);
    // The default filter opens as the Filter panel's condition, which the button counts (T-6303).
    await f.locator("summary").filter({ hasText: /^Filter\s*1$/ }).click();
    await expect(f.getByRole("combobox", { name: "Pole", exact: true })).toHaveValue("startDate");
    const op = f.getByRole("combobox", { name: "Operátor", exact: true });
    await expect(op).toHaveValue("gte");
    await expect.poll(() => requests.some((r) => /^startDate>=\d{4}-\d{2}-\d{2}$/.test(r.url.searchParams.get("q") ?? ""))).toBe(true);
    await expect(f.getByRole("button", { name: "Strana 2" })).toBeVisible();
    await page.screenshot({ path: path.join(shots, `presented-${width}-default.png`), fullPage: true });
    await page.keyboard.press("Escape");
    await f.getByRole("button", { name: "Zrušiť filtre" }).click();
    await expect(f.locator("summary").filter({ hasText: /^Filter$/ })).toBeVisible();
    await expect(op).toHaveCount(0);
    await expect(f.getByRole("button", { name: "Zrušiť filtre" })).toHaveCount(0);
    await page.screenshot({ path: path.join(shots, `presented-${width}-reset.png`), fullPage: true });
  });
}

// API/11 EP-164 (T-5197): the Endpoint's table start (filter, page length, columns) wins over the
// model's, member by member; reset clears it as it clears the model's.
for (const width of [1440, 400]) {
  test(`publishedtable ${width}: the Endpoint's table start wins over the model's`, async ({ page, context }) => {
    const requests = await serve(context);
    await page.setViewportSize({ width, height: 900 });
    await page.goto(`https://${CATALOGUE}/dataset/publishedtable?lang=sk`);
    const f = frame(page);
    const grid = f.getByRole("grid");
    await expect(grid).toBeVisible({ timeout: 20000 });
    await expect(grid.locator("tbody tr")).toHaveCount(20);
    const now = new Date();
    const week = new Date(now.getFullYear(), now.getMonth(), now.getDate() + 7);
    const day = `${week.getFullYear()}-${String(week.getMonth() + 1).padStart(2, "0")}-${String(week.getDate()).padStart(2, "0")}`;
    await expect.poll(() => requests.some((r) => r.url.searchParams.get("q") === `startDate>=${day}` && r.url.searchParams.get("limit") === "20")).toBe(true);
    const heads = (await grid.locator("thead tr").first().locator("th").allInnerTexts()).map((t) => t.replace(/[⋮↑↓]/g, "").trim());
    expect(heads).toEqual(["ID", "Typ", "Názov", "Začiatok"]);
    await page.screenshot({ path: path.join(shots, `publishedtable-${width}-default.png`), fullPage: true });
    const before = requests.length;
    await f.getByRole("button", { name: "Zrušiť filtre" }).click();
    await expect.poll(() => requests.slice(before).some((r) => r.url.pathname.endsWith("/ngsi-ld/v1/entities") && !r.url.searchParams.get("q"))).toBe(true);
    await page.screenshot({ path: path.join(shots, `publishedtable-${width}-reset.png`), fullPage: true });
  });
}

// DM-135 (T-5231): the default filter is a chip the reader removes; the choice rides in the address,
// so a reload or a shared link opens without it, and "Obnoviť predvolené" brings it back.
for (const width of [1440, 400]) {
  test(`presented ${width}: the default-filter chip removes, survives a reload and restores`, async ({ page, context }) => {
    const requests = await serve(context);
    await page.setViewportSize({ width, height: 900 });
    await page.goto(`https://${CATALOGUE}/dataset/presented?lang=sk`);
    const f = frame(page);
    await expect(f.getByRole("grid").locator("tbody tr")).toHaveCount(10, { timeout: 20000 });
    await expect(f.getByText("Predvolený filter: Začiatok ≥ dnes")).toBeVisible();
    const remove = f.getByRole("button", { name: "Odstrániť predvolený filter Začiatok ≥ dnes" });
    const box = await remove.boundingBox();
    expect(box.width).toBeGreaterThanOrEqual(44);
    expect(box.height).toBeGreaterThanOrEqual(44);
    await page.screenshot({ path: path.join(shots, `chip-${width}-default.png`), fullPage: true });
    const csv = f.getByRole("link", { name: /CSV/ });
    await expect(csv).toHaveAttribute("href", /q=startDate/);
    const before = requests.length;
    await remove.focus();
    await page.keyboard.press("Enter");
    await expect(remove).toHaveCount(0);
    await expect.poll(() => requests.slice(before).some((r) => r.url.pathname.endsWith("/ngsi-ld/v1/entities") && !r.url.searchParams.get("q"))).toBe(true);
    await expect(csv).not.toHaveAttribute("href", /q=/);
    const inner = page.frame({ url: /explore\.html/ });
    await expect.poll(() => inner.evaluate(() => location.hash)).toMatch(/&all(&|$)/);
    await page.screenshot({ path: path.join(shots, `chip-${width}-removed.png`), fullPage: true });
    // A reload of the same address keeps the reader's choice.
    const hash = await inner.evaluate(() => location.hash);
    const after = requests.length;
    await inner.evaluate((h) => { location.hash = h; location.reload(); }, hash);
    await expect(frame(page).getByRole("button", { name: "Obnoviť predvolené" })).toBeVisible({ timeout: 20000 });
    await expect(frame(page).getByText("Predvolený filter: Začiatok ≥ dnes")).toHaveCount(0);
    expect(requests.slice(after).filter((r) => r.url.pathname.endsWith("/ngsi-ld/v1/entities") && r.headers.accept !== "application/geo+json").every((r) => !r.url.searchParams.get("q"))).toBe(true);
    await frame(page).getByRole("button", { name: "Obnoviť predvolené" }).click();
    await expect(frame(page).getByText("Predvolený filter: Začiatok ≥ dnes")).toBeVisible();
    await expect.poll(() => page.frame({ url: /explore\.html/ }).evaluate(() => location.hash)).not.toMatch(/&all(&|$)/);
  });
}
