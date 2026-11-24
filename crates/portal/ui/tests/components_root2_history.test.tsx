/** A resource's history: what a change touched, paging back, and a failed read. */
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { ResourceHistory } from "../src/components/ResourceHistory";
import { odataCall, odataValue } from "./odata";

vi.mock("../src/api/permissions", async (original) => ({
  ...(await original<typeof import("../src/api/permissions")>()),
  usePermissions: () => ({ can: () => true }),
}));

function change(id: string, at: string) {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Change",
    metadata: { name: id, namespace: "helsinki" },
    status: { lane: "yellow", phase: "Merged", plan: { create: 0, update: 1, delete: 0 } },
    summary: { key: "change.summary.update", params: { kind: "Pipeline", name: "air", fields: 1 } },
    author: { name: "Aino Virtanen" },
    createdAt: at,
    decision: { by: "mikko@hel.fi", at },
  };
}

const asked: string[] = [];

function mount(answer: (url: URL, request: Request) => Response) {
  asked.length = 0;
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const request = input as Request;
      const url = new URL(request.url);
      asked.push(`${request.method} ${url.pathname}${url.search}`);
      return answer(url, request);
    }),
  );
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>
        <ResourceHistory project="helsinki" kind="Pipeline" name="air" />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

const page = (items: unknown[], next?: number) =>
  odataValue({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ChangeList", items, ...(next ? { next } : {}) });

describe("a resource's history, more paths", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("says why the history could not be read", async () => {
    mount(() => new Response(JSON.stringify({ title: "Server Error", status: 500, detail: "history store is down" }), { status: 500, headers: { "Content-Type": "application/problem+json" } }));
    expect(await screen.findByRole("alert")).toHaveTextContent("history store is down");
  });

  it("shows a lost connection in the alert", async () => {
    mount(() => {
      throw new TypeError("offline");
    });
    expect(await screen.findByRole("alert")).toHaveTextContent("offline");
  });

  it("opens and closes what a change touched, and says when it names no field", async () => {
    mount((url) => {
      if (odataCall(url)?.name === "jc_change_history") return page([change("chg-00000009", "2026-10-11T10:00:00Z")]);
      return new Response(
        JSON.stringify({ ...change("chg-00000009", "2026-10-11T10:00:00Z"), planFields: [{ path: "spec.mapping", from: "a", to: "b" }, { path: "spec.name", to: "air" }] }),
        { status: 200, headers: { "Content-Type": "application/json" } },
      );
    });
    const row = await screen.findByTestId("resource-history-item");
    const toggle = within(row).getByRole("button", { name: en.approvals.history.whatChanged });
    await userEvent.click(toggle);
    expect(await within(row).findByText("changed Mapping from a to b")).toBeInTheDocument();
    expect(within(row).getByText("set Name to air")).toBeInTheDocument();
    expect(toggle).toHaveAttribute("aria-expanded", "true");
    await userEvent.click(toggle);
    expect(within(row).queryByText("set Name to air")).not.toBeInTheDocument();
    expect(toggle).toHaveAttribute("aria-expanded", "false");
  });

  it("says when the change's fields cannot be read", async () => {
    mount((url) =>
      odataCall(url)?.name === "jc_change_history"
        ? page([change("chg-00000009", "2026-10-11T10:00:00Z")])
        : new Response(JSON.stringify({ title: "Not Found", status: 404, detail: "gone" }), { status: 404, headers: { "Content-Type": "application/problem+json" } }),
    );
    const row = await screen.findByTestId("resource-history-item");
    await userEvent.click(within(row).getByRole("button", { name: en.approvals.history.whatChanged }));
    expect(await within(row).findByText(en.approvals.history.noFields)).toBeInTheDocument();
  });

  it("asks for the next page on Older and lists both", async () => {
    mount((url) => {
      const call = odataCall(url);
      if (call?.name !== "jc_change_history") return new Response("{}", { status: 404 });
      return call.params.page === 2
        ? page([change("chg-00000001", "2026-10-01T10:00:00Z")])
        : page([change("chg-00000002", "2026-10-02T10:00:00Z")], 2);
    });
    expect(await screen.findAllByTestId("resource-history-item")).toHaveLength(1);
    await userEvent.click(screen.getByRole("button", { name: en.approvals.history.older }));
    expect(await screen.findAllByTestId("resource-history-item")).toHaveLength(2);
    expect(screen.queryByRole("button", { name: en.approvals.history.older })).not.toBeInTheDocument();
    expect(asked.filter((line) => line.includes("jc_change_history"))).toHaveLength(2);
  });
});
