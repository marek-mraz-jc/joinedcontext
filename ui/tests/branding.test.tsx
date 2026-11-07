import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { App } from "../src/App";
import { applyBranding, NEUTRAL_BRANDING, offeredLocales, withBundledFont } from "../src/branding";

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  roles: [],
};

const HELSINKI = {
  instanceName: "Helsinki Region Context",
  shortName: "Helsinki Context",
  organisation: "City of Helsinki",
  contactEmail: "opendata@hel.fi",
  logo: "logo.svg",
  favicon: "",
  colours: {
    primary: "#0000bf",
    secondary: "#0072c6",
    accent: "#ffe977",
    background: "#ffffff",
    text: "#1a1a1a",
  },
  fonts: { heading: "HelsinkiGrotesk, system-ui, sans-serif", body: "system-ui, sans-serif" },
  languages: { default: "en", offered: ["en", "sk"] },
  primaryForeground: "#ffffff",
};

function renderApp(branding: unknown | null) {
  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    const request = input as Request;
    const path = new URL(request.url).pathname;
    const json = (body: unknown, code = 200) =>
      Promise.resolve(
        new Response(JSON.stringify(body), {
          status: code,
          headers: {
            "Content-Type": code >= 400 ? "application/problem+json" : "application/json",
          },
        }),
      );
    if (path.endsWith("/branding")) {
      return branding === null
        ? json({ title: "Internal Server Error", status: 500 }, 500)
        : json(branding);
    }
    if (path.endsWith("/auth/me")) {
      return json(IDENTITY);
    }
    return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
  });
  vi.stubGlobal("fetch", fetchMock);

  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <App />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return fetchMock;
}

describe("runtime branding", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.localStorage.clear();
    document.documentElement.removeAttribute("style");
    window.history.pushState({}, "", "/projects/banskabystrica/spaces");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  /** UI-30: one image, and the tokens every component already reads. */
  it("writes the colours and fonts as custom properties on the root element", () => {
    applyBranding(HELSINKI as never);
    const style = document.documentElement.style;

    expect(document.title).toBe("Helsinki Region Context");
    expect(style.getPropertyValue("--portal-color-primary")).toBe("#0000bf");
    expect(style.getPropertyValue("--portal-color-primary-fg")).toBe("#ffffff");
    expect(style.getPropertyValue("--portal-color-secondary")).toBe("#0072c6");
    expect(style.getPropertyValue("--portal-color-accent")).toBe("#ffe977");
    expect(style.getPropertyValue("--portal-color-surface")).toBe("#ffffff");
    expect(style.getPropertyValue("--portal-color-surface-fg")).toBe("#1a1a1a");
    expect(style.getPropertyValue("--portal-font-heading")).toBe(
      'HelsinkiGrotesk, "Inter", system-ui, sans-serif',
    );
  });

  it("falls back to the bundled Inter before any system family, and names it once (T-0756)", () => {
    expect(withBundledFont("HelsinkiGrotesk, system-ui, sans-serif")).toBe('HelsinkiGrotesk, "Inter", system-ui, sans-serif');
    expect(withBundledFont("system-ui, sans-serif")).toBe('"Inter", system-ui, sans-serif');
    expect(withBundledFont("Georgia")).toBe('Georgia, "Inter"');
    expect(withBundledFont("'Inter', sans-serif")).toBe("'Inter', sans-serif");
    expect(withBundledFont("")).toBe('"Inter"');
  });

  it("shows the instance name and its logo in the shell", async () => {
    renderApp(HELSINKI);

    const header = await screen.findByRole("banner");
    await waitFor(() => {
      expect(header).toHaveTextContent("Helsinki Context");
    });
    const logo = header.querySelector("img");
    expect(logo).toHaveAttribute("src", "/api/v1/branding/logo");
    // Below md the logo stands alone; the name, cut to "Hel…" there, stays for a screen reader (T-4165).
    expect(within(header).getByText("Helsinki Context")).toHaveClass("max-md:sr-only");
    const footer = screen.getByRole("contentinfo");
    expect(footer).toHaveTextContent("City of Helsinki");
    expect(footer.querySelector("a")).toHaveAttribute("href", "mailto:opendata@hel.fi");
  });

  it("offers only the languages the installation names", async () => {
    renderApp(HELSINKI);
    const user = userEvent.setup();

    await user.click(await screen.findByRole("button", { name: /language/i }));

    const items = await screen.findAllByRole("menuitem");
    expect(items.map((item) => item.textContent)).toEqual(["English", "Slovenčina"]);
  });

  it("falls back to the neutral look when the endpoint answers an error", async () => {
    renderApp(null);

    const header = await screen.findByRole("banner");
    await waitFor(() => {
      expect(header).toHaveTextContent(NEUTRAL_BRANDING.instanceName as string);
    });
    // Nothing is branded, so nothing is claimed: no logo, and the footer holds only the
    // accessibility statement and the glossary every page links (T-6344).
    expect(header.querySelector("img")).toBeNull();
    expect(within(screen.getByRole("contentinfo")).getAllByRole("link").map((link) => link.getAttribute("href"))).toEqual(["/accessibility", "/glossary"]);
  });

  it("keeps every bundled language when the installation names none", () => {
    expect(offeredLocales({ ...NEUTRAL_BRANDING, languages: { default: "en", offered: [] } })).toEqual(
      ["en"],
    );
    expect(offeredLocales(HELSINKI as never)).toEqual(["en", "sk"]);
  });
});
