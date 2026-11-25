// T-7706: ModelFileDrop: an address read that throws a plain error says "network", and the drafted fields'
// class column, once revealed, is searched like the others.
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { ModelFileDrop } from "../src/pages/models/ModelFileDrop";

const operation = vi.hoisted(() => ({ fail: false }));
vi.mock("../src/api/operations", async (original) => {
  const actual = await original<typeof import("../src/api/operations")>();
  return {
    ...actual,
    callOperation: (...args: Parameters<typeof actual.callOperation>) => {
      if (operation.fail) return Promise.reject(new Error("the door fell over"));
      return actual.callOperation(...args);
    },
  };
});

const ANSWER = {
  linkml: "name: sensors\n",
  operations: [
    { op: "addClass", name: "Sensors", is_a: "Entity" },
    { op: "addSlot", name: "temp", class: "Sensors", range: "float" },
    { op: "addSlot", name: "plate", class: "Sensors", range: "string" },
  ],
  detectedTypes: { temp: "float", plate: "string" },
  matches: {},
  untyped: [],
  rows: 2,
};

function mount() {
  vi.stubGlobal("fetch", (input: RequestInfo | URL) => {
    const url = input instanceof Request ? input.url : String(input);
    const body = url.includes("/organizations") ? { apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] } : ANSWER;
    return Promise.resolve(new Response(JSON.stringify(body), { status: 200, headers: { "Content-Type": "application/json" } }));
  });
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>
        <ModelFileDrop project="helsinki" onPopulate={vi.fn()} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

afterEach(() => {
  operation.fail = false;
  vi.unstubAllGlobals();
});

describe("ModelFileDrop edges (T-7706)", () => {
  it("says the address could not be read over the network when the operation door throws a plain error", async () => {
    operation.fail = true;
    mount();
    await userEvent.type(screen.getByLabelText(en.models.infer.url), "https://api.example.org/odd{Enter}");
    await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent(en.models.infer.failed.replace("{detail}", "network")));
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("searches the class column once it is shown, and a needle in no column leaves no field", async () => {
    mount();
    const file = new File(["id,temp,plate\n1,21.5,AB\n"], "sensors.csv", { type: "text/csv" });
    await userEvent.upload(screen.getByLabelText(en.models.infer.chooseFile), file);
    const dialog = await screen.findByRole("dialog");
    const table = within(dialog).getByRole("table", { name: en.models.infer.title });
    await userEvent.click(within(dialog).getByRole("button", { name: en.entityGrid.arrangeColumns }));
    const panel = screen.getByRole("dialog", { name: en.entityGrid.arrangeColumns });
    await userEvent.click(within(panel).getByRole("checkbox", { name: `${en.entityGrid.showColumn}: ${en.models.infer.class}` }));
    const search = within(dialog).getByRole("searchbox", { name: en.dataGrid.searchRows });
    await userEvent.type(search, "Sensors");
    expect(within(table).getAllByText("temp").length).toBeGreaterThan(0);
    expect(within(table).getAllByText("plate").length).toBeGreaterThan(0);
    await userEvent.clear(search);
    await userEvent.type(search, "Nonexistent");
    expect(within(table).queryAllByText("plate")).toHaveLength(0);
  });
});
