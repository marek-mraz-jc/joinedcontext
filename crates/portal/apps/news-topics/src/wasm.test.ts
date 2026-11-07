import { describe, expect, it } from "vitest";
import { initSync, analyse } from "../wasm/pkg/news_topics_wasm.js";
import module from "../wasm/pkg/news_topics_wasm_bg.wasm?inline";
import { ARTICLES } from "./fixtures/news";

// The module the build lane compiled (wasm/pkg), its bytes as Vite inlines them.
initSync({ module: Uint8Array.from(atob(module.slice(module.indexOf(",") + 1)), (c) => c.charCodeAt(0)) });

interface TopicOutput {
  id: number;
  keywords: Array<{ term: string; weight: number }>;
  articles: string[];
  share: number;
}

interface WeekOutput {
  week: string;
  shares: number[];
}

interface AnalyseOutput {
  topics: TopicOutput[];
  weeks: WeekOutput[];
  unassigned: string[];
  error?: string;
}

function toArticleInput(row: (typeof ARTICLES)[number]) {
  const nameVal = row.name;
  const title =
    typeof nameVal === "object" && nameVal !== null && "languageMap" in nameVal
      ? (nameVal.languageMap as Record<string, string>).en
      : String(nameVal ?? "");
  const descVal = row.description;
  const summary =
    typeof descVal === "object" && descVal !== null && "languageMap" in descVal
      ? (descVal.languageMap as Record<string, string>).en
      : typeof descVal === "string"
        ? descVal
        : undefined;
  const published = typeof row.datePublished === "string" ? row.datePublished : undefined;
  return {
    id: row.id,
    title,
    ...(summary ? { summary } : {}),
    ...(published ? { published } : {}),
  };
}

describe("WASM topic analysis", () => {
  it("clusters news fixtures into ranked topics with keywords and weekly shares", () => {
    const articles = ARTICLES.map(toArticleInput);
    const input = JSON.stringify({ articles, k: 5, seed: 1 });
    const output: AnalyseOutput = JSON.parse(analyse(input));

    expect(output.error).toBeUndefined();
    expect(Array.isArray(output.topics)).toBe(true);
    expect(output.topics.length).toBe(5);

    // Topics are ranked by article count descending, renumbered 0..4
    for (let i = 0; i < output.topics.length; i++) {
      const topic = output.topics[i];
      expect(topic.id).toBe(i);
      expect(Array.isArray(topic.keywords)).toBe(true);
      expect(topic.keywords.length).toBeGreaterThan(0);
      expect(topic.keywords.length).toBeLessThanOrEqual(6);
      for (const kw of topic.keywords) {
        expect(typeof kw.term).toBe("string");
        expect(kw.term.length).toBeGreaterThanOrEqual(3);
        expect(typeof kw.weight).toBe("number");
        expect(kw.weight).toBeGreaterThan(0);
      }
      expect(Array.isArray(topic.articles)).toBe(true);
      expect(topic.articles.length).toBeGreaterThan(0);
      expect(typeof topic.share).toBe("number");
      expect(topic.share).toBeGreaterThan(0);

      if (i > 0) {
        expect(output.topics[i - 1].articles.length).toBeGreaterThanOrEqual(topic.articles.length);
      }
    }

    // Overall topic shares should sum to 1.0 (within float tolerance)
    const totalShare = output.topics.reduce((sum, t) => sum + t.share, 0);
    expect(totalShare).toBeCloseTo(1, 2);

    // Weeks are sorted in ISO format, and non-empty weeks sum to 1.0
    expect(Array.isArray(output.weeks)).toBe(true);
    expect(output.weeks.length).toBeGreaterThanOrEqual(6);

    for (let i = 0; i < output.weeks.length; i++) {
      const week = output.weeks[i];
      expect(week.week).toMatch(/^\d{4}-W\d{2}$/);
      expect(week.shares.length).toBe(output.topics.length);

      if (i > 0) {
        expect(week.week.localeCompare(output.weeks[i - 1].week)).toBeGreaterThan(0);
      }

      const weekSum = week.shares.reduce((a, b) => a + b, 0);
      if (weekSum > 0) {
        expect(weekSum).toBeCloseTo(1, 2);
      } else {
        expect(weekSum).toBe(0);
      }
    }
  });

  it("returns an error object for malformed JSON input without panicking", () => {
    const invalidJson = "{ malformed json: true ";
    const res1 = JSON.parse(analyse(invalidJson));
    expect(res1.error).toBeDefined();
    expect(typeof res1.error).toBe("string");

    const nonObject = analyse("null");
    const res2 = JSON.parse(nonObject);
    expect(res2.error).toBeDefined();

    const emptyString = analyse("");
    const res3 = JSON.parse(emptyString);
    expect(res3.error).toBeDefined();
  });

  it("handles empty articles list without panicking", () => {
    const res: AnalyseOutput = JSON.parse(analyse(JSON.stringify({ articles: [], k: 5, seed: 1 })));
    expect(res.topics).toEqual([]);
    expect(res.weeks).toEqual([]);
    expect(res.unassigned).toEqual([]);
  });

  it("produces deterministic output for the same seed", () => {
    const articles = ARTICLES.map(toArticleInput);
    const input = JSON.stringify({ articles, k: 5, seed: 42 });
    const run1 = analyse(input);
    const run2 = analyse(input);
    expect(run1).toBe(run2);
  });
});
