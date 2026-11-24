// T-7706: publicRouter: a visitor's catalogue search moves the address, and a page that throws shows the
// error page (or the chunk-failed notice for a failed module load) instead of a blank screen.
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, describe, expect, it, vi } from "vitest";
import { PublicApp } from "../src/PublicApp";
import en from "../src/locales/en.json";

const failure = vi.hoisted(() => ({ error: new Error("boom") }));
vi.mock("../src/pages/glossary/GlossaryPage", () => ({
  GlossaryPage: (): never => {
    throw failure.error;
  },
}));

const PAGE = {
  total: 1, page: 1, pageSize: 20,
  datasets: [{ name: "bikes", title: "City bikes", publisher: { name: "helsinki", title: "Helsinki" }, formats: ["CSV"], themes: [] }],
  facets: { publisher: [], theme: [], format: [], licence: [], spatial: [], year: [] },
  unavailable: [],
};

function visit(path: string) {
  window.history.pushState({}, "", path);
  const seen: URL[] = [];
  vi.stubGlobal("fetch", (input: RequestInfo | URL) => {
    const url = new URL(typeof input === "string" ? input : input instanceof URL ? input.href : input.url, "http://localhost");
    seen.push(url);
    const ok = url.pathname === "/api/v1/catalogue";
    return Promise.resolve(new Response(JSON.stringify(ok ? PAGE : { title: "Unauthorized", status: 401 }), {
      status: ok ? 200 : 401,
      headers: { "Content-Type": ok ? "application/json" : "application/problem+json" },
    }));
  });
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <PublicApp />
    </QueryClientProvider>,
  );
  return seen;
}

afterEach(() => {
  failure.error = new Error("boom");
  vi.unstubAllGlobals();
  window.history.pushState({}, "", "/");
});

describe("publicRouter (T-7706)", () => {
  it("carries a typed search into the address and asks the catalogue for it", async () => {
    const seen = visit("/catalogue");
    const box = await screen.findByRole("searchbox", { name: en.catalogue.search.label });
    await userEvent.type(box, "bikes{Enter}");
    await waitFor(() => expect(window.location.search).toContain("q=bikes"));
    await waitFor(() => expect(seen.some((url) => url.searchParams.get("q") === "bikes")).toBe(true));
  });

  it("shows the error page with a reference when a public page throws", async () => {
    visit("/glossary");
    expect(await screen.findByRole("alert")).toHaveTextContent(/This page stopped/);
    expect(screen.getByTestId("error-reference").textContent).not.toBe("");
  });

  it("shows the chunk-failed notice, not the error page, when the page's module fails to load", async () => {
    failure.error = new Error("Failed to fetch dynamically imported module: /assets/Glossary.js");
    visit("/glossary");
    await waitFor(() => expect(document.querySelector("main")).not.toBeNull());
    await waitFor(() => expect(screen.queryByText(/This page stopped/)).toBeNull());
    expect(screen.queryByTestId("error-reference")).toBeNull();
    expect(screen.getByRole("alert")).toBeInTheDocument();
  });
});
