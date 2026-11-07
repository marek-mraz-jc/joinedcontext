import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import cs from "../src/locales/cs.json";
import de from "../src/locales/de.json";
import en from "../src/locales/en.json";
import sk from "../src/locales/sk.json";
import { ACTIVITY_KINDS } from "../src/api/activity";

/** The label `activity.kinds.<kind>` names in one locale, or nothing. */
function label(locale: unknown, kind: string): unknown {
  return ["activity", "kinds", ...kind.split(".")].reduce<unknown>(
    (node, key) => (typeof node === "object" && node !== null ? (node as Record<string, unknown>)[key] : undefined),
    locale,
  );
}

/**
 * Every kind the Portal server knows: the ingest vocabulary `KINDS` other services post through,
 * and the kinds the Portal writes itself past that door (`kind: "x".to_owned()` on an
 * ActivityEvent, a knowledge `record(.., "x", ..)` call or its `let kind = "x"`).
 */
function serverKinds(): string[] {
  const root = join(__dirname, "../../src");
  const found = new Set<string>();
  const patterns = [
    /\bkind: "([a-z]+[.-][a-z.-]*)"\.to_(?:owned|string)\(\)/g,
    /\brecord\(\s*&state,\s*&project,\s*"([a-z]+[.-][a-z.-]*)"/g,
    /\blet kind = "([a-z]+[.-][a-z.-]*)";/g,
  ];
  for (const entry of readdirSync(root, { recursive: true, encoding: "utf8" })) {
    if (!entry.endsWith(".rs")) continue;
    const text = readFileSync(join(root, entry), "utf8").split("#[cfg(test)]")[0];
    for (const pattern of patterns) {
      for (const match of text.matchAll(pattern)) found.add(match[1]);
    }
  }
  const vocabulary = /pub const KINDS: &\[&str\] = &\[([^\]]*)\]/.exec(
    readFileSync(join(root, "activity.rs"), "utf8"),
  );
  for (const match of vocabulary?.[1].matchAll(/"([^"]+)"/g) ?? []) found.add(match[1]);
  return [...found].sort();
}

describe("ACTIVITY_KINDS", () => {
  // T-4968: knowledge.file.uploaded was written by the Portal but missing here, so the feed showed
  // the raw key, the filter could not pick it and the live tail never heard it.
  it("holds every kind the Portal server writes", () => {
    const kinds = serverKinds();
    expect(kinds).toContain("knowledge.file.uploaded");
    expect(kinds).toContain("personal-data");
    expect(kinds).toContain("pipeline.throughput");
    for (const kind of kinds) expect(ACTIVITY_KINDS, kind).toContain(kind);
  });

  // T-3065: the live tail listens for these kinds alone and the filter offers them alone, so a
  // kind missing here (person.changed was) never reaches the feed as it happens.
  it("holds the person and model key kinds the Portal writes to the org project", () => {
    expect(ACTIVITY_KINDS).toContain("person.changed");
    expect(ACTIVITY_KINDS).toContain("model.key");
    expect(new Set(ACTIVITY_KINDS).size).toBe(ACTIVITY_KINDS.length);
  });

  it("names every kind in every language", () => {
    for (const [name, locale] of Object.entries({ cs, de, en, sk })) {
      for (const kind of ACTIVITY_KINDS) {
        expect(typeof label(locale, kind), `${name}: activity.kinds.${kind}`).toBe("string");
      }
    }
  });
});
