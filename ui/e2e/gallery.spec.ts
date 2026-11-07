/**
 * The component gallery in a real browser, at three sizes (T-1729; UI-15, UI-16, UI-27).
 *
 * The gallery is a development route — a production build drops it — so this runs against the
 * dev server (the `gallery` project of `playwright.config.ts`), while every other spec runs
 * against the preview of `dist/`.
 *
 * Two things are asserted at each size. What a picture cannot say: nothing pushes the page
 * sideways, with a sixty-character German label on every specimen, and every specimen is on the
 * page — the measurement `visual.spec.ts` already makes at phone width. And the picture itself
 * (T-2413): one baseline per size, compared the way the page baselines are, so a token, a
 * radius, a focus ring or a spacing scale changed by hand is a red lane instead of something a
 * person notices in a recording.
 *
 * A baseline changes only on purpose, and the four PNGs under `gallery.spec.ts-snapshots/` are
 * reviewed like code. They are taken in the Playwright image ci-full.yml's `ui-gallery` job runs in,
 * never on a bare machine, whose fallback fonts move ~2 000 pixels (T-5575). From `ui/`, after
 * `pnpm install` here and in `../sdk`:
 *
 *   docker run --rm --ipc=host --user "$(id -u):$(id -g)" -e HOME=/tmp -e PW_ONLY=gallery \
 *     -v "$PWD/..:/repo" -w /repo/ui mcr.microsoft.com/playwright@<digest in ci.yml> \
 *     node node_modules/@playwright/test/cli.js test --update-snapshots
 *
 * The dark theme is `@media (prefers-color-scheme: dark)` in `src/tokens.css`, so it is a size
 * here like any other: the same page with `colorScheme: "dark"`.
 */
import { expect, test } from "@playwright/test";

const SIZES = [
  // The recording: 1920×1080 at zoom 1.5 lays the page out at 1280 CSS px.
  { name: "1920x1080-zoom1.5", viewport: { width: 1280, height: 720 }, deviceScaleFactor: 1.5, colorScheme: "light" as const },
  { name: "400px", viewport: { width: 400, height: 860 }, deviceScaleFactor: 1, colorScheme: "light" as const },
  // WCAG 1.4.4: the page at twice the text size, laid out at 640 CSS px.
  { name: "text-200", viewport: { width: 640, height: 900 }, deviceScaleFactor: 2, colorScheme: "light" as const },
  // The same controls in the other colour way (`prefers-color-scheme: dark`, src/tokens.css).
  { name: "dark", viewport: { width: 1280, height: 720 }, deviceScaleFactor: 1, colorScheme: "dark" as const },
];

