/**
 * T-4760: an Endpoint's page shows its NGSI-LD query card as the gateway serves it to this person
 * (`card://{slug}` on the Endpoint's MCP surface, read with the person's own session), with its MCP
 * address, a Copy button for each, and "Ask the assistant" scoped to the Endpoint. A person whose
 * access does not reach the Endpoint sees no card.
 */
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { QueryCard } from "../src/components/endpoints/QueryCard";
import { ASSISTANT_EVENT } from "../src/navigation/shortcuts";

const SLUG = "3ecozggnnhjlp5miouhia53mr2";
/** A card as the gateway answers it (fixture of `jc_core::query_card::served`). */
const CARD =
  "NGSI-LD Endpoint helsinki/helsinki-air (Air in Helsinki), ETSI GS CIM 009 / TS 104 175. Read it with query_entities (type, q, georel, pick).\n" +
  "AirQualityObserved: pm10:float[ug/m3], temperature:float[Cel]\n  e.g. q=pm10>0";

function stubGateway(answer: () => Response) {
  const asked: {
    url: string;
    body: unknown;
    credentials: RequestCredentials;
  }[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const request = input as Request;
      asked.push({
        url: request.url,
        body: JSON.parse(await request.text()),
        credentials: request.credentials,
      });
      return answer();
    }),
  );
  return asked;
}

const rpc = (body: unknown, type = "application/json") =>
  new Response(typeof body === "string" ? body : JSON.stringify(body), {
    headers: { "Content-Type": type },
  });

function renderCard() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <QueryCard slug={SLUG} name="helsinki-air" domain="dev.city.example" />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

describe("QueryCard", () => {
  const writeText = vi.fn(() => Promise.resolve());
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText },
      configurable: true,
    });
  });
  afterEach(() => {
    writeText.mockClear();
    vi.unstubAllGlobals();
  });

  it("renders the gateway's card and copies the exact text and the MCP address", async () => {
    const asked = stubGateway(() =>
      rpc(
        `data: ${JSON.stringify({ jsonrpc: "2.0", id: 1, result: { contents: [{ uri: `card://${SLUG}`, mimeType: "text/plain", text: CARD }] } })}\n\n`,
        "text/event-stream",
      ),
    );
    renderCard();
    const card = await screen.findByLabelText(en.endpoints.card.title, {
      selector: "pre",
    });
    expect(card.textContent).toBe(CARD);
    expect(asked).toEqual([
      {
        url: `${window.location.origin}/api/endpoint/${SLUG}/mcp`,
        body: {
          jsonrpc: "2.0",
          id: 1,
          method: "resources/read",
          params: { uri: `card://${SLUG}` },
        },
        credentials: "same-origin",
      },
    ]);
    // userEvent.click, not a setup(): setup() puts its own clipboard in place of the stub.
    await userEvent.click(
      screen.getByRole("button", { name: en.endpoints.card.copy }),
    );
    expect(writeText).toHaveBeenLastCalledWith(CARD);
    const mcp = `https://api.dev.city.example/api/endpoint/${SLUG}/mcp`;
    expect(screen.getByText(mcp)).toBeInTheDocument();
    await userEvent.click(
      screen.getByRole("button", { name: en.endpoints.copyUrl }),
    );
    expect(writeText).toHaveBeenLastCalledWith(mcp);
  });

  it("opens the assistant scoped to this Endpoint", async () => {
    stubGateway(() =>
      rpc({ jsonrpc: "2.0", id: 1, result: { contents: [{ text: CARD }] } }),
    );
    renderCard();
    const seen: unknown[] = [];
    const listen = (event: Event) => seen.push((event as CustomEvent).detail);
    window.addEventListener(ASSISTANT_EVENT, listen);
    await userEvent.click(
      screen.getByRole("button", { name: en.endpoints.card.ask }),
    );
    window.removeEventListener(ASSISTANT_EVENT, listen);
    expect(seen).toEqual([{ endpoints: ["helsinki-air"] }]);
  });

  it.each([401, 403])(
    "shows no card to a person the gateway refuses with %i",
    async (status) => {
      stubGateway(() => new Response("", { status }));
      renderCard();
      expect(
        await screen.findByText(en.endpoints.card.refused),
      ).toBeInTheDocument();
      expect(screen.queryByText(/AirQualityObserved/)).toBeNull();
      expect(
        screen.queryByRole("button", { name: en.endpoints.card.copy }),
      ).toBeNull();
    },
  );

  it("shows no card when the gateway does not serve it to this person", async () => {
    stubGateway(() =>
      rpc({
        jsonrpc: "2.0",
        id: 1,
        error: { code: -32602, message: "unknown resource" },
      }),
    );
    renderCard();
    expect(
      await screen.findByText(en.endpoints.card.refused),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: en.endpoints.card.copy }),
    ).toBeNull();
  });

  it("says the card could not be read when the gateway fails", async () => {
    stubGateway(() => new Response("", { status: 502 }));
    renderCard();
    expect(await screen.findByText(en.endpoints.card.failed)).toHaveAttribute(
      "role",
      "status",
    );
    expect(screen.queryByRole("alert")).toBeNull();
  });
});
