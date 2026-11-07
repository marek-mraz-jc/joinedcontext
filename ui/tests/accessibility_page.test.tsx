// covers (T-2137, the module gate in gate_modules.test.ts): src/pages/accessibility/AccessibilityPage.tsx,
// the accessibility statement a Slovak public body publishes (T-5108, Decree 78/2020, WCAG 2.1 AA).
import { render, screen } from "@testing-library/react";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n, { SUPPORTED_LOCALES } from "../src/i18n";
import * as branding from "../src/branding";
import en from "../src/locales/en.json";
import { AccessibilityPage } from "../src/pages/accessibility/AccessibilityPage";
import { expectNoRawKeys, expectNoViolations } from "./checks";

const show = (overrides: Partial<branding.Branding> = {}) => {
  vi.spyOn(branding, "useBranding").mockReturnValue({ ...branding.NEUTRAL_BRANDING, ...overrides });
  return render(
    <I18nextProvider i18n={i18n}>
      <AccessibilityPage />
    </I18nextProvider>,
  );
};

describe("the accessibility statement (T-5108)", () => {
  beforeEach(async () => {
    vi.restoreAllMocks();
    await i18n.changeLanguage("en");
  });

  it("names the status, every gap, the method, the dates, the contact and the enforcement body", async () => {
    const { container } = show({ organisation: "Mesto Banská Bystrica", contactEmail: "pristupnost@example.sk" });
    expect(screen.getByRole("heading", { level: 1, name: en.accessibility.title })).toBeInTheDocument();
    for (const key of ["law", "status", "gaps", "method", "dates", "contact", "enforcement"] as const) {
      expect(screen.getByRole("heading", { level: 2, name: en.accessibility[key].heading })).toBeInTheDocument();
    }
    expect(screen.getByText(/Mesto Banská Bystrica wants everyone/)).toBeInTheDocument();
    expect(screen.getByText(en.accessibility.status.body)).toBeInTheDocument();
    expect(screen.getAllByRole("listitem")).toHaveLength(4);
    expect(screen.getByText(/prepared on October 21, 2026/)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "pristupnost@example.sk" })).toHaveAttribute("href", "mailto:pristupnost@example.sk");
    await expectNoViolations(container);
  });

  it("without a contact address names the operator instead of a dead link", () => {
    show({ organisation: "", instanceName: "Open Data BB", contactEmail: "" });
    expect(screen.queryByRole("link")).toBeNull();
    expect(screen.getByText(/ask its operator, Open Data BB/)).toBeInTheDocument();
  });

  it("refuses an address that would not be a mail link", () => {
    show({ contactEmail: "javascript:alert(1)//@x" });
    expect(screen.queryByRole("link")).toBeNull();
  });

  it.each(SUPPORTED_LOCALES)("says everything in %s with no raw key", async (locale) => {
    await i18n.changeLanguage(locale);
    const { container } = show({ contactEmail: "a@b.sk" });
    expectNoRawKeys(container);
  });
});
