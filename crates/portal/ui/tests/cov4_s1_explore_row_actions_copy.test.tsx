// T-7706: ExploreRowActions: the copy of the entity's JSON when the clipboard takes it and when it refuses.
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { ExploreRowActions } from "../src/pages/explore/ExploreRowActions";
import en from "../src/locales/en.json";

const ID = "urn:ngsi-ld:BikeHireDockingStation:hel.fi:helsinki:001";

function show(deniedReason?: string) {
  const fetch = vi.fn(() => Promise.resolve(new Response(JSON.stringify({ id: ID, type: "BikeHireDockingStation" }), { status: 200, headers: { "Content-Type": "application/json" } })));
  vi.stubGlobal("fetch", fetch);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <ExploreRowActions slug="scsd2eehkx42n53z2zyd6vshfh7s7irf" id={ID} deniedReason={deniedReason} onDeleted={() => undefined} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return fetch;
}

afterEach(() => {
  vi.unstubAllGlobals();
});

function stubClipboard(writeText: (text: string) => Promise<void>) {
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
}

describe("copying the entity's JSON", () => {
  it("says it copied once the clipboard took the text", async () => {
    await i18n.changeLanguage("en");
    const writeText = vi.fn(async () => undefined);
    stubClipboard(writeText);
    show();
    await userEvent.click(screen.getByText(en.explore.technical));
    await userEvent.click(await screen.findByRole("button", { name: en.explore.copyJson }));
    expect(writeText).toHaveBeenCalledWith(expect.stringContaining(ID));
    expect(await screen.findByRole("button", { name: en.endpoints.copied })).toBeInTheDocument();
  });

  it("keeps saying Copy when the clipboard refuses", async () => {
    await i18n.changeLanguage("en");
    const writeText = vi.fn(async () => {
      throw new Error("denied");
    });
    stubClipboard(writeText);
    show();
    await userEvent.click(screen.getByText(en.explore.technical));
    await userEvent.click(await screen.findByRole("button", { name: en.explore.copyJson }));
    await waitFor(() => expect(writeText).toHaveBeenCalled());
    expect(screen.getByRole("button", { name: en.explore.copyJson })).toBeInTheDocument();
  });
});
