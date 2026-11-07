/** AP-136 (T-2795): the chip of the App probe's verdict, in words first and colour second. */
import { cleanup, render, screen } from "@testing-library/react";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import de from "../src/locales/de.json";
import { AppCheckChip } from "../src/pages/apps/AppCheckChip";
import type { AppCheck } from "../src/pages/apps/AppCheckChip";

const AT = "2026-09-25T09:00:00Z";

function chip(check: AppCheck | undefined) {
  return render(
    <I18nextProvider i18n={i18n}>
      <AppCheckChip check={check} />
    </I18nextProvider>,
  );
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(cleanup);

describe("the App check chip", () => {
  it.each([
    ["green", en.apps.check.green, "success"],
    ["red", en.apps.check.red, "danger"],
    ["amber", en.apps.check.amber, "warning"],
  ] as const)("says %s in words and colours it to match (AP-136)", (state, words, tone) => {
    chip({ name: "a", state, at: AT });
    const badge = screen.getByText(words);
    expect(badge.className).toContain(`text-${tone}`);
    expect(screen.getByText(/^Checked /)).toBeInTheDocument();
  });

  it("names the reason of a failure beside the time", () => {
    chip({ name: "a", state: "red", at: AT, reason: "3 console errors" });
    expect(screen.getByText(/^Checked .+: 3 console errors$/)).toBeInTheDocument();
  });

  it("draws nothing before the first check", () => {
    const { container } = chip(undefined);
    expect(container).toBeEmptyDOMElement();
  });

  it("speaks the reader's language", async () => {
    await i18n.changeLanguage("de");
    chip({ name: "a", state: "amber", at: AT });
    expect(screen.getByText(de.apps.check.amber)).toBeInTheDocument();
    expect(screen.getByText(/^Geprüft /)).toBeInTheDocument();
  });
});
