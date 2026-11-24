// T-7706: CatalogSection edges: a publisher with no English name or none at all, an address that is no link, a raw record that is not JSON.
import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { CatalogSection } from "../src/pages/endpoints/CatalogSection";
import type { CatalogManifest } from "../src/pages/endpoints/catalog";

function show(catalog: CatalogManifest) {
  render(
    <I18nextProvider i18n={i18n}>
      <CatalogSection slug="scsd2eehkx42n53z2zyd6vshfh7s7irf" catalog={catalog} audience="public" />
    </I18nextProvider>,
  );
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("the catalogue card, the edges", () => {
  it("names the publisher by the first text it has when English is missing, and links it only when the address is safe", () => {
    show({ publisher: { name: { sk: "Mesto", cs: "  " }, uri: "https://city.example.org/" }, license: "CC_BY_4_0" });
    expect(screen.getByRole("link", { name: "Mesto" })).toHaveAttribute("href", "https://city.example.org/");
    cleanup();
    show({ publisher: { name: { sk: "Mesto" }, uri: "javascript:alert(1)" } });
    expect(screen.getByText("Mesto")).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Mesto" })).not.toBeInTheDocument();
  });

  it("shows no publisher name when the record gives none", () => {
    show({ license: "CC_BY_4_0", frequency: "DAILY" });
    expect(screen.getByText("Every day")).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Mesto" })).not.toBeInTheDocument();
  });

  it("says the raw record could not be read when the answer is not JSON", async () => {
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(new Response("<html>not json</html>", { status: 200 })));
    const user = userEvent.setup();
    show({ license: "CC_BY_4_0" });
    await user.click(screen.getByRole("button", { name: "Your access as ODRL" }));
    const dialog = await screen.findByRole("dialog");
    await waitFor(() => expect(within(dialog).getByText(/did not answer/)).toBeInTheDocument());
  });
});
