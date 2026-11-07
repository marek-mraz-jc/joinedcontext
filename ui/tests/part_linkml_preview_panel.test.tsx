/**
 * T-1847: the LinkML preview panel against the UI contract (UI-01, UI-15, UI-16, UI-48).
 *
 * The panel drew three different situations the same way. An empty editor, a compilation on its
 * way, and a compilation that came back without that artifact all rendered
 * `JSON.stringify(undefined ?? null)` — the word `null` in a code box, which reads as "Model
 * Tools says your model compiles to nothing". And a failed compile said only that the service
 * did not answer: not what it said, and with nothing to press. Each state is its own now.
 */
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n, { SUPPORTED_LOCALES } from "../src/i18n";
import en from "../src/locales/en.json";
import { expectNoRawKeys, expectNoViolations } from "./checks";
import { LinkmlPreviewPanel } from "../src/pages/models/LinkmlPreviewPanel";

const SOURCE = `id: https://banskabystrica.sk/models/air
name: air
classes:
  AirQualityObserved:
    slots: [pm10]
slots:
  pm10:
    range: float
`;

const ARTIFACTS = {
  jsonSchema: { title: "AirQualityObserved", type: "object" },
  context: { "@context": { pm10: "https://banskabystrica.sk/terms/pm10" } },
  example: { id: "urn:ngsi-ld:AirQualityObserved:banskabystrica.sk:ovzdusie:st-1", pm10: 31.4 },
  generatorVersion: "linkml-1.8.0",
  errors: [],
};

/** A compilation that never answers, so the panel can be seen while it waits. */
function pending(): Promise<Response> {
  return new Promise<Response>(() => undefined);
}

function show(answer: () => Promise<Response>, source = SOURCE) {
  const fetchMock = vi.fn(answer);
  vi.stubGlobal("fetch", fetchMock);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const view = render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <LinkmlPreviewPanel source={source} debounceMs={0} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return { fetchMock, container: view.container, user: userEvent.setup(), unmount: view.unmount };
}

const ok = () =>
  Promise.resolve(
    new Response(JSON.stringify(ARTIFACTS), { status: 200, headers: { "Content-Type": "application/json" } }),
  );

const refused = () =>
  Promise.resolve(
    new Response(JSON.stringify({ title: "Unprocessable", detail: "the model has no classes" }), {
      status: 422,
      headers: { "Content-Type": "application/problem+json" },
    }),
  );

describe("the LinkML preview panel against the UI contract", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("says there is nothing to compile yet instead of showing the word null", () => {
    show(ok, "   ");
    expect(screen.getByText(en.models.previewEmpty)).toBeInTheDocument();
    expect(screen.queryByText("null")).toBeNull();
  });

  it("shows the artifact taking shape while Model Tools compiles, and says so", () => {
    show(pending);
    const status = screen.getByRole("status", { name: en.models.previewCompiling });
    expect(status).toBeInTheDocument();
    expect(screen.queryByText("null")).toBeNull();
  });

  it("says what the compile service said, and compiles again when asked", async () => {
    const { fetchMock, user, container } = show(refused);
    // The sentence and the API's own detail are one paragraph: the panel says what happened
    // and what the service said about it.
    const said = await within(container).findByText(/the model has no classes/);
    expect(said).toHaveTextContent(en.models.previewUnavailable);
    // And no code box holding the word `null` where an artifact would be.
    expect(within(container).getByText(en.models.noArtifact)).toBeInTheDocument();
    expect(within(container).queryByText("null")).toBeNull();

    const before = fetchMock.mock.calls.length;
    await user.click(screen.getByRole("button", { name: en.app.error.retry }));
    expect(fetchMock.mock.calls.length).toBeGreaterThan(before);
  });

  it("shows the artifacts once they arrive", async () => {
    show(ok);
    expect(await screen.findByText(/AirQualityObserved/)).toBeInTheDocument();
    expect(screen.queryByRole("status", { name: en.models.previewCompiling })).toBeNull();
  });

  it("has no axe violation while compiling, when it failed, and when it is done", async () => {
    const waiting = show(pending);
    await expectNoViolations(waiting.container);
    waiting.unmount();
    vi.unstubAllGlobals();

    const failed = show(refused);
    await within(failed.container).findByText(/the model has no classes/);
    await expectNoViolations(failed.container);
    failed.unmount();
    vi.unstubAllGlobals();

    const done = show(ok);
    await within(done.container).findByText(/AirQualityObserved/);
    await expectNoViolations(done.container);
  });

  it("shows no raw translation key in any locale the organisation offers", async () => {
    for (const locale of SUPPORTED_LOCALES) {
      await i18n.changeLanguage(locale);
      const { container } = show(pending);
      expectNoRawKeys(container);
      vi.unstubAllGlobals();
    }
    await i18n.changeLanguage("en");
  });
});
