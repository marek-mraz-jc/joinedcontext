/**
 * T-1804: the language switcher against the UI contract (UI-15, UI-16, UI-27, UI-48).
 *
 * It had no test of any kind. What it has to hold: the trigger is the shared `Button` (one focus
 * ring, one size, one disabled state), the menu opens and is walked by keyboard alone, the
 * language in use is marked as current, choosing one switches the page, and the list is the
 * overlap between what the installation offers and what the bundle can actually say — a locale
 * with no strings would switch the Portal to keys.
 */
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n, { SUPPORTED_LOCALES } from "../src/i18n";
import { BrandingProvider, NEUTRAL_BRANDING } from "../src/branding";
import { LanguageSwitcher } from "../src/components/LanguageSwitcher";
import { expectNoRawKeys, expectNoViolations } from "./checks";

/** The installation's branding, which is where the offered languages come from. */
function stubBranding(offered: string[] | undefined) {
  vi.stubGlobal(
    "fetch",
    vi.fn(() =>
      Promise.resolve(
        new Response(
          JSON.stringify({
            ...NEUTRAL_BRANDING,
            languages: offered === undefined ? {} : { default: offered[0], offered },
          }),
          { status: 200, headers: { "Content-Type": "application/json" } },
        ),
      ),
    ),
  );
}

function show() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <BrandingProvider>
          <LanguageSwitcher />
        </BrandingProvider>
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

const trigger = () => screen.getByRole("button", { name: i18n.t("lang.label") });

describe("the language switcher against the UI contract", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("sk");
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("names the language in use, and says the button's purpose to a screen reader", async () => {
    stubBranding(["sk", "en", "de", "cs"]);
    show();

    const button = trigger();
    // The globe is what a phone-width header has room for; the name is what a wider one shows,
    // and the accessible name says it either way (UI-27).
    expect(button).toHaveAccessibleName(i18n.t("lang.label"));
    expect(button).toHaveTextContent(i18n.t("lang.sk"));
    // The shared Button, so the focus ring and the size come from one place.
    expect(button.className).toContain("focus-ring");
  });

  it("opens and is walked by keyboard alone, and switches the page's language", async () => {
    const user = userEvent.setup();
    stubBranding(["sk", "en", "de", "cs"]);
    show();

    trigger().focus();
    await user.keyboard("{Enter}");

    const items = await screen.findAllByRole("menuitem");
    expect(items).toHaveLength(4);
    // The language in use is marked as current, not merely ticked in colour.
    expect(screen.getByRole("menuitem", { name: i18n.t("lang.sk") })).toHaveAttribute(
      "aria-current",
      "true",
    );

    await user.keyboard("{ArrowDown}{Enter}");
    await waitFor(() => expect(i18n.language).not.toBe("sk"));
    expect(SUPPORTED_LOCALES).toContain(i18n.language);
  });

  it("has no axe violation, closed and open", async () => {
    const user = userEvent.setup();
    stubBranding(["sk", "en"]);
    const { container, baseElement } = show();

    await expectNoViolations(container);

    await user.click(trigger());
    // The menu is portalled out of the component, so axe reads the menu where it landed. The
    // document body is not read here: a fragment rendered without the app's shell has no
    // landmark around it, which is the page's own contract (T-1797), not this part's.
    await expectNoViolations(await screen.findByRole("menu"));
    expect(baseElement).toContainElement(screen.getByRole("menu"));
  });

  it("offers only the languages the bundle can speak", async () => {
    const user = userEvent.setup();
    // `fi` has no bundle: offering it would switch the Portal to keys.
    stubBranding(["sk", "fi"]);
    show();

    await user.click(trigger());
    const items = await screen.findAllByRole("menuitem");
    expect(items.map((item) => item.textContent)).toEqual([i18n.t("lang.sk")]);
  });

  it("keeps every language of the bundle when the installation names none it can speak", async () => {
    const user = userEvent.setup();
    // Finnish has no bundle, so the overlap is empty and the switcher falls back to all of them
    // rather than offering nothing at all.
    stubBranding(["fi"]);
    show();

    await user.click(trigger());
    const items = await screen.findAllByRole("menuitem");
    expect(items).toHaveLength(SUPPORTED_LOCALES.length);
  });

  it("offers English alone when the installation configured no languages", async () => {
    const user = userEvent.setup();
    // No `languages` at all is answered by the API as its own default, `offered: ["en"]`
    // (NEUTRAL_BRANDING), and the switcher says what the installation says.
    stubBranding(undefined);
    show();

    await user.click(trigger());
    const items = await screen.findAllByRole("menuitem");
    expect(items.map((item) => item.textContent)).toEqual([i18n.t("lang.en")]);
  });

  it.each(SUPPORTED_LOCALES)("writes its own words in %s", async (locale) => {
    await i18n.changeLanguage(locale);
    stubBranding(["sk", "en", "de", "cs"]);
    const { container } = show();

    expect(trigger()).toHaveAccessibleName(i18n.t("lang.label"));
    expect(trigger()).toHaveTextContent(i18n.t(`lang.${locale}`));
    expectNoRawKeys(container);
  });
});
