import { describe, expect, it } from "vitest";
import type { LanguageMap } from "../src/components/forms/uischema";
import { shippedForms } from "../src/schemas/forms";

/**
 * The words every shipped form shows (T-4824): help that ends where its sentence ends, in each of
 * the four languages, and the identifier first. A bare ` #` in a YAML value starts a comment, so
 * "+ for one level, # for every level below." reached the form as "+ for one level," and nothing
 * else noticed.
 */
const LANGUAGES = ["en", "sk", "cs", "de"];
const SENTENCE_END = /[.!?:)…"»]$/;
const IDENTIFIERS = new Set(["name", "title", "domain"]);

function texts(value: LanguageMap | undefined): [string, string][] {
  if (value === undefined) return [];
  return typeof value === "string" ? [["en", value]] : Object.entries(value);
}

describe.each(shippedForms.map((form) => [form.metadata?.name ?? form.spec.for, form] as const))("%s form", (_, form) => {
  const { spec } = form;

  it("ends every help and group text with its sentence, in every language", () => {
    const cut = [
      ...Object.entries(spec.fields ?? {}).flatMap(([field, arrangement]) =>
        texts(arrangement.help).map(([lang, text]) => [`${field} ${lang}`, text.trim()] as const),
      ),
      ...(spec.groups ?? []).flatMap((group, i) =>
        texts(group.description).map(([lang, text]) => [`group ${i} ${lang}`, text.trim()] as const),
      ),
    ].filter(([, text]) => !SENTENCE_END.test(text));
    expect(cut).toEqual([]);
  });

  it("writes every help in each of the four languages", () => {
    const missing = Object.entries(spec.fields ?? {})
      .filter(([, arrangement]) => arrangement.help !== undefined && typeof arrangement.help !== "string")
      .flatMap(([field, arrangement]) =>
        LANGUAGES.filter((lang) => !(arrangement.help as Record<string, string>)[lang]?.trim()).map((lang) => `${field} ${lang}`),
      );
    expect(missing).toEqual([]);
  });

  it("asks for the identifier first", () => {
    const order = spec.order ?? [];
    expect(order.length === 0 || order.slice(0, 2).some((field) => IDENTIFIERS.has(field))).toBe(true);
  });

  it("keeps experimental fields in a folded group", () => {
    const folded = new Set((spec.groups ?? []).filter((group) => group.folded).flatMap((group) => group.fields));
    const exposed = Object.entries(spec.fields ?? {})
      .filter(([, arrangement]) => texts(arrangement.help).some(([, text]) => /experimental/i.test(text)))
      .map(([field]) => field)
      .filter((field) => !folded.has(field) && !spec.fields?.[field]?.advanced);
    expect(exposed).toEqual([]);
  });
});