for (const size of SIZES) {
  test.describe(size.name, () => {
    test.use({
      viewport: size.viewport,
      deviceScaleFactor: size.deviceScaleFactor,
      colorScheme: size.colorScheme,
    });

    test(`every control is drawn inside the page at ${size.name}`, async ({ page }) => {
      await page.route("**/api/v1/**", (route) =>
        route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({ grants: [], bootstrap: true }),
        }),
      );
      await page.goto("/__gallery?lang=en", { waitUntil: "networkidle" });
      await expect(page.getByRole("heading", { level: 1, name: "Component gallery" })).toBeVisible();
      await page.evaluate(() => document.fonts.ready);

      // A page scrolls down, never sideways (UI-27) — a sixty-character German label is what
      // pushes a row out of the page, and it is on every specimen.
      const { overflow, offenders } = await page.evaluate(() => {
        const doc = document.documentElement;
        // An element inside a horizontal scroller is not what pushes the page sideways — a wide
        // table in its own `overflow-x-auto` is the design (`components/ui/Table.tsx`). Naming
        // those as offenders sent two sessions after a table that was never the cause, so each
        // one now says how far past the edge it reaches and whether anything clips it.
        const clipped = (element: HTMLElement) => {
          for (let node = element.parentElement; node; node = node.parentElement) {
            if (getComputedStyle(node).overflowX !== "visible") {
              return true;
            }
          }
          return false;
        };
        // Everything drawn, not `main *`: the element that widened the page once turned out to be
        // outside `main` and the run before it was `position: fixed`, whose `offsetParent` is
        // null — both invisible to the old scan, which then blamed the table it could see.
        const wide: string[] = [];
        document.querySelectorAll<HTMLElement>("body *").forEach((element) => {
          const box = element.getBoundingClientRect();
          if (box.width === 0 && box.height === 0) {
            return;
          }
          if (box.right > doc.clientWidth + 1) {
            const past = Math.round(box.right - doc.clientWidth);
            const where = clipped(element) ? "inside a scroller" : "NOT CLIPPED";
            const id = element.id ? `#${element.id}` : "";
            wide.push(
              `${element.tagName}${id}.${String(element.className).slice(0, 40)} +${past}px ${getComputedStyle(element).position} ${where}`,
            );
          }
        });
        // The unclipped ones first, widest first: those are the page's own width, the rest are
        // context. A list that is all "inside a scroller" means the cause is none of them.
        const past = (entry: string) => Number(/\+(\d+)px/.exec(entry)?.[1] ?? 0);
        wide.sort(
          (a, b) =>
            Number(b.includes("NOT CLIPPED")) - Number(a.includes("NOT CLIPPED")) ||
            past(b) - past(a),
        );
        return { overflow: doc.scrollWidth - doc.clientWidth, offenders: wide.slice(0, 8) };
      });
      // Two runs listed only elements inside a scroller, which means no border box reaches past
      // the edge at all and the width comes from something a rect scan cannot see: a margin box,
      // a pseudo-element, a transform. So when the page is wide, ask the page itself — hide one
      // subtree at a time and keep the one whose absence takes the overflow away.
      const culprit =
        overflow > 1
          ? await page.evaluate(() => {
              const doc = document.documentElement;
              const excess = () => doc.scrollWidth - doc.clientWidth;
              let node = document.body;
              // The tree is walked downwards, so the guard is depth and not breadth; 200 is far
              // past any page and keeps a layout that re-widens itself from looping.
              for (let depth = 0; depth < 200; depth += 1) {
                const child = Array.from(node.children).find((candidate) => {
                  const element = candidate as HTMLElement;
                  const was = element.style.display;
                  element.style.display = "none";
                  const gone = excess() <= 1;
                  element.style.display = was;
                  return gone;
                }) as HTMLElement | undefined;
                if (!child) {
                  break;
                }
                node = child;
              }
              const style = getComputedStyle(node);
              const before = getComputedStyle(node, "::before");
              const after = getComputedStyle(node, "::after");
              return (
                `${node.tagName}${node.id ? `#${node.id}` : ""}` +
                `.${String(node.className).slice(0, 60)}` +
                ` right=${Math.round(node.getBoundingClientRect().right)} of ${doc.clientWidth}` +
                ` margin-right=${style.marginRight} position=${style.position}` +
                ` transform=${style.transform} width=${style.width} min-width=${style.minWidth}` +
                ` ::before=${before.content}/${before.width} ::after=${after.content}/${after.width}`
              );
            })
          : "";
      expect(
        overflow,
        `the gallery scrolls sideways by ${overflow}px.\n  hiding this takes it away: ${culprit}\n  past the edge: ${offenders.join(" | ")}`,
      ).toBeLessThanOrEqual(1);

      // Every specimen is on the page: one section per component, each with its heading.
      const sections = await page.getByRole("region").count();
      expect(sections).toBeGreaterThan(8);

      // UI-15, UI-16, UI-27: the gallery as a person sees it. `fullPage`, because the point is
      // every specimen, not the first screen of them.
      await expect(page).toHaveScreenshot(`gallery-${size.name}.png`, {
        fullPage: true,
        animations: "disabled",
        caret: "hide",
        // Pixels, not a ratio of the page (T-5575): 2 px more padding on the shared Button moved
        // 5 758 to 14 563 pixels per size, under 1 % of the full page, so the old 3 % ratio let it
        // through. The bare ubuntu-latest runner (ci-full e2e) and the ui-gallery image missed the
        // older sandbox baselines by the same 2 041 to 2 278 pixels, within 4 of each other.
        maxDiffPixels: 1500,
      });
    });
  });
}

/**
 * The failure that only ever appeared on the CI runner (T-2446), as a test that fails anywhere.
 *
 * A badge holding an entity id is one token with no space in it, so its minimum width is the
 * whole string and every ancestor inherits it. Where the monospace font draws a shade wider —
 * the runner, not this machine — that minimum passed what the viewport had and the page scrolled
 * sideways against UI-27. Widening the font here is what makes the difference reproducible: the
 * pill wraps instead of growing, and nothing below it moves.
 */
test.describe("a label with no spaces in it", () => {
  test.use({ viewport: { width: 400, height: 900 } });

  test("wraps instead of pushing the page sideways", async ({ page }) => {
    await page.route("**/api/v1/**", (route) =>
      route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ grants: [], bootstrap: true }),
      }),
    );
    await page.goto("/__gallery?lang=en", { waitUntil: "networkidle" });
    await expect(page.getByRole("heading", { level: 1, name: "Component gallery" })).toBeVisible();
    await page.evaluate(() => document.fonts.ready);

    // Wider than any machine draws it, so the test does not depend on whose font is installed.
    await page.addStyleTag({ content: ".font-mono { letter-spacing: 0.08em; font-size: 115% }" });
    const overflow = await page.evaluate(() => {
      const doc = document.documentElement;
      return doc.scrollWidth - doc.clientWidth;
    });
    expect(
      overflow,
      `an id in a badge pushed the page ${overflow}px sideways when the font drew wider`,
    ).toBeLessThanOrEqual(1);
  });
});
