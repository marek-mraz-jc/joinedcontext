// covers (T-2137, the module gate in gate_modules.test.ts): src/pages/glossary/GlossaryPage.tsx,
// the platform's words with one example each (T-3236, UI-45), the page every Term links to.
import { render, screen, within } from "@testing-library/react";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it } from "vitest";
import i18n, { SUPPORTED_LOCALES } from "../src/i18n";
import { TERMS } from "../src/components/ui";
import en from "../src/locales/en.json";
import { GlossaryPage } from "../src/pages/glossary/GlossaryPage";
import { expectNoRawKeys, expectNoViolations } from "./checks";

const show = () =>
  render(
    <I18nextProvider i18n={i18n}>
      <GlossaryPage />
    </I18nextProvider>,
  );

describe("the glossary page (T-3236)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("names every word once, with its definition and an example, each at its own anchor", async () => {
    const { container } = show();
    expect(screen.getByRole("heading", { level: 1, name: en.glossary.page.title })).toBeInTheDocument();
    for (const name of TERMS) {
      const entry = container.querySelector(`#term-${name}`) as HTMLElement;
      const words = (en.glossary as unknown as Record<string, { term: string; definition: string; example: string }>)[name];
      expect(within(entry).getByText(words.term)).toBeInTheDocument();
      expect(within(entry).getByText(words.definition)).toBeInTheDocument();
      expect(within(entry).getByText(`Example: ${words.example}`)).toBeInTheDocument();
    }
    await expectNoViolations(container);
  });

  it.each(SUPPORTED_LOCALES)("says everything in %s with no raw key", async (locale) => {
    await i18n.changeLanguage(locale);
    const { container } = show();
    expectNoRawKeys(container);
  });
});
