// T-7706: ModelsPage: a source write that fails with something that is not an API refusal still says what it met, on Check and on Save.
import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { RouterProvider, createRootRoute, createRoute, createRouter } from "@tanstack/react-router";
import i18n from "../src/i18n";
import { rememberPrefill } from "../src/assistant/state";
import en from "../src/locales/en.json";
import { ModelsPage } from "../src/pages/models/ModelsPage";

vi.mock("../src/api/datamodelSource", async (importOriginal) => {
  const original = await importOriginal<typeof import("../src/api/datamodelSource")>();
  return {
    ...original,
    writeModelSource: vi.fn(async (options: { dryRun: boolean }) => {
      if (options.dryRun) throw new Error("the check crashed");
      throw "the save crashed";
    }),
  };
});

const PUBLISHED = `id: https://example.org/models/air-quality
name: air-quality
prefixes:
  aq: https://example.org/aq/
default_prefix: aq
imports:
  - linkml:types
classes:
  AirQualityObserved:
    class_uri: https://example.org/aq/AirQualityObserved
    slots:
      - dateObserved
slots:
  dateObserved:
    range: string
    required: true
`;
const EDITED = PUBLISHED.replace("      - dateObserved\n", "      - dateObserved\n      - pm10\n").concat("  pm10:\n    range: float\n");
const baseline = { name: "air-quality", version: "1.0.0", lifecycle: "published" } as const;

function show() {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const rootRoute = createRootRoute();
  const home = createRoute({
    getParentRoute: () => rootRoute,
    path: "/",
    component: () => (
      <QueryClientProvider client={queryClient}>
        <ModelsPage project="ovzdusie" baseline={baseline} />
      </QueryClientProvider>
    ),
  });
  render(
    <I18nextProvider i18n={i18n}>
      <RouterProvider router={createRouter({ routeTree: rootRoute.addChildren([home]) })} />
    </I18nextProvider>,
  );
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const url = new URL(input instanceof Request ? input.url : String(input), window.location.origin);
      if (url.pathname.includes("/datamodels/air-quality/source")) {
        return new Response(PUBLISHED, { status: 200, headers: { "Content-Type": "text/yaml" } });
      }
      return new Response(JSON.stringify({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] }), { status: 200, headers: { "Content-Type": "application/json" } });
    }),
  );
});
afterEach(() => {
  vi.unstubAllGlobals();
});

describe("a write that throws", () => {
  it("says the error's own words on Check and the text of a bare rejection on Save", async () => {
    const user = userEvent.setup();
    rememberPrefill("/", { source: EDITED });
    show();
    await user.click(await screen.findByRole("button", { name: en.models.source.saveCheck }));
    await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent("the check crashed"));
    await user.click(screen.getByRole("button", { name: /^Propose v\d+\.\d+\.\d+$/ }));
    await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent("the save crashed"));
  });
});
