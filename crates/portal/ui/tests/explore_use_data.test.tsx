// "Use this data" in the explorer (T-3254): the dialog shows the view as curl, Python and
// JavaScript for the endpoint the person reads through, says a public endpoint needs no token and
// otherwise how to get one, and a copy carries the snippet and never a secret.
import type { ReactNode } from "react";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { UseThisData } from "../src/pages/explore/UseThisData";
import { InRouter } from "./pageHarness";

const U = en.explore.useData;
const QUERY = { type: "BikeStation", q: "bikes<5", attrs: ["bikes"] };

function show(open: boolean, wrap: (node: ReactNode) => ReactNode = (node) => node) {
  return render(
    <I18nextProvider i18n={i18n}>
      <InRouter>{wrap(<UseThisData project="helsinki" slug="bikes-ops" query={QUERY} open={open} />)}</InRouter>
    </I18nextProvider>,
  );
}

describe("use this data", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("shows the view as three snippets for the person's endpoint, with its filter", async () => {
    show(true);
    await userEvent.click(await screen.findByRole("button", { name: U.open }));
    const dialog = await screen.findByRole("dialog", { name: U.title });
    expect(within(dialog).getByText(U.public)).toBeInTheDocument();
    for (const language of ["curl", "python", "javascript"] as const) {
      const code = dialog.querySelector(`[data-snippet="${language}"]`)?.textContent ?? "";
      expect(code).toContain("/api/endpoint/bikes-ops/ngsi-ld/v1/entities?type=BikeStation&q=bikes%3C5&attrs=bikes");
      expect(code).not.toContain("JC_TOKEN");
    }
  });

  it("says how a program gets its token for a closed endpoint, and links to the service accounts", async () => {
    show(false);
    await userEvent.click(await screen.findByRole("button", { name: U.open }));
    const dialog = await screen.findByRole("dialog", { name: U.title });
    expect(within(dialog).getByRole("link", { name: U.tokenLink })).toHaveAttribute("href", "/projects/helsinki/settings/service-accounts");
    expect(dialog.querySelector('[data-snippet="curl"]')?.textContent).toContain('"Authorization: Bearer $JC_TOKEN"');
  });

  it("copies the snippet itself", async () => {
    const copied: string[] = [];
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText: async (text: string) => void copied.push(text) },
    });
    show(true);
    await userEvent.click(await screen.findByRole("button", { name: U.open }));
    await userEvent.click(await screen.findByRole("button", { name: "Copy the Python 3 (no packages needed) snippet" }));
    expect(copied[0]).toContain("import json, os, urllib.request");
  });
});
