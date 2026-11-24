
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import i18n from "../src/i18n";
import { AiConnectorsPage, CreateDialog, NothingToReach, PresetsField, ScopeTree } from "../src/pages/mcp/AiConnectorsPage";
import type { ConnectorOptions } from "../src/pages/mcp/aiConnectors";
import { emptyForm } from "../src/pages/mcp/aiConnectors";

const listeners = new Set<() => void>();
const pairedState = vi.hoisted(() => ({ connector: null as string | null }));

vi.mock("../src/connectors/pairedTab", () => ({
  connectTab: vi.fn(async (id: string) => {
    pairedState.connector = id;
    listeners.forEach((l) => l());
    return undefined;
  }),
  disconnectTab: vi.fn(async () => {
    pairedState.connector = null;
    listeners.forEach((l) => l());
    return undefined;
  }),
  onPairingChange: vi.fn((listener: () => void) => {
    listeners.add(listener);
    return () => listeners.delete(listener);
  }),
  pairing: vi.fn(() => (pairedState.connector ? { connector: pairedState.connector } : null)),
}));
const NOW = new Date("2026-10-28T12:00:00Z");

const BASE_OPTIONS: ConnectorOptions = {
  segments: ["data-read", "context", "pipeline"],
  segmentKinds: [
    { segment: "context", kinds: ["ContextSpace", "DataModel", "Endpoint"] },
    { segment: "pipeline", kinds: ["Pipeline"] },
  ],
  ceiling: {
    segments: ["data-read", "context", "pipeline"],
    writes: true,
    maxDays: 30,
    kinds: [
      { kind: "Endpoint", verbs: ["read", "create"] },
      { kind: "Pipeline", verbs: ["read", "run"] },
    ],
  },
  projects: [{ project: "doprava", spaces: ["parking"], endpoints: ["bike-counters"], pipelines: ["p1"], allowed: true }],
  clients: [{ id: "mcp-local", isBoundary: false }],
};

function renderWithClient(node: React.ReactNode) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>{node}</I18nextProvider>
    </QueryClientProvider>,
  );
}

describe("AiConnectorsPage additional coverage", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    pairedState.connector = null;
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("handles retry when connector list fails to cover line 83", async () => {
    let callCount = 0;
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL) => {
        const url = new URL(typeof input === "string" ? input : (input as Request).url);
        if (url.pathname === "/api/v1/connectors") {
          callCount += 1;
          if (callCount === 1) {
            return new Response(JSON.stringify({ status: 500, detail: "Error listing connectors" }), {
              status: 500,
              headers: { "Content-Type": "application/problem+json" },
            });
          }
          return new Response(JSON.stringify({ items: [] }), { headers: { "Content-Type": "application/json" } });
        }
        return new Response("{}", { status: 404 });
      }),
    );

    renderWithClient(<AiConnectorsPage />);
    const retryBtn = await screen.findByRole("button", { name: /Retry/i });
    fireEvent.click(retryBtn);

    expect(await screen.findByText("No AI connectors yet")).toBeInTheDocument();
  });

  it("opens and cancels CreateDialog from the page header to cover line 106", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL) => {
        const url = new URL(typeof input === "string" ? input : (input as Request).url);
        if (url.pathname === "/api/v1/connectors") {
          return new Response(JSON.stringify({ items: [] }), { headers: { "Content-Type": "application/json" } });
        }
        if (url.pathname === "/api/v1/connectors/options") {
          return new Response(JSON.stringify(BASE_OPTIONS), { headers: { "Content-Type": "application/json" } });
        }
        return new Response("{}", { status: 404 });
      }),
    );

    const user = userEvent.setup();
    renderWithClient(<AiConnectorsPage />);

    const newBtn = await screen.findByRole("button", { name: "New AI connector" });
    await user.click(newBtn);

    const dialog = await screen.findByRole("dialog");
    expect(dialog).toBeInTheDocument();

    const cancelBtn = within(dialog).getByRole("button", { name: "Cancel" });
    await user.click(cancelBtn);

    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("covers ScopeTree setAccess when changing access to write (lines 354, 372)", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    const form = {
      ...emptyForm(BASE_OPTIONS, NOW),
      picks: [{ project: "doprava", kind: "endpoint" as const, name: "bike-counters", access: "read" as const }],
    };

    renderWithClient(<ScopeTree options={BASE_OPTIONS} form={form} onChange={onChange} />);

    const accessSelect = screen.getByRole("combobox", { name: "Access for bike-counters" });
    expect(accessSelect).toHaveValue("read");

    await user.selectOptions(accessSelect, "write");
    expect(onChange).toHaveBeenCalledWith([
      { project: "doprava", kind: "endpoint", name: "bike-counters", access: "write" },
    ]);
  });

  it("covers PresetsField unticking a segment (line 587)", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    const form = {
      ...emptyForm(BASE_OPTIONS, NOW),
      segments: ["data-read", "context"],
    };

    renderWithClient(<PresetsField options={BASE_OPTIONS} form={form} readsOnly={false} onChange={onChange} />);

    const dataReadCheckbox = screen.getByRole("checkbox", { name: /^Data/i });
    expect(dataReadCheckbox).toBeChecked();

    await user.click(dataReadCheckbox);
    expect(onChange).toHaveBeenCalledWith(["context"]);
  });

  it("covers NothingToReach dialog onOpenChange (line 668)", async () => {
    const onClose = vi.fn();
    renderWithClient(<NothingToReach onClose={onClose} />);

    // Press Escape to trigger onOpenChange(false)
    fireEvent.keyDown(document, { key: "Escape" });
    expect(onClose).toHaveBeenCalled();
  });

  it("covers CreateDialog error on options fetch and retry (line 763)", async () => {
    let callCount = 0;
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL) => {
        const url = new URL(typeof input === "string" ? input : (input as Request).url);
        if (url.pathname === "/api/v1/connectors/options") {
          callCount += 1;
          if (callCount === 1) {
            return new Response(JSON.stringify({ status: 500, detail: "Options failed" }), {
              status: 500,
              headers: { "Content-Type": "application/problem+json" },
            });
          }
          return new Response(JSON.stringify(BASE_OPTIONS), { headers: { "Content-Type": "application/json" } });
        }
        return new Response("{}", { status: 404 });
      }),
    );

    renderWithClient(<CreateDialog onClose={vi.fn()} onCreated={vi.fn()} />);

    const retryBtn = await screen.findByRole("button", { name: /Retry/i });
    fireEvent.click(retryBtn);

    expect(await screen.findByLabelText(/Name/i)).toBeInTheDocument();
  });
});
