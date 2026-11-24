// T-7589: coverage for GrafanaPage error states, empty endpoints, kind selection, pending dashboards
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import i18n from "../src/i18n";
import { VisualizationForm, VisualizationPreview } from "../src/pages/grafana/GrafanaPage";

function renderWithClient(node: ReactNode) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>{node}</I18nextProvider>
    </QueryClientProvider>,
  );
}

describe("GrafanaPage additional coverage", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("handles non-JSON error response in VisualizationPreview to cover line 164", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () =>
        new Response("Internal Server Error (plain text)", {
          status: 500,
          headers: { "Content-Type": "text/plain" },
        }),
      ),
    );

    renderWithClient(<VisualizationPreview slug="error-slug" type="AirQuality" attribute="pm25" />);
    expect(await screen.findByText(/HTTP 500|Internal Server Error/)).toBeInTheDocument();
  });

  it("triggers onChange for kind select in VisualizationForm to cover line 336", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL) => {
        const url = new URL(typeof input === "string" ? input : (input as Request).url);
        if (url.pathname.endsWith("/grafana/options")) {
          return new Response(
            JSON.stringify({
              endpoints: [{ name: "air", title: "Air quality", slug: "airslug", public: true, usable: true }],
              types: ["AirQualityObserved"],
              attributes: [{ name: "pm25", kinds: ["timeseries", "stat", "table"] }],
            }),
            { headers: { "Content-Type": "application/json" } },
          );
        }
        if (url.pathname.startsWith("/api/endpoint/")) {
          return new Response(JSON.stringify([{ id: "urn:ngsi-ld:1", pm25: 12 }]), {
            headers: { "Content-Type": "application/json" },
          });
        }
        return new Response("{}", { status: 404 });
      }),
    );

    const user = userEvent.setup();
    renderWithClient(<VisualizationForm project="helsinki" />);

    await screen.findByRole("option", { name: "Air quality" });
    await user.selectOptions(screen.getByLabelText("Endpoint"), "air");
    await waitFor(() => expect(screen.getByLabelText("Type")).toBeEnabled());
    await user.selectOptions(screen.getByLabelText("Type"), "AirQualityObserved");
    await waitFor(() => expect(screen.getByLabelText("Attribute")).toBeEnabled());
    await user.selectOptions(screen.getByLabelText("Attribute"), "pm25");

    const kindSelect = screen.getByLabelText("Visualization");
    expect(kindSelect).toHaveValue("timeseries");

    fireEvent.change(kindSelect, { target: { value: "stat" } });
    expect(kindSelect).toHaveValue("stat");
  });
});
