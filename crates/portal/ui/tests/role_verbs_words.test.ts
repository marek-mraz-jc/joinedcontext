import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

// T-3848: the role form offers the caller's own verbs (`api/permissions.ts` `Verb`), not only
// ROLE_VERBS, and a verb with no word showed its raw key `choice.verb.update` on the live walk.
const VERBS = readFileSync("src/api/permissions.ts", "utf8")
  .match(/export type Verb = ([^;]+);/)?.[1]
  .match(/"([a-z]+)"/g)
  ?.map((quoted) => quoted.slice(1, -1));

describe("role verbs", () => {
  it.each(["en", "sk", "cs", "de"])("every verb has a word in %s", (locale) => {
    const words = JSON.parse(readFileSync(`src/locales/${locale}.json`, "utf8")).choice.verb;
    expect(VERBS?.length).toBeGreaterThan(3);
    expect(VERBS?.filter((verb) => !words[verb])).toEqual([]);
  });
});
