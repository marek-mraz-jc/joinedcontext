import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { ExploreRowActions } from "../src/pages/explore/ExploreRowActions";
import { expectDenied } from "./checks";
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

// T-5619: the explorer's own part of a row's detail, the removal and the raw entity.
describe("the explorer's row actions (T-5619)", () => {
  it("opens the reason in place and gives the focus back to Delete on Cancel", async () => {
    await i18n.changeLanguage("en");
    show();
    const remove = screen.getByTestId("explore-delete");
    expect(remove).toHaveAttribute("aria-expanded", "false");
    await userEvent.click(remove);
    expect(remove).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByRole("region", { name: en.explore.removeTitle })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: en.form.cancel }));
    expect(screen.queryByRole("region", { name: en.explore.removeTitle })).toBeNull();
    expect(remove).toHaveFocus();
  });

  it("keeps Delete in sight and says why when the grant refuses it", async () => {
    await i18n.changeLanguage("en");
    show("This endpoint's grant does not let you delete a BikeHireDockingStation");
    expectDenied(screen.getByTestId("explore-delete"), /delete/);
  });

  it("reads the entity only when the technical details are opened", async () => {
    await i18n.changeLanguage("en");
    const fetch = show();
    expect(fetch).not.toHaveBeenCalled();
    await userEvent.click(screen.getByText(en.explore.technical));
    expect(await screen.findByRole("group", { name: en.codeBlock.entity })).toHaveTextContent(ID);
    expect(fetch).toHaveBeenCalledTimes(1);
  });
});
