// covers (T-2137, the module gate in gate_modules.test.ts): src/pages/settings/SettingsPage.tsx,
// src/components/layout/ShellFooter.tsx. Settings (T-6344): a person's own, the project's and, for
// a platform administrator alone, the platform's settings; the signed-in footer keeps the
// accessibility statement that left the account menu.
import type { ReactNode } from "react";
import { render, screen } from "@testing-library/react";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n, { SUPPORTED_LOCALES } from "../src/i18n";
import { expectNoRawKeys, expectNoViolations } from "./checks";

let opensAdministration = false;
vi.mock("../src/api/permissions", () => ({
  useAdministers: () => ({ known: true, administers: opensAdministration, opensAdministration }),
}));
vi.mock("@tanstack/react-router", () => ({
  Link: ({ children, to, params }: { children: ReactNode; to: string; params?: Record<string, string> }) => (
    <a href={Object.entries(params ?? {}).reduce((path, [k, v]) => path.replace(`$${k}`, v), to)}>{children}</a>
  ),
}));

const { SettingsPage } = await import("../src/pages/settings/SettingsPage");
const { ShellFooter } = await import("../src/components/layout/ShellFooter");

const show = (node: ReactNode) => render(<I18nextProvider i18n={i18n}>{node}</I18nextProvider>);

describe("the settings page (T-6344)", () => {
  beforeEach(async () => {
    opensAdministration = false;
    await i18n.changeLanguage("en");
  });

  it("lists the person's own and the project's settings, and no platform group for a non-administrator", async () => {
    const { container } = show(<SettingsPage project="doprava" />);
    expect(screen.getByRole("heading", { level: 1, name: "Settings" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "My AI connectors" })).toHaveAttribute("href", "/ai-connectors");
    expect(screen.getByRole("link", { name: "Settings of doprava" })).toHaveAttribute("href", "/projects/doprava/settings");
    expect(screen.queryByText("Platform")).toBeNull();
    // The line under the title names only what this person sees.
    expect(screen.getByText("Your own settings and this project's.")).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Administration" })).toBeNull();
    await expectNoViolations(container);
  });

  it("gives a member with no project their own settings alone (T-6411)", () => {
    show(<SettingsPage />);
    expect(screen.getByRole("link", { name: "My AI connectors" })).toHaveAttribute("href", "/ai-connectors");
    expect(screen.queryByRole("link", { name: /Settings of/ })).toBeNull();
    expect(screen.getByText("Your own settings.")).toBeInTheDocument();
  });

  it("adds Administration for a platform administrator", () => {
    opensAdministration = true;
    show(<SettingsPage project="doprava" />);
    expect(screen.getByText("Platform")).toBeInTheDocument();
    expect(screen.getByText("Your own settings, this project's and, for an administrator, the platform's.")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Administration" })).toHaveAttribute("href", "/organization/settings");
  });

  it.each(SUPPORTED_LOCALES)("says everything in %s with no raw key", async (locale) => {
    await i18n.changeLanguage(locale);
    opensAdministration = true;
    const { container } = show(<SettingsPage project="doprava" />);
    expectNoRawKeys(container);
  });
});

describe("the signed-in footer (T-6344)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("links the accessibility statement and the glossary with no branding set", async () => {
    const { container } = show(<ShellFooter />);
    expect(screen.getByRole("link", { name: "Accessibility" })).toHaveAttribute("href", "/accessibility");
    expect(screen.getByRole("link", { name: "Glossary" })).toHaveAttribute("href", "/glossary");
    expect(screen.queryByRole("link", { name: /@/ })).toBeNull();
    await expectNoViolations(container);
  });

  it("adds the organisation and its contact address when the branding sets them", () => {
    show(<ShellFooter organisation="City of Helsinki" contactEmail="data@hel.fi" />);
    expect(screen.getByText("City of Helsinki")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "data@hel.fi" })).toHaveAttribute("href", "mailto:data@hel.fi");
  });
});
