// T-7706: AccessRequest.tsx edges: the lines of a refusal sorted to their field, the retry of a failed read, the end date and the
// reason field clearing the "write why" mark.
import { fireEvent, render, screen, waitFor, cleanup } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { ApiError } from "../src/api/client";
import { EndpointAccessRequest, fieldErrors } from "../src/pages/endpoints/AccessRequest";
import { InRouter } from "./pageHarness";
import { doorsAsRest } from "./odata";

const R = en.endpoints.request;

describe("fieldErrors", () => {
  it("sorts the lines of a refusal to the field each names and drops what names none", () => {
    const err = new ApiError(422, "refused", { errors: [{ path: "types", message: "object line" }, 5, "types[0]: unknown type", "until: in the past", "other: x"] } as never);
    expect(fieldErrors(err)).toEqual({ types: ["object line", "unknown type"], until: ["in the past"] });
    expect(fieldErrors(new Error("plain"))).toEqual({});
  });
});

function serve(failFirstRead: boolean) {
  let reads = 0;
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const req = input as Request;
      const url = new URL(req.url);
      const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
      if (url.pathname.endsWith("/access-requests") && req.method === "GET") {
        reads += 1;
        if (failFirstRead && reads === 1) return json({ title: "x", detail: "the owners cannot be reached", status: 500 }, 500);
        return json({ types: ["AirQualityObserved"], refusal: null, items: [] });
      }
      return json({ items: [] });
    }),
  );
  doorsAsRest();
}

function page() {
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>
        <InRouter>
          <EndpointAccessRequest project="banskabystrica" name="banskabystrica-mesto" />
        </InRouter>
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("EndpointAccessRequest edges", () => {
  it("reads again when the person retries a failed read", async () => {
    serve(true);
    page();
    expect(await screen.findByText(/the owners cannot be reached/)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: R.retry }));
    expect(await screen.findByRole("button", { name: R.send })).toBeInTheDocument();
  });

  it("takes an end date and clears the missing-reason mark once a reason is typed", async () => {
    serve(false);
    page();
    await userEvent.click(await screen.findByRole("button", { name: R.send }));
    await userEvent.click(screen.getByRole("combobox", { name: R.types }));
    await userEvent.click(await screen.findByRole("option", { name: /AirQualityObserved/ }));
    const until = screen.getByLabelText(/^Until/);
    fireEvent.change(until, { target: { value: "2027-01-31" } });
    expect(until).toHaveValue("2027-01-31");
    await userEvent.click(screen.getByRole("button", { name: R.send }));
    expect(await screen.findByText(R.missing.reason)).toBeInTheDocument();
    await userEvent.type(screen.getByLabelText(new RegExp(`^${R.reason}`)), "w");
    await waitFor(() => expect(screen.queryByText(R.missing.reason)).not.toBeInTheDocument());
  });
});
