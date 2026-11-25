// T-7706: small component units: a column's grid attribute, a Vega-Lite title object, a sparkline with one point,
// an unwritten widget oneOf, a model drawing with no source, the data bar's audience words, the conversation list's
// Retry and a refused Disconnect of a paired tab.
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { attrOf } from "../src/components/panels/TablePanel";
import { Sparkline } from "../src/components/panels/ValuePanels";
import { specTitle } from "../src/components/dashboards/VegaLiteWidget";
import { fieldWidget } from "../src/components/grid/fieldWidget";
import { DraftedModel } from "../src/pages/spaces/DraftedModel";
import { DataBar } from "../src/assistant/EndpointPicker";
import { ConversationList } from "../src/assistant/ConversationList";
import { PairedTabBar } from "../src/connectors/PairedTabBar";
import { json, renderPart } from "./page_contract";

afterEach(() => {
  vi.unstubAllGlobals();
  sessionStorage.clear();
});

describe("pure helpers (T-7706)", () => {
  it("names a grid attribute from a column: digits get a prefix, nothing left gets its index, id/type and repeats get the index", () => {
    const taken = new Set<string>();
    expect(attrOf("2024 total", 0, taken)).toBe("c_2024_total");
    expect(attrOf("---", 1, taken)).toBe("c1");
    expect(attrOf("id", 2, taken)).toBe("id_2");
    expect(attrOf("type", 3, taken)).toBe("type_3");
    expect(attrOf("PM2.5", 4, taken)).toBe("PM2_5");
    expect(attrOf("PM2.5", 5, taken)).toBe("PM2_5_5");
  });

  it("reads a Vega-Lite title that is an object with text, and none for a title without one", () => {
    expect(specTitle({ title: "Plain" } as never)).toBe("Plain");
    expect(specTitle({ title: { text: "Object title" } } as never)).toBe("Object title");
    expect(specTitle({ title: { text: 5 } } as never)).toBeNull();
    expect(specTitle({} as never)).toBeNull();
  });

  it("offers no inline widget for a oneOf whose values are not all text", () => {
    const schema = {
      type: "object",
      properties: { spec: { type: "object", properties: { level: { oneOf: [{ const: 1 }, { const: 2 }] } } } },
    };
    expect(fieldWidget("Endpoint", schema, "level")).toBeNull();
  });
});

describe("Sparkline", () => {
  it("draws nothing for fewer than two measured points", () => {
    const { container } = render(<Sparkline points={[{ t: 1, v: 3 }, { t: 2, v: null }]} color="#000" label="trend" height={20} />);
    expect(container.querySelector("svg")).toBeNull();
  });
});

describe("DraftedModel", () => {
  it("draws nothing while there is no drafted source", () => {
    const { container } = renderPart(<DraftedModel source={undefined} type="Sensors" table={{ columns: [], rows: [] } as never} />);
    expect(container.querySelector("[data-testid=drafted-model]")).toBeNull();
  });
});

describe("DataBar audience words", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("badges a project endpoint, and leaves an endpoint without an audience unbadged", async () => {
    const endpoint = (name: string, audience?: string) => ({
      apiVersion: "joinedcontext.com/v1alpha1", kind: "Endpoint",
      metadata: { name, project: "helsinki", title: { en: name } },
      spec: audience ? { audience } : {},
    });
    vi.stubGlobal("fetch", () =>
      Promise.resolve(json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [endpoint("inside", "project"), endpoint("bare")] })));
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <I18nextProvider i18n={i18n}>
          <DataBar project="helsinki" selected={[]} onChange={() => undefined} />
        </I18nextProvider>
      </QueryClientProvider>,
    );
    await userEvent.setup().click(screen.getByRole("button", { name: en.assistant.data.add }));
    const list = await screen.findByRole("listbox", { name: en.assistant.data.label });
    const inside = within(list).getByRole("option", { name: /inside/ });
    expect(inside).toHaveTextContent(en.assistant.data.audience.project);
    expect(within(list).getByRole("option", { name: /bare/ })).not.toHaveTextContent(en.assistant.data.audience.project);
  });
});

describe("ConversationList", () => {
  it("shows the failed read with its reason and reads again on Retry", async () => {
    let reads = 0;
    renderPart(<ConversationList project="helsinki" current={null} onOpen={() => undefined} />, {
      answer: async (url) => {
        if (url.pathname.endsWith("/auth/me")) return json({ subject: "s", username: "jana.kovacova", roles: [] });
        if (url.pathname.endsWith("/agent-runs")) {
          reads += 1;
          return reads === 1 ? json({ title: "Unavailable", status: 503, detail: "the run store is away" }, 503) : json({ items: [{ id: "r1", prompt: "Show PM10", createdAt: "2026-10-18T10:00:00Z" }] });
        }
        return undefined;
      },
    });
    expect(await screen.findByText(/the run store is away/)).toBeInTheDocument();
    await userEvent.setup().click(screen.getByRole("button", { name: en.form.listRetry }));
    expect(await screen.findByText(/Show PM10/)).toBeInTheDocument();
    expect(reads).toBe(2);
  });
});

class FakeSource {
  static CLOSED = 2;
  readyState = 1;
  onerror: (() => void) | null = null;
  constructor(public url: string) {}
  addEventListener() {}
  close() {
    this.readyState = FakeSource.CLOSED;
  }
}

describe("PairedTabBar", () => {
  it("keeps the bar and says why when the server refuses the disconnect", async () => {
    vi.stubGlobal("EventSource", FakeSource);
    sessionStorage.setItem("jc.pairedTab", JSON.stringify({ connector: "abc", name: "doprava-in-claude", tab: "0123456789abcdef0123456789abcdef" }));
    renderPart(<PairedTabBar />, {
      answer: async (url, request) => {
        if (request.method === "DELETE") return json({ title: "Forbidden", status: 403, detail: "the pairing is not yours" }, 403);
        if (url.pathname.endsWith("/auth/me")) return json({ subject: "s", username: "jana.kovacova", roles: [] });
        return undefined;
      },
    });
    const bar = await screen.findByRole("region", { name: en.pairedTab.region });
    await act(async () => {
      await userEvent.setup().click(within(bar).getByRole("button", { name: en.pairedTab.disconnect }));
    });
    await waitFor(() => expect(within(bar).getByText(/the pairing is not yours/)).toBeInTheDocument());
    expect(within(bar).getByRole("button", { name: en.pairedTab.disconnect })).toBeInTheDocument();
  });
});
