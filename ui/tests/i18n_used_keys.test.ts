/**
 * T-2813, UI-05: every translation key the code names is in the catalogue.
 *
 * i18next answers a missing key with the key itself, so `t("app.close")` rendered the words
 * `app.close` as the accessible name of every confirm dialog's close button, and `app.cancel` as
 * its visible Cancel. `expectNoRawKeys` reads the text on the screen of the pages it renders; this
 * reads the source, so a key in an `aria-label`, a `title` or a page no test renders is caught
 * too. Only literal keys are read: a key built at run time (`t(\`nav.${plural}\`)`) carries its
 * own `defaultValue` or is checked by the suite of the page that builds it.
 */
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative } from "node:path";
import { describe, expect, it } from "vitest";
import en from "../src/locales/en.json";

const SRC = join(__dirname, "../src");

function sources(dir: string): string[] {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) return sources(path);
    return /\.(ts|tsx)$/.test(name) && !/\.test\.tsx?$/.test(name) ? [path] : [];
  });
}

/** `t("a.b")`, `t('a.b', …)` and `i18n.t("a.b")`; a key needs at least one dot to be one. */
const LITERAL = /\bt\(\s*["']([a-zA-Z][\w]*(?:\.[\w-]+)+)["']/g;

function has(key: string): boolean {
  let node: unknown = en;
  for (const part of key.split(".")) {
    if (node === null || typeof node !== "object" || !(part in node)) return false;
    node = (node as Record<string, unknown>)[part];
  }
  return true;
}

/** An ICU plural lives under the key itself; an i18next one under `_one`/`_other`. */
const exists = (key: string) => has(key) || has(`${key}_one`) || has(`${key}_other`);

describe("the keys the code names (T-2813)", () => {
  it("finds literal keys at all, so an empty scan cannot pass", () => {
    const count = sources(SRC).reduce(
      (sum, file) => sum + [...readFileSync(file, "utf8").matchAll(LITERAL)].length,
      0,
    );
    expect(count).toBeGreaterThan(500);
  });

  it("names no key the English catalogue does not hold", () => {
    const missing = sources(SRC).flatMap((file) => {
      const text = readFileSync(file, "utf8");
      return [...text.matchAll(LITERAL)]
        .filter((match) => !exists(match[1]))
        .map((match) => {
          const line = text.slice(0, match.index).split("\n").length;
          return `${relative(SRC, file)}:${line} ${match[1]}`;
        });
    });
    expect(missing, "a key missing from en.json renders as the key itself").toEqual([]);
  });
});
