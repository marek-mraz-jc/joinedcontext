/**
 * UI-44, T-1610: a page with nothing in it says what lives there and what the first step is.
 *
 * Measured on main on 2026-09-19: of 22 empty states in the UI, three carried both a sentence and an
 * action and fifteen carried neither — a heading and an empty table teach nobody what the page is
 * for. This test reads the sources, so a page added later with a bare empty state fails here instead
 * of on dev, and it is the survey of T-1610 kept runnable.
 *
 * Four states are deliberately without a sentence, each for a reason named below: they are not empty
 * pages but filtered ones, where the first step is to clear the filter and a paragraph about the kind
 * would be in the way.
 */
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

/** An empty state that says nothing beyond its title, and why that is right. */
const WITHOUT_A_SENTENCE: Record<string, string> = {
  "approvals.noneMatch": "a filtered list, not an empty project: the step is to widen the filter",
  "workspaces.compare.empty":
    "a comparison of one copy against main, which is empty because nothing was changed in it",
  "assistantPage.empty": "its own title says the first step: ask the assistant or start below",
};

function sources(dir: string, found: string[] = []): string[] {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) {
      sources(path, found);
    } else if (entry.name.endsWith(".tsx")) {
      found.push(path);
    }
  }
  return found;
}

interface Found {
  file: string;
  line: number;
  titleKey?: string;
  hasTitle: boolean;
  hasDescription: boolean;
}

/**
 * The component gallery is not a page of the Portal: it is the development-only route that shows
 * every shared component in every state, with literal sample text on purpose, and a production
 * build does not contain it (`src/router.tsx`, T-1729). Its specimens are demonstrations of the
 * empty state, not empty states a person can land on.
 */
const GALLERY = "src/pages/gallery/";

function emptyStates(): Found[] {
  const found: Found[] = [];
  for (const file of sources("src").filter((file) => !file.startsWith(GALLERY))) {
    const text = readFileSync(file, "utf8");
    for (const match of text.matchAll(/<EmptyState\b([\s\S]*?)\/>/g)) {
      const block = match[1];
      found.push({
        file,
        line: text.slice(0, match.index).split("\n").length,
        titleKey: /title=\{[^}]*t\("([^"]+)"\)/.exec(block)?.[1],
        hasTitle: /\btitle=/.test(block),
        hasDescription: block.includes("description"),
      });
    }
  }
  return found;
}

describe("a page with nothing in it", () => {
  it("is never a blank box: every empty state names itself", () => {
    const states = emptyStates();
    expect(states.length, "the UI has empty states to check").toBeGreaterThan(15);
    // A literal title is allowed, a state with no title at all is not.
    const unnamed = states
      .filter((state) => !state.hasTitle)
      .map((state) => `${state.file}:${state.line}`);
    expect(unnamed, "an empty state with no title").toEqual([]);
  });

  it("says what lives there, or is one of the filtered views that must not", () => {
    const silent = emptyStates()
      .filter((state) => !state.hasDescription)
      .filter((state) => !(state.titleKey && state.titleKey in WITHOUT_A_SENTENCE))
      .map((state) => `${state.file}:${state.line} (${state.titleKey ?? "literal title"})`);
    expect(silent, "an empty page that explains nothing").toEqual([]);
  });

  it("keeps the list of states that say nothing short and reasoned", () => {
    // Every exception carries its reason here; the list is allowed to shrink, not to grow quietly.
    expect(Object.keys(WITHOUT_A_SENTENCE).length).toBeLessThanOrEqual(4);
    for (const [key, why] of Object.entries(WITHOUT_A_SENTENCE)) {
      expect(why.length, `${key} needs a reason a reader can weigh`).toBeGreaterThan(30);
    }
  });
});
