/**
 * T-5699, AG-63: the page behind an AI client's MCP question. The person reads what the call
 * would do and records accept or decline with their session; a question that is not open for
 * them says what to do next and offers no button.
 */
// covers (T-2137, the module gate in gate_modules.test.ts): src/pages/mcp/McpElicitationPage.tsx.
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { McpConfirmationPage, McpElicitationPage } from "../src/pages/mcp/McpElicitationPage";
import { expectNoViolations } from "./checks";
import { InRouter } from "./pageHarness";

const ID = "eli-0a1b2c3d4e5f6a7b";
const PAGE = `/api/v1/projects/city/mcp-elicitations/${ID}`;

function serve(status: number, consumedOnceAnswered = false) {
  const posted: unknown[] = [];
  let question: Record<string, unknown> = {
    id: ID,
    operation: "jc_resource_delete",
    message: "Delete Resource in project 'city' (red lane).",
    resource: "/projects/city/endpoints/air",
    expiresAt: 1_900_000_000,
    decision: null,
  };
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const request = input as Request;
      const url = new URL(request.url ?? String(input), "http://localhost");
      const reply = (body: unknown, code = 200) =>
        new Response(JSON.stringify(body), { status: code, headers: { "Content-Type": "application/json" } });
      const gone = status !== 200 || (consumedOnceAnswered && question.decision !== null);
      if (url.pathname === PAGE && gone) return reply({ status: 404, title: "Not Found", detail: "no open question" }, 404);
      if (url.pathname === PAGE) return reply(question);
      if (url.pathname === `${PAGE}/decision`) {
        const body = (await request.json()) as { decision: string };
        posted.push(body);
        question = { ...question, decision: body.decision };
        return reply(question);
      }
      return reply({}, 404);
    }),
  );
  return posted;
}

const show = (client = new QueryClient({ defaultOptions: { queries: { retry: false } } })) =>
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <InRouter>
          <McpElicitationPage project="city" id={ID} />
        </InRouter>
      </I18nextProvider>
    </QueryClientProvider>,
  );

describe("the MCP question page", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("shows the call, links what it changes, and records the person's accept and decline", async () => {
    const posted = serve(200);
    const { container } = show();
    expect(await screen.findByTestId("mcp-elicitation-message")).toHaveTextContent("Delete Resource in project 'city'");
    expect(screen.getByRole("link", { name: en.mcpElicitation.look })).toHaveAttribute("href", "/projects/city/endpoints/air");
    expect(screen.getByText(en.mcpElicitation.hint)).toBeInTheDocument();
    await expectNoViolations(container);
    await userEvent.click(screen.getByRole("button", { name: en.mcpElicitation.accept }));
    expect(await screen.findByText(en.mcpElicitation.accepted)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: en.mcpElicitation.decline }));
    expect(await screen.findByText(en.mcpElicitation.declined)).toBeInTheDocument();
    expect(posted).toEqual([{ decision: "accept" }, { decision: "decline" }]);
  });

  // T-6501: the client sends its call again, the Portal consumes the answered question, and the
  // paired tab re-reads every query. The person who accepted here still reads that they accepted.
  it("keeps the person's answer once the client's call used up the question", async () => {
    serve(200, true);
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    show(client);
    await userEvent.click(await screen.findByRole("button", { name: en.mcpElicitation.accept }));
    expect(await screen.findByText(en.mcpElicitation.accepted)).toBeInTheDocument();
    await client.invalidateQueries();
    await waitFor(() => expect(client.getQueryState(["mcp-elicitation", "city", ID])?.status).toBe("error"));
    expect(screen.getByText(en.mcpElicitation.accepted)).toBeInTheDocument();
    expect(screen.queryByText(en.mcpElicitation.gone)).toBeNull();
    expect(screen.queryByRole("button", { name: en.mcpElicitation.accept })).toBeNull();
  });

  it("says what to do when the question is no longer open, with no button to press", async () => {
    serve(404);
    show();
    expect(await screen.findByText(en.mcpElicitation.gone)).toBeInTheDocument();
    expect(screen.getByText(en.mcpElicitation.goneHint)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: en.mcpElicitation.accept })).toBeNull();
  });
});

const BROKER_ID = "0a1b2c3d4e5f60718293a4b5c6d7e8f9";
const BROKER_PAGE = `/api/v1/mcp-confirmations/${BROKER_ID}`;

function serveBroker(status: number) {
  const posted: unknown[] = [];
  let question: Record<string, unknown> = {
    id: BROKER_ID,
    view: "bb-air",
    tool: "create_entity",
    message: "Create the Entity urn:ngsi-ld:AirQualityObserved:1 in view bb-air.",
    expiresAt: 1_900_000_000,
    decision: null,
  };
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const request = input as Request;
      const url = new URL(request.url ?? String(input), "http://localhost");
      const reply = (body: unknown, code = 200) =>
        new Response(JSON.stringify(body), { status: code, headers: { "Content-Type": "application/json" } });
      if (url.pathname === BROKER_PAGE && status !== 200) return reply({ status, title: "Error", detail: "no open question" }, status);
      if (url.pathname === BROKER_PAGE) return reply(question);
      if (url.pathname === `${BROKER_PAGE}/decision`) {
        const body = (await request.json()) as { decision: string };
        posted.push(body);
        question = { ...question, decision: body.decision };
        return reply(question);
      }
      return reply({}, 404);
    }),
  );
  return posted;
}

const showBroker = () =>
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>
        <InRouter>
          <McpConfirmationPage id={BROKER_ID} />
        </InRouter>
      </I18nextProvider>
    </QueryClientProvider>,
  );

describe("the broker MCP write page (T-5829)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("shows the view and tool and records accept and decline, with no subject in the body", async () => {
    const posted = serveBroker(200);
    const { container } = showBroker();
    expect(await screen.findByTestId("mcp-elicitation-message")).toHaveTextContent("AirQualityObserved:1");
    expect(screen.getByText("bb-air")).toBeInTheDocument();
    expect(screen.getByText("create_entity")).toBeInTheDocument();
    expect(screen.getByText(en.mcpConfirmation.lead)).toBeInTheDocument();
    await expectNoViolations(container);
    await userEvent.click(screen.getByRole("button", { name: en.mcpElicitation.accept }));
    expect(await screen.findByText(en.mcpElicitation.accepted)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: en.mcpElicitation.decline }));
    expect(await screen.findByText(en.mcpElicitation.declined)).toBeInTheDocument();
    expect(posted).toEqual([{ decision: "accept" }, { decision: "decline" }]);
  });

  it("says what to do when the question is not open for this person, with a way back", async () => {
    serveBroker(404);
    showBroker();
    expect(await screen.findByText(en.mcpElicitation.goneHint)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: en.mcpConfirmation.home })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: en.mcpElicitation.accept })).toBeNull();
  });

  it("offers a retry when a broker did not answer", async () => {
    serveBroker(503);
    showBroker();
    expect(await screen.findByRole("button", { name: /try again|retry/i })).toBeInTheDocument();
  });
});
