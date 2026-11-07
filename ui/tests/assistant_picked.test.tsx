/**
 * SDK-46: the element a person pointed at in the preview shows in the dock, rides on the next
 * message to its run as `scope`, and is gone after it, so the message after edits everything again.
 */
import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { RouterProvider, createRootRoute, createRouter } from "@tanstack/react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { AssistantDock } from "../src/assistant/AssistantDock";
import { pickElement, pickedElement, rememberRun } from "../src/assistant/state";

const PROJECT = "helsinki";
const RUN_ID = "01J8ZQ4T7K9M2N3P4Q5R6S7T8V";

function renderDock() {
  const sent: Record<string, unknown>[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const request = input as Request;
      const url = new URL(request.url, "http://localhost");
      const json = (body: unknown, code = 200) =>
        new Response(JSON.stringify(body), { status: code, headers: { "Content-Type": "application/json" } });
      if (request.method === "POST" && url.pathname.endsWith(`/agent-runs/${RUN_ID}/messages`)) {
        sent.push(JSON.parse(await request.text()) as Record<string, unknown>);
        return new Response(null, { status: 204 });
      }
      if (url.pathname.endsWith(`/agent-runs/${RUN_ID}`)) {
        return json({
          id: RUN_ID,
          project: PROJECT,
          kind: "application",
          appName: "bike-stations",
          endpointName: "bikes",
          prompt: "Show the bike stations",
          status: "awaiting_approval",
          steps: 3,
          tokensUsed: 10,
          createdBy: "demo.steward",
          createdAt: "2026-10-10T05:00:00Z",
          endpoints: [],
        });
      }
      if (url.pathname.endsWith("/endpoints")) {
        return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
      }
      return json({});
    }),
  );
  vi.stubGlobal(
    "EventSource",
    class {
      addEventListener(): void {}
      removeEventListener(): void {}
      close(): void {}
    } as unknown as typeof EventSource,
  );
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const router = createRouter({
    routeTree: createRootRoute({ component: () => <AssistantDock project={PROJECT} /> }),
  });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <RouterProvider router={router} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return sent;
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});
afterEach(() => {
  pickElement(RUN_ID, null);
  rememberRun(null);
  vi.unstubAllGlobals();
});

describe("the pointed element in the dock (SDK-46)", () => {
  it("scopes_the_next_message_to_the_pointed_file_and_widens_after_it", async () => {
    const sent = renderDock();
    await act(async () => {
      rememberRun({ project: PROJECT, runId: RUN_ID });
      pickElement(RUN_ID, "src/pages/Card.tsx:4");
    });
    const chip = await screen.findByTestId("assistant-picked");
    expect(chip.textContent).toContain("src/pages/Card.tsx:4");
    const user = userEvent.setup();

    await user.type(await screen.findByPlaceholderText(en.agentRun.conversation.placeholder), "Make the title red{Enter}");
    await waitFor(() => expect(sent).toHaveLength(1));
    expect(sent[0]).toMatchObject({ text: "Make the title red", scope: "src/pages/Card.tsx:4" });
    await waitFor(() => expect(screen.queryByTestId("assistant-picked")).toBeNull());
    expect(pickedElement(RUN_ID)).toBeNull();

    await user.type(screen.getByPlaceholderText(en.agentRun.conversation.placeholder), "Now the footer{Enter}");
    await waitFor(() => expect(sent).toHaveLength(2));
    expect(sent[1].scope).toBeUndefined();
  });

  it("drops_the_pointed_element_when_the_person_widens_it", async () => {
    renderDock();
    await act(async () => {
      rememberRun({ project: PROJECT, runId: RUN_ID });
      pickElement(RUN_ID, "src/App.tsx:9");
    });
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: en.assistant.pickedClear }));
    expect(screen.queryByTestId("assistant-picked")).toBeNull();
    expect(pickedElement(RUN_ID)).toBeNull();
  });

  it("keeps_another_runs_pick_out_of_this_dock", async () => {
    renderDock();
    await act(async () => {
      rememberRun({ project: PROJECT, runId: RUN_ID });
      pickElement("01J8ZQ4T7K9M2N3P4Q5R6S7T8W", "src/App.tsx:9");
    });
    await screen.findByPlaceholderText(en.agentRun.conversation.placeholder);
    expect(screen.queryByTestId("assistant-picked")).toBeNull();
  });
});
