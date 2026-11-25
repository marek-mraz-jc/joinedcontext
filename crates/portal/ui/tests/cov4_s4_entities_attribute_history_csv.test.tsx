// T-7706: AttributeHistory CSV import: choosing no file changes nothing, and a row the gateway refuses names its line.
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { AttributeHistory } from "../src/components/entities/history/AttributeHistory";

const SEEN: Request[] = [];
const VERSIONS = [{ type: "Property", value: 14, observedAt: "2026-10-18T07:00:00Z", instanceId: "urn:i:1" }];

function show() {
  vi.spyOn(globalThis, "fetch").mockImplementation(async (input) => {
    const request = input as Request;
    SEEN.push(request);
    if (request.method === "GET") return new Response(JSON.stringify({ pm10: VERSIONS }), { status: 200 });
    return new Response(JSON.stringify({ detail: "no grant" }), { status: 403 });
  });
  render(
    <I18nextProvider i18n={i18n}>
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <AttributeHistory slug="slug" entityId="urn:ngsi-ld:S:1" attributes={["pm10"]} />
      </QueryClientProvider>
    </I18nextProvider>,
  );
}

describe("AttributeHistory CSV import edges (T-7706)", () => {
  beforeEach(async () => {
    SEEN.length = 0;
    await i18n.changeLanguage("en");
  });
  afterEach(() => vi.restoreAllMocks());

  it("ignores a file dialog closed without a file, and keeps the confirm button off", async () => {
    show();
    await screen.findByText(/14/, { selector: "li *" });
    const csv = screen.getByRole("region", { name: i18n.t("attributeHistory.importCsv") });
    const input = within(csv).getByLabelText(/CSV file/);
    fireEvent.change(input, { target: { files: [] } });
    expect(within(csv).queryByRole("status")).toBeNull();
    expect(SEEN.filter((request) => request.method !== "GET")).toEqual([]);
  });

  it("names the line of the row the gateway refused and keeps the preview for a second try", async () => {
    show();
    await screen.findByText(/14/, { selector: "li *" });
    const csv = screen.getByRole("region", { name: i18n.t("attributeHistory.importCsv") });
    const file = new File(["observedAt,value\n2026-03-03T14:00:00Z,9\n"], "old.csv", { type: "text/csv" });
    await userEvent.upload(within(csv).getByLabelText(/CSV file/), file);
    await within(csv).findByText(i18n.t("attributeHistory.csvPreview", { added: 1, duplicates: 0, corrections: 0 }));
    await userEvent.type(within(csv).getByRole("textbox", { name: /Reason/ }), "Paper log");
    await userEvent.click(within(csv).getByRole("button", { name: i18n.t("attributeHistory.confirmImport") }));
    await waitFor(() => expect(within(csv).getByText(/was not saved/)).toBeInTheDocument());
    expect(within(csv).getByText(/line 2/)).toBeInTheDocument();
    expect(within(csv).getByText(/no grant/)).toBeInTheDocument();
    expect(within(csv).getByRole("status")).toBeInTheDocument();
  });
});
