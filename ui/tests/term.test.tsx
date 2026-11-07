/**
 * UI-45: the platform's own words carry their definition where a person meets them.
 *
 * Measured on main on 2026-09-20: `Endpoint`, `Policy`, `Context Space` and `Risk lane` stand in
 * table headers and section headings of eleven pages, `docs/Glossary.md` defines every one of
 * them, and nothing on the screen ever did (T-1609).
 *
 * The definition is in the document whether or not it is on screen, because `aria-describedby`
 * cannot point at an element that is not rendered — that is what these cases hold.
 */
// covers (T-2137, the module gate in gate_modules.test.ts): the cases in this file drive
// src/components/ui/Term.tsx through the page they belong to; each was confirmed by
// making the module throw and watching this file go red.
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join } from "node:path";
import { I18nextProvider } from "react-i18next";
import { describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import { TERMS, Term } from "../src/components/ui";
import cs from "../src/locales/cs.json";
import de from "../src/locales/de.json";
import en from "../src/locales/en.json";
import sk from "../src/locales/sk.json";

function show(name: (typeof TERMS)[number] = "endpoint", label?: string) {
  return render(
    <I18nextProvider i18n={i18n}>
      <Term name={name}>{label}</Term>
    </I18nextProvider>,
  );
}

describe("a domain word and its definition", () => {
  it("names the definition from the word, so a screen reader reads both", () => {
    show("lane", "Risk lane");
    const word = screen.getByRole("term");
    const described = word.getAttribute("aria-describedby");
    expect(described).toBeTruthy();
    const definition = document.getElementById(described!);
    expect(definition?.textContent).toBe(en.glossary.lane.definition);
  });

  it("shows the definition on keyboard focus and hides it again on blur", async () => {
    const person = userEvent.setup();
    show("contextSpace", "Context Space");
    const word = screen.getByRole("term");
    const definition = document.getElementById(word.getAttribute("aria-describedby")!)!;

    expect(definition.className).toContain("sr-only");
    await person.tab();
    expect(word).toHaveFocus();
    expect(definition.className).not.toContain("sr-only");
    await person.tab();
    expect(definition.className).toContain("sr-only");
  });

  it("closes the definition on Escape without moving the focus", async () => {
    const person = userEvent.setup();
    show("policy", "Policies");
    const word = screen.getByRole("term");
    const definition = document.getElementById(word.getAttribute("aria-describedby")!)!;

    await person.tab();
    expect(definition.className).not.toContain("sr-only");
    await person.keyboard("{Escape}");
    expect(definition.className).toContain("sr-only");
    expect(word).toHaveFocus();
  });

  it("writes the page's own word and falls back to the bundle's spelling", () => {
    const { unmount } = show("endpoint", "Endpoints");
    expect(screen.getByRole("term").textContent).toBe("Endpoints");
    unmount();
    show("endpoint");
    expect(screen.getByRole("term").textContent).toBe(en.glossary.endpoint.term);
  });

  it("defines every word in all four languages, each in its own words", () => {
    for (const name of TERMS) {
      const written = [en, sk, cs, de].map(
        (bundle) =>
          (bundle as unknown as Record<string, Record<string, { definition: string }>>)
            .glossary[name].definition,
      );
      for (const definition of written) {
        expect(definition.length, `${name} is defined in one sentence`).toBeGreaterThan(30);
      }
      // Four locales that are one string are one locale: the text was copied, not translated.
      expect(new Set(written).size, `${name} is translated, not copied`).toBe(4);
    }
  });

  // T-3236: every word has an example, and its whole entry is one step away.
  it("gives every word an example in all four languages", () => {
    for (const name of TERMS) {
      for (const bundle of [en, sk, cs, de]) {
        const entry = (bundle as unknown as Record<string, Record<string, { example?: string }>>).glossary[name];
        expect(entry.example?.length ?? 0, `${name} has an example`).toBeGreaterThan(10);
      }
    }
  });

  it("links the word to its glossary entry, named for a screen reader", () => {
    show("entityType");
    const link = screen.getByRole("link", { name: "What is Entity type? Open the glossary" });
    expect(link).toHaveAttribute("href", "/glossary#term-entityType");
    // The word keeps its own definition; the link is beside it, not inside it.
    expect(screen.getByRole("term")).not.toContainElement(link);
  });
});

/** Every `.tsx` under `ui/src`, which is where a `Term` can be placed. */
function sources(directory: string, found: string[] = []): string[] {
  for (const entry of readdirSync(directory)) {
    const path = join(directory, entry);
    if (statSync(path).isDirectory()) {
      sources(path, found);
    } else if (path.endsWith(".tsx")) {
      found.push(path);
    }
  }
  return found;
}

describe("where a domain word may stand", () => {
  const files = sources(join(__dirname, "..", "src"));

  it("puts no word inside a button, a link or a label", () => {
    // A focusable inside a focusable is unreachable by keyboard in one of the two, and the
    // definition would be folded into the control's own accessible name.
    const enclosing = /<(Button|button|a|Link|label)\b[^>]*>([\s\S]*?)<\/\1>/g;
    const offending: string[] = [];
    for (const file of files.filter((path) => !path.endsWith("Term.tsx"))) {
      const source = readFileSync(file, "utf8");
      for (const [, tag, inside] of source.matchAll(enclosing)) {
        if (inside.includes("<Term")) {
          offending.push(`${file}: <Term> inside <${tag}>`);
        }
      }
      // `Field` renders a real `<label htmlFor>`, so its label prop is the same trap as the
      // element; a `<dt>` or a heading that happens to be called `label` is not.
      if (/<Field[^>]*\slabel=\{[^}]*<Term/.test(source)) {
        offending.push(`${file}: <Term> in a Field label`);
      }
    }
    expect(offending).toEqual([]);
  });

  it("is used where the pages write the words, and only with a word the bundles define", () => {
    const used = new Map<string, number>();
    for (const file of files.filter((path) => !path.endsWith("Term.tsx"))) {
      for (const [, name] of readFileSync(file, "utf8").matchAll(/<Term\s+name="([^"]+)"/g)) {
        used.set(name, (used.get(name) ?? 0) + 1);
      }
    }
    expect([...used.keys()].filter((name) => !TERMS.includes(name as never))).toEqual([]);
    // The four words the pages write as a heading or a column of their own. `Change` is written
    // only inside sentences, which a tooltip cannot reach without splitting the sentence.
    // `LinkML` is written only on the Model editor's source tab, a button (T-3236); the glossary
    // page explains it.
    expect([...used.keys()].sort()).toEqual(["contextSpace", "endpoint", "entityType", "lane", "ngsiLd", "policy"]);
  });
});
