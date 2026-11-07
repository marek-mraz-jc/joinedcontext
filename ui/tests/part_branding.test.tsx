/**
 * T-1801: the installation's own look, against the UI contract (UI-16, UI-30, PF-50).
 *
 * Branding is the one place where values from outside become CSS a browser evaluates, so this
 * holds the boundary: only a hex colour and a plain font stack are written, a value that is not
 * one keeps the default and is named in the console rather than shown, the neutral block is
 * merged so a field the API left out blanks nothing, and the language a visitor chose survives an
 * installation default.
 *
 * The six hex colours in `NEUTRAL_BRANDING` stay hex on purpose: they mirror what the API
 * answers for an installation with no branding ConfigMap, and they are written into the colour
 * tokens rather than used as a colour here — that is their line in the UI rules allow-list.
 */
import { render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n, { SUPPORTED_LOCALES } from "../src/i18n";
import {
  applyBranding,
  BrandingProvider,
  isFontStack,
  isHexColour,
  logoUrl,
  NEUTRAL_BRANDING,
  offeredLocales,
  useBranding,
  withBundledFont,
} from "../src/branding";
import type { Branding } from "../src/branding";
import { expectNoViolations } from "./checks";

const HELSINKI: Branding = {
  ...NEUTRAL_BRANDING,
  instanceName: "Helsinki Region Context",
  shortName: "Helsinki Context",
  logo: "logo.svg",
  colours: {
    primary: "#0000bf",
    secondary: "#0072c6",
    accent: "#ffe977",
    background: "#ffffff",
    text: "#1a1a1a",
  },
  fonts: { heading: "HelsinkiGrotesk, system-ui, sans-serif", body: "system-ui, sans-serif" },
  languages: { default: "en", offered: ["en", "sk"] },
};

/** What the tree is handed, as text a test can read. */
function Reader() {
  const branding = useBranding();
  return <output aria-label="branding">{`${branding.instanceName}|${branding.colours?.primary ?? ""}|${branding.licenseDefault ?? ""}`}</output>;
}

function show(answer: Partial<Branding> | number) {
  vi.stubGlobal(
    "fetch",
    vi.fn(() =>
      Promise.resolve(
        typeof answer === "number"
          ? new Response(JSON.stringify({ status: answer, title: "Server Error" }), {
              status: answer,
              headers: { "Content-Type": "application/problem+json" },
            })
          : new Response(JSON.stringify(answer), {
              status: 200,
              headers: { "Content-Type": "application/json" },
            }),
      ),
    ),
  );
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <BrandingProvider>
          <Reader />
        </BrandingProvider>
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

const read = () => screen.getByLabelText("branding").textContent ?? "";

describe("branding against the UI contract", () => {
  beforeEach(async () => {
    localStorage.clear();
    await i18n.changeLanguage("sk");
    document.documentElement.removeAttribute("style");
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  it("writes only what a token may take, and says in the console what it dropped", () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const doc = document.implementation.createHTMLDocument("t");

    applyBranding(
      {
        ...HELSINKI,
        colours: { ...HELSINKI.colours, primary: "red; background: url(https://elsewhere/x)" },
        fonts: { heading: "url(https://elsewhere/font.woff2)", body: "Inter, sans-serif" },
      },
      doc,
    );

    const style = doc.documentElement.style;
    expect(style.getPropertyValue("--portal-color-primary")).toBe("");
    expect(style.getPropertyValue("--portal-font-heading")).toBe("");
    expect(style.getPropertyValue("--portal-color-accent")).toBe("#ffe977");
    // Named, never shown: the value that was refused never reaches the page.
    expect(warn).toHaveBeenCalled();
    expect(doc.documentElement.outerHTML).not.toContain("elsewhere");
  });

  it("puts the bundled font before the first family the browser resolves on its own", () => {
    expect(withBundledFont("HelsinkiGrotesk, system-ui, sans-serif")).toBe(
      'HelsinkiGrotesk, "Inter", system-ui, sans-serif',
    );
    expect(withBundledFont("Inter, sans-serif")).toBe("Inter, sans-serif");
    expect(withBundledFont("Brand")).toBe('Brand, "Inter"');
  });

  it("knows a colour and a font stack from something that only looks like one", () => {
    expect(isHexColour("#abc")).toBe(true);
    expect(isHexColour("#0000bf")).toBe(true);
    expect(isHexColour("rgb(0,0,191)")).toBe(false);
    expect(isHexColour("#0000bf; background: red")).toBe(false);
    expect(isFontStack("Inter, system-ui, sans-serif")).toBe(true);
    expect(isFontStack("url(https://elsewhere/x)")).toBe(false);
    expect(isFontStack("Inter; background: red")).toBe(false);
    expect(isFontStack("A".repeat(201))).toBe(false);
  });

  it("merges the answer into the neutral block, so a missing field blanks nothing", async () => {
    show({ instanceName: "Zvolen Context" });

    await waitFor(() => expect(read()).toContain("Zvolen Context"));
    // The colour and the licence the API did not send are still the neutral ones.
    expect(read()).toContain(NEUTRAL_BRANDING.colours!.primary!);
    expect(read()).toContain("CC-BY-4.0");
  });

  it("is the neutral look when the branding cannot be read, not an error state", async () => {
    const { container } = show(500);

    await waitFor(() => expect(read()).toContain(NEUTRAL_BRANDING.instanceName!));
    expect(screen.queryByRole("alert")).toBeNull();
    expect(container.children).toHaveLength(1);
    await expectNoViolations(container);
  });

  it("offers the installation's logo only when it configured one", () => {
    expect(logoUrl(HELSINKI)).toBe("/api/v1/branding/logo");
    expect(logoUrl(NEUTRAL_BRANDING)).toBeUndefined();
  });

  it("names the locales the installation offers, in its own order", () => {
    expect(offeredLocales(HELSINKI)).toEqual(["en", "sk"]);
    expect(offeredLocales({ ...HELSINKI, languages: { default: "en", offered: [] } })).toEqual(
      NEUTRAL_BRANDING.languages!.offered,
    );
  });

  it("takes the installation's default language, and leaves a chosen one alone", async () => {
    // No choice yet: `changeLanguage` above writes the switcher's own key, so it goes first.
    localStorage.removeItem("jc-lang");
    show({ languages: { default: "de", offered: ["de", "sk"] } });

    // The default is the installation's own, taken from the answer. Applying the neutral block's
    // `en` first wrote `jc-lang=en` on the way through, and the real default then read that as a
    // visitor's choice and never applied at all (T-1801).
    await waitFor(() => expect(i18n.language).toBe("de"));
  });

  it("leaves the language a visitor chose alone", async () => {
    // The key is the switcher's own, which is what a choice looks like from here.
    localStorage.setItem("jc-lang", "sk");
    await i18n.changeLanguage("sk");

    show({ languages: { default: "de", offered: ["de", "sk"] } });

    await waitFor(() => expect(read()).toContain("CC-BY-4.0"));
    expect(i18n.language).toBe("sk");
  });

  it.each(SUPPORTED_LOCALES)("keeps the neutral defaults whatever the page's language is (%s)", async (locale) => {
    await i18n.changeLanguage(locale);
    const doc = document.implementation.createHTMLDocument("t");
    applyBranding(NEUTRAL_BRANDING, doc);

    // The six defaults are data the API answers, written into the tokens as they are.
    expect(doc.documentElement.style.getPropertyValue("--portal-color-primary")).toBe("#1d4ed8");
    expect(doc.title).toContain(NEUTRAL_BRANDING.instanceName!);
  });
});
