// covers (T-2137, the module gate in gate_modules.test.ts): src/pages/endpoints/AccessRequest.tsx.
/**
 * T-5615, EP-55, PF-58: a person who reads an Endpoint but gets nothing through it asks its
 * space's owners for a read of the types it serves, sees where the request stands, and reads the
 * server's refusal beside the field it names; the owners see the person's request in their inbox.
 */
import type { JSX } from "react";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import {
  EndpointAccessRequest,
  personRequestBody,
} from "../src/pages/endpoints/AccessRequest";
import { AccessRequestInbox } from "../src/pages/pipelines/AccessRequests";
import type { AccessRequestView } from "../src/pages/pipelines/AccessRequests";
import { InRouter } from "./pageHarness";

const R = en.endpoints.request;
const PATH =
  "/api/v1/projects/banskabystrica/endpoints/banskabystrica-mesto/access-requests";

const mine = (state: AccessRequestView["state"]): AccessRequestView => ({
  id: "ar-0a1b2c3d4e5f",
  pipeline: null,
  person: "demo.viewer",
  account: "demo.viewer",
  endpoint: { project: "banskabystrica", name: "banskabystrica-mesto" },
  space: "mesto",
  operations: "read",
  types: [{ type: "AirQualityObserved", attrs: [] }],
  until: "2027-01-31T23:59:59Z",
  reason: "weekly air report",
  state,
  requestedBy: "demo.viewer",
  requestedAt: "2026-10-25T08:00:00Z",
  decidedBy: state === "pending" ? null : "demo.steward",
  decidedAt: null,
  decisionReason: state === "declined" ? "use the open endpoint" : null,
  change: null,
});

describe("the body a person's form builds (EP-55)", () => {
  it("names the first missing field, then sends types, end and reason", () => {
    expect(personRequestBody({ types: [], until: "", reason: "x" })).toEqual({
      missing: "types",
    });
    expect(
      personRequestBody({ types: ["Device"], until: "", reason: " " }),
    ).toEqual({ missing: "reason" });
    expect(
      personRequestBody({ types: ["Device"], until: "", reason: " why " }),
    ).toEqual({ body: { types: ["Device"], reason: "why" } });
    const dated = personRequestBody({
      types: ["Device"],
      until: "2027-01-31",
      reason: "why",
    }) as { body: { until: string } };
    expect(dated.body.until).toMatch(/^2027-0[12]-/);
  });
});

describe("a person's access request on the Endpoint page (EP-55)", () => {
  let sent: { path: string; body: unknown }[];
  function serve(
    answer: {
      types: string[];
      refusal: string | null;
      items: AccessRequestView[];
    },
    refusal?: { status: number; detail: string; errors?: string[] },
  ) {
    sent = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL) => {
        const req = input as Request;
        const url = new URL(req.url);
        const json = (body: unknown, status = 200) =>
          new Response(JSON.stringify(body), {
            status,
            headers: { "Content-Type": "application/json" },
          });
        if (req.method === "POST") {
          const text = await req.text();
          sent.push({
            path: url.pathname,
            body: text ? JSON.parse(text) : null,
          });
          if (refusal) {
            return new Response(
              JSON.stringify({ title: "Unprocessable", ...refusal }),
              {
                status: refusal.status,
                headers: { "Content-Type": "application/problem+json" },
              },
            );
          }
          return json(mine("pending"), 201);
        }
        if (
          url.pathname.includes("/endpoints/") &&
          url.pathname.endsWith("/access-requests")
        )
          return json(answer);
        if (url.pathname.endsWith("/access-requests"))
          return json({ items: answer.items });
        return json({ items: [] });
      }),
    );
  }
  const wrap = (node: JSX.Element) =>
    render(
      <QueryClientProvider
        client={
          new QueryClient({ defaultOptions: { queries: { retry: false } } })
        }
      >
        <I18nextProvider i18n={i18n}>
          <InRouter>{node}</InRouter>
        </I18nextProvider>
      </QueryClientProvider>,
    );
  const page = () =>
    wrap(
      <EndpointAccessRequest
        project="banskabystrica"
        name="banskabystrica-mesto"
      />,
    );

  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("files a request for a type the server offers, refusing an empty one in place", async () => {
    serve({
      types: ["AirQualityObserved", "Device"],
      refusal: null,
      items: [],
    });
    page();
    await userEvent.click(await screen.findByRole("button", { name: R.send }));
    expect(await screen.findByText(R.missing.types)).toBeInTheDocument();
    expect(sent).toHaveLength(0);
    expect(screen.getByText(R.hint.types)).toBeInTheDocument();
    expect(screen.getByText(R.hint.reason)).toBeInTheDocument();
    expect(screen.getByText(R.hint.until)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("combobox", { name: R.types }));
    await userEvent.click(
      await screen.findByRole("option", { name: /AirQualityObserved/ }),
    );
    await userEvent.type(
      screen.getByLabelText(new RegExp(`^${R.reason}`)),
      "weekly air report",
    );
    await userEvent.click(screen.getByRole("button", { name: R.send }));
    await waitFor(() => expect(sent).toHaveLength(1));
    expect(sent[0]).toEqual({
      path: PATH,
      body: { types: ["AirQualityObserved"], reason: "weekly air report" },
    });
    expect(await screen.findByText(R.sent)).toBeInTheDocument();
  });

  it("shows the server's refusal beside the field it names", async () => {
    serve(
      { types: ["AirQualityObserved"], refusal: null, items: [] },
      {
        status: 422,
        detail: "the request is not complete (EP-55)",
        errors: ["until: until has passed; choose a later end or none"],
      },
    );
    page();
    await userEvent.click(
      await screen.findByRole("combobox", { name: R.types }),
    );
    await userEvent.click(
      await screen.findByRole("option", { name: /AirQualityObserved/ }),
    );
    await userEvent.type(
      screen.getByLabelText(new RegExp(`^${R.reason}`)),
      "report",
    );
    await userEvent.click(screen.getByRole("button", { name: R.send }));
    expect(
      await screen.findByText("until has passed; choose a later end or none"),
    ).toBeInTheDocument();
  });

  it("shows where an open request stands instead of a second form", async () => {
    serve({
      types: ["AirQualityObserved"],
      refusal: null,
      items: [mine("approved")],
    });
    page();
    expect(
      await screen.findByText(en.pipelines.access.state.approved),
    ).toBeInTheDocument();
    expect(screen.getByText(R.approvedHint)).toBeInTheDocument();
    expect(
      screen.getByText(/AirQualityObserved \(all attributes\)/),
    ).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: R.send })).toBeNull();
  });

  it("says why nothing can be asked for: no model, or an Endpoint that names its policies", async () => {
    serve({ types: [], refusal: null, items: [] });
    page();
    expect(await screen.findByText(R.noTypes)).toBeInTheDocument();
    vi.unstubAllGlobals();
    serve({
      types: ["Device"],
      refusal: "this endpoint serves only the policies it names",
      items: [],
    });
    wrap(<EndpointAccessRequest project="banskabystrica" name="other" />);
    expect(
      await screen.findByText(/serves only the policies it names/),
    ).toBeInTheDocument();
  });

  it("lists a person's request in the owners' inbox with its decision buttons", async () => {
    serve({ types: [], refusal: null, items: [mine("pending")] });
    wrap(<AccessRequestInbox project="banskabystrica" space="mesto" />);
    expect(await screen.findByText("demo.viewer")).toBeInTheDocument();
    expect(
      screen.getByRole("button", {
        name: en.pipelines.access.approveOf.replace("{account}", "demo.viewer"),
      }),
    ).toBeInTheDocument();
  });
});
