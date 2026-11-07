import { render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";

import { describe, expect, it, vi, beforeEach } from "vitest";
import i18n from "../src/i18n";
import { ListFailed } from "../src/components/forms/widgets/ListFailed";
import { reasonOf } from "../src/api/client";
import { ExplorePage } from "../src/pages/explore/ExplorePage";

/**
 * UI-44, T-1502: a choice that is empty because nobody could ask says so, in the same words on
 * every page, with the API's own reason and a retry. An empty select and a failed list look
 * identical and mean opposite things.
 */
function wrap(node: React.JSX.Element): React.JSX.Element {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return (
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        {node}
      </I18nextProvider>
    </QueryClientProvider>
  );
}

describe("says why when the list cannot be loaded", () => {
  beforeEach(() => {
    vi.restoreAllMocks();
  });

  it("names the list, the reason and a way to try again", async () => {
    const retry = vi.fn();
    render(
      wrap(
        <ListFailed
          what={i18n.t("nav.endpoints")}
          reason="403 Forbidden"
          onRetry={retry}
        />,
      ),
    );

    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain(i18n.t("nav.endpoints"));
    expect(alert.textContent).toContain("403 Forbidden");
    screen.getByRole("button", { name: i18n.t("form.listRetry") }).click();
    await waitFor(() => expect(retry).toHaveBeenCalledTimes(1));
  });

  it("falls back to a sentence when the failure carries none", () => {
    expect(reasonOf(new Error(""), "could not be reached")).toBe("could not be reached");
    expect(reasonOf({ status: 500 }, "could not be reached")).toBe("could not be reached");
    expect(reasonOf(new Error("503 Service Unavailable"), "x")).toBe("503 Service Unavailable");
  });

  it("says why on the explorer when its endpoint list was refused", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn((input: unknown) => {
        const url = String(input instanceof Request ? input.url : input);
        if (url.includes("/endpoints")) {
          return Promise.resolve(
            new Response(JSON.stringify({ title: "Forbidden", detail: "not your project" }), {
              status: 403,
              headers: { "Content-Type": "application/problem+json" },
            }),
          );
        }
        return Promise.resolve(
          new Response(JSON.stringify({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] }), {
            status: 200,
            headers: { "Content-Type": "application/json" },
          }),
        );
      }),
    );
    render(wrap(<ExplorePage project="helsinki" initialSpace="helsinki" />));

    const alert = await screen.findByRole("alert", {}, { timeout: 5000 });
    expect(alert.textContent).toContain(i18n.t("nav.endpoints"));
    // The API's own words, and nothing a person cannot act on.
    expect(alert.textContent).toMatch(/Forbidden|not your project|403/);
    // Never the empty-list sentence, which would say the opposite of what happened.
    expect(screen.queryByText(i18n.t("explore.noEndpoint"))).toBeNull();
  });
});
