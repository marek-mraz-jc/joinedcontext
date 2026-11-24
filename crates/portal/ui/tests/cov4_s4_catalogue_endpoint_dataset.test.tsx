// T-7706: a dataset page by its Endpoint: the view's choices written to the address (and a repeat written once), and the endpoint's own DCAT title read in any language.
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { App } from "../src/App";
import { dcatTitle } from "../src/pages/catalogue/EndpointDatasetPage";
import type { DataLink } from "../src/components/entities/EndpointDataView";

vi.mock("../src/components/entities/EndpointDataView", () => ({
  EndpointDataView: ({ link, onLink }: { link?: DataLink; onLink?: (next: DataLink) => void }) => (
    <div>
      <p data-testid="view-type">{link?.type ?? "no type"}</p>
      <button type="button" onClick={() => onLink?.({ ...link, type: "Station" })}>
        pick a type
      </button>
      <button type="button" onClick={() => onLink?.({ ...link })}>
        pick the same
      </button>
    </div>
  ),
}));

const SLUG = "k7m2qz4tv6xh3n5jb2ryd3wcfa";
const IDENTITY = { subject: "b7c1e0f4", username: "jana.kovacova", name: "Jana Kováčová", roles: ["portal-viewer"] };

beforeEach(async () => {
  await i18n.changeLanguage("en");
});
afterEach(() => {
  vi.unstubAllGlobals();
});

function open(path: string, dcat: unknown) {
  window.history.pushState({}, "", path);
  const asked: URL[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL) => {
      const url = new URL(typeof input === "string" ? input : input instanceof URL ? input.href : input.url);
      asked.push(url);
      const json = (body: unknown) => new Response(JSON.stringify(body), { headers: { "Content-Type": "application/json" } });
      if (url.pathname.includes("/auth/me")) return Promise.resolve(json(IDENTITY));
      if (url.pathname === `/api/endpoint/${SLUG}/`) return Promise.resolve(json(dcat));
      if (url.pathname === "/api/v1/catalogue/readable") return Promise.resolve(json({ datasets: [], unanswered: 0 }));
      return Promise.resolve(json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] }));
    }),
  );
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>
        <App />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return asked;
}

describe("the dataset page of an endpoint", () => {
  it("writes the view's type into the address, and writes nothing for a choice that changes nothing", async () => {
    open(`/catalogue/endpoint/${SLUG}`, { "dct:title": "Bike stations" });
    expect(await screen.findByRole("heading", { level: 1, name: "Bike stations" })).toBeInTheDocument();
    await userEvent.click(await screen.findByRole("button", { name: "pick the same" }));
    expect(window.location.search).toBe("");
    await userEvent.click(screen.getByRole("button", { name: "pick a type" }));
    await waitFor(() => expect(window.location.search).toContain("type=Station"));
    await waitFor(() => expect(screen.getByTestId("view-type")).toHaveTextContent("Station"));
    const before = window.location.href;
    await userEvent.click(screen.getByRole("button", { name: "pick a type" }));
    expect(window.location.href).toBe(before);
  });
});

describe("the title of an endpoint's DCAT entry", () => {
  it("reads a plain string, the reader's language, English, then the first title", () => {
    expect(dcatTitle({ "dct:title": "Plain title" }, "sk")).toBe("Plain title");
    const titles = [
      { "@value": "Deutsch", "@language": "de" },
      { "@value": "English", "@language": "en" },
      { "@value": "Slovensky", "@language": "sk" },
    ];
    expect(dcatTitle({ "dct:title": titles }, "sk-SK")).toBe("Slovensky");
    expect(dcatTitle({ "dct:title": titles }, "cs")).toBe("English");
    expect(dcatTitle({ "dct:title": [titles[0]] }, "cs")).toBe("Deutsch");
    expect(dcatTitle({ "dct:title": [{ "@value": "  " }, null] }, "en")).toBeUndefined();
    expect(dcatTitle(undefined, "en")).toBeUndefined();
  });
});
