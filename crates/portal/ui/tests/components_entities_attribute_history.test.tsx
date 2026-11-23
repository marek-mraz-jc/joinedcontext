/**
 * AttributeHistory branches the first suite leaves open: the retry of a failed read, the inline
 * correction's value field, and paging to older measurements and back to the newest.
 */
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { AttributeHistory } from "../src/components/entities/history/AttributeHistory";

const SEEN: Request[] = [];

const day = (n: number) => `2026-10-${String(n).padStart(2, "0")}T07:00:00Z`;
const reading = (n: number, value: number) => ({ type: "Property", value, observedAt: day(n), instanceId: `urn:i:${n}` });

function show() {
  render(
    <I18nextProvider i18n={i18n}>
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <AttributeHistory slug="slug" entityId="urn:ngsi-ld:S:1" attributes={["pm10"]} />
      </QueryClientProvider>
    </I18nextProvider>,
  );
}

describe("AttributeHistory branches", () => {
  beforeEach(async () => {
    SEEN.length = 0;
    await i18n.changeLanguage("en");
  });
  afterEach(() => vi.restoreAllMocks());

  it("asks the broker again when Retry is pressed after a failed read", async () => {
    let calls = 0;
    vi.spyOn(globalThis, "fetch").mockImplementation(async (input) => {
      SEEN.push(input as Request);
      calls += 1;
      if (calls === 1) return new Response(JSON.stringify({ detail: "broker down" }), { status: 500 });
      return new Response(JSON.stringify({ pm10: [reading(1, 12)] }), { status: 200 });
    });
    show();
    expect(await screen.findByText(/broker down/)).toBeTruthy();
    await userEvent.click(screen.getByRole("button", { name: i18n.t("app.error.retry") }));
    expect(await screen.findByText(/12/, { selector: "li *" })).toBeTruthy();
    expect(screen.queryByText(/broker down/)).toBeNull();
    expect(SEEN.filter((request) => request.method === "GET")).toHaveLength(2);
  });

  it("corrects a measurement inline: the value typed is what is written at its instant", async () => {
    vi.spyOn(globalThis, "fetch").mockImplementation(async (input) => {
      const request = input as Request;
      SEEN.push(request);
      if (request.method === "GET") return new Response(JSON.stringify({ pm10: [reading(1, 12)] }), { status: 200 });
      return new Response(null, { status: 204 });
    });
    show();
    await userEvent.click(await screen.findByRole("button", { name: "Correct" }));
    const form = screen.getByRole("region", { name: /Correct measurement/ });
    const value = within(form).getByRole("textbox", { name: /Value/ });
    await userEvent.clear(value);
    await userEvent.type(value, "20");
    await userEvent.type(within(form).getByRole("textbox", { name: /Reason/ }), "Typo");
    await userEvent.click(within(form).getByRole("button", { name: /Save/ }));
    await waitFor(() => expect(SEEN.filter((request) => request.method === "PATCH")).toHaveLength(1));
    const patch = SEEN.find((request) => request.method === "PATCH")!;
    expect(patch.headers.get("JC-Reason")).toBe("Typo");
    expect((await patch.json()).pm10).toMatchObject({ value: 20, observedAt: day(1) });
  });

  it("pages to the measurements before the oldest shown, and back to the newest", async () => {
    vi.spyOn(globalThis, "fetch").mockImplementation(async (input) => {
      const request = input as Request;
      SEEN.push(request);
      const older = new URL(request.url).searchParams.get("timerel") === "before";
      const body = older ? [reading(1, 100)] : Array.from({ length: 9 }, (_, i) => reading(i + 2, 200 + i));
      return new Response(JSON.stringify({ pm10: body }), { status: 200 });
    });
    show();
    await userEvent.click(await screen.findByRole("button", { name: "Older" }));
    await waitFor(() => expect(SEEN.some((request) => new URL(request.url).searchParams.get("timerel") === "before")).toBe(true));
    const asked = SEEN.find((request) => new URL(request.url).searchParams.get("timerel") === "before")!;
    expect(new URL(asked.url).searchParams.get("timeAt")).toBe(day(2));
    expect(screen.queryByRole("button", { name: "Older" })).toBeNull();
    await userEvent.click(await screen.findByRole("button", { name: "Newest" }));
    expect(await screen.findByRole("button", { name: "Older" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Newest" })).toBeNull();
  });
});
