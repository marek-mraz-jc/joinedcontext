/**
 * T-1051, UI-01: every button has a name a screen reader can read.
 *
 * A button whose only child is an icon reads as "button" and nothing else, which is how a
 * close, a remove or a view switch becomes unusable without sight. This walks the source
 * rather than one rendered dialog, because the invariant is about every button the Portal has
 * and a rendered test only covers the ones its fixture happens to open.
 */
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

function sources(dir: string): string[] {
  return readdirSync(dir).flatMap((entry) => {
    const path = join(dir, entry);
    if (statSync(path).isDirectory()) {
      return sources(path);
    }
    return path.endsWith(".tsx") ? [path] : [];
  });
}

/** A `<button>` or `<Button>` element with its attributes and its children. */
const ELEMENT = /<(button|Button)\b((?:[^>]|\n)*?)>((?:(?!<\/?\1\b)[\s\S])*?)<\/\1>/g;

/** What is left of the children once tags and expressions are gone: the literal text. */
function literalText(body: string): string {
  return body
    .replace(/<[^>]*>/g, "")
    .replace(/\{[^{}]*\}/g, "")
    .trim();
}

describe("every button says what it is", () => {
  it("has a name: its own text, a translated expression, or an aria-label", () => {
    const unnamed: string[] = [];
    for (const path of sources("src")) {
      const text = readFileSync(path, "utf8");
      for (const match of text.matchAll(ELEMENT)) {
        const [, , attributes, body] = match;
        const labelled = ["aria-label", "aria-labelledby", "title="].some((key) =>
          attributes.includes(key),
        );
        const speaks = literalText(body) !== "" || body.includes("{");
        if (!labelled && !speaks) {
          unnamed.push(`${path}:${text.slice(0, match.index).split("\n").length}`);
        }
      }
    }
    expect(unnamed).toEqual([]);
  });

  it("requires a name on the one close button every dialog shares", () => {
    const dialog = readFileSync("src/components/ui/Dialog.tsx", "utf8");
    // Not optional: a dialog cannot be opened without saying what its close button is called.
    expect(dialog).toContain("closeLabel: string;");
    expect(dialog).toContain("aria-label={closeLabel}");
  });
});
