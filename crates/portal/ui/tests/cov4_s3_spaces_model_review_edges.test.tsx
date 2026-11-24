// T-7706: ModelReview edges: an attribute with no column in the file says no sample, and a model without the type shows nothing.
import { render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { ModelReview } from "../src/pages/spaces/ModelReview";

const SOURCE = `id: https://hel.fi/models/stations
name: stations
default_prefix: hel
prefixes:
  hel: https://hel.fi/models/
classes:
  Station:
    slots: [name, capacity]
slots:
  name:
    range: string
  capacity:
    range: integer
`;

function review(source: string | undefined, type = "Station") {
  vi.stubGlobal("fetch", vi.fn(async () => new Response(JSON.stringify({}), { status: 200, headers: { "Content-Type": "application/json" } })));
  return render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>
        <ModelReview project="helsinki" space="stations" type={type} source={source} table={{ columns: ["name"], rows: [{ name: "Kamppi" }] }} onCorrect={() => undefined} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

describe("the model read before it is proposed", () => {
  it("shows a value from the file where a column matches and says there is none where it does not", () => {
    review(SOURCE);
    expect(screen.getByText(en.spaces.complete.review.sample.replace("{value}", "Kamppi"))).toBeInTheDocument();
    expect(screen.getAllByText(en.spaces.complete.review.noSample)).toHaveLength(1);
  });

  it("shows nothing for a type the model does not have", () => {
    const { container } = review(SOURCE, "Nowhere");
    expect(container).toBeEmptyDOMElement();
  });
});
