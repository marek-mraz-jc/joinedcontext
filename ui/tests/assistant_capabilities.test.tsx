/**
 * T-2718, AG-92: what the assistant may do in one conversation, chosen beside "Add endpoint". A
 * preset and each endpoint's access travel with the start and with every message; a path the
 * preset leaves out is disabled with why; an endpoint's write switch is off with why when the
 * preset is Read or the person's own grant does not write there. Nothing chosen sends nothing, so
 * the server narrows nothing (the profile and the grants still do, AG-70).
 */
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { RouterProvider, createRootRoute, createRouter } from "@tanstack/react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { AssistantDock } from "../src/assistant/AssistantDock";
import { accessFor, rememberCapabilities, storedCapabilities } from "../src/assistant/Capabilities";
import { rememberEndpoints } from "../src/assistant/EndpointPicker";
import { rememberRun } from "../src/assistant/state";
import { expectNoViolations } from "./checks";

const PROJECT = "helsinki";
const RUN_ID = "01J8ZQ4T7K9M2N3P4Q5R6S7T8V";
const words = en.assistant.capabilities;

function endpoint(name: string) {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Endpoint",
    metadata: { name, project: PROJECT },
    spec: { slug: `${name}-slug`, contextSpaceRef: "helsinki", audience: "public" },
  };
}

/** bikes: the person's grant writes there; air: it only reads. */
const GRANTS: Record<string, string[]> = {
  "bikes-slug": ["queryEntity", "updateAttrs"],
  "air-slug": ["queryEntity"],
};

class StubEventSource {
  addEventListener(): void {}
  removeEventListener(): void {}
  close(): void {}
}

function renderDock() {
  const started: Record<string, unknown>[] = [];
  const messages: Record<string, unknown>[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const request = input as Request;
      const url = new URL(request.url, "http://localhost");
      const json = (body: unknown, status = 200) =>
        new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
      const access = /^\/api\/endpoint\/([^/]+)\/access$/.exec(url.pathname);
      if (access) {
        return json({ permissions: [{ resource: { type: "*" }, actions: GRANTS[access[1]] ?? [], attributes: "*" }] });
      }
      if (url.pathname.endsWith("/auth/me")) return json({ subject: "s", username: "demo.steward", roles: ["domain-editor"] });
      if (request.method === "POST" && url.pathname.endsWith("/assistant/conversations")) {
        started.push(JSON.parse(await request.text()) as Record<string, unknown>);
        return json({ id: RUN_ID, status: "queued" }, 202);
      }
      if (request.method === "POST" && url.pathname.endsWith(`/agent-runs/${RUN_ID}/messages`)) {
        messages.push(JSON.parse(await request.text()) as Record<string, unknown>);
        return json({}, 202);
      }
      if (url.pathname.endsWith(`/agent-runs/${RUN_ID}`)) {
        return json({
          id: RUN_ID,
          project: PROJECT,
          kind: "conversation",
          appName: "",
          endpointName: "bikes",
          prompt: "Which stations are empty?",
          status: "interviewing",
          steps: 1,
          tokensUsed: 10,
          createdBy: "demo.steward",
          createdAt: "2026-09-25T08:00:00Z",
          endpoints: [{ name: "bikes", slug: "bikes-slug", space: "helsinki" }],
        });
      }
      if (url.pathname.endsWith("/endpoints")) {
        return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: ["bikes", "air"].map(endpoint) });
      }
      if (url.pathname.endsWith("/agent-runs")) return json({ items: [] });
      return json({});
    }),
  );
  vi.stubGlobal("EventSource", StubEventSource as unknown as typeof EventSource);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  const rootRoute = createRootRoute({ component: () => <AssistantDock project={PROJECT} /> });
  const router = createRouter({ routeTree: rootRoute });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <RouterProvider router={router} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return { started, messages };
}

const trigger = () => screen.getByRole("button", { name: new RegExp(`^${words.button}`) });
const preset = (title: string) => screen.getByRole("radio", { name: new RegExp(`^${title}`) });
const writes = (name: string) => screen.getByRole("switch", { name: words.endpoint.writes.replace("{name}", name) });
const path = (id: string) => document.querySelector<HTMLButtonElement>(`[data-path="${id}"]`)!;

async function openDock() {
  const person = userEvent.setup();
  await person.click(await screen.findByRole("button", { name: en.assistant.open }));
  await screen.findByLabelText(en.assistant.empty.composer);
  return person;
}

beforeEach(async () => {
  rememberRun(null);
  window.sessionStorage.clear();
  await i18n.changeLanguage("en");
});

afterEach(() => {
  vi.unstubAllGlobals();
  rememberRun(null);
});

describe("what travels with a request", () => {
  it("is nothing until a choice is made, and an endpoint the choice does not name reads", () => {
    expect(accessFor(null, ["bikes"])).toBeUndefined();
    expect(accessFor({ preset: "propose", endpoints: { bikes: "readWrite" } }, ["bikes", "air"])).toEqual({
      preset: "propose",
      endpoints: { bikes: "readWrite", air: "read" },
    });
  });

  it("keeps a choice per project and reads nothing it does not know from storage", () => {
    rememberCapabilities(PROJECT, { preset: "read", endpoints: { bikes: "readWrite" } });
    expect(storedCapabilities(PROJECT)).toEqual({ preset: "read", endpoints: { bikes: "readWrite" } });
    expect(storedCapabilities("tampere")).toBeNull();
    window.sessionStorage.setItem(`jc.assistant.capabilities.${PROJECT}`, '{"preset":"admin"}');
    expect(storedCapabilities(PROJECT)).toBeNull();
    window.sessionStorage.setItem(`jc.assistant.capabilities.${PROJECT}`, '{"preset":"build","endpoints":{"bikes":"all"}}');
    expect(storedCapabilities(PROJECT)).toEqual({ preset: "build", endpoints: {} });
    window.sessionStorage.setItem(`jc.assistant.capabilities.${PROJECT}`, '{"preset":"read","segment":"pipeline"}');
    expect(storedCapabilities(PROJECT)).toEqual({ preset: "read", endpoints: {}, segment: "pipeline" });
    window.sessionStorage.setItem(`jc.assistant.capabilities.${PROJECT}`, '{"preset":"read","segment":"root"}');
    expect(storedCapabilities(PROJECT)).toEqual({ preset: "read", endpoints: {} });
    window.sessionStorage.setItem(`jc.assistant.capabilities.${PROJECT}`, "{not json");
    expect(storedCapabilities(PROJECT)).toBeNull();
  });
});

describe("the Capabilities control (AG-92)", () => {
  it("says everything is allowed until a choice is made, and nothing chosen sends no access", async () => {
    const { started } = renderDock();
    const person = await openDock();
    expect(trigger()).toHaveTextContent(words.everything);
    expect(path("build-app")).not.toHaveAttribute("aria-disabled");
    await person.type(screen.getByLabelText(en.assistant.empty.composer), "Which stations are empty?{Enter}");
    await waitFor(() => expect(started).toHaveLength(1));
    expect(started[0]).not.toHaveProperty("access");
  });

  it("Read in two clicks: the paths it leaves out are disabled with why, and the start carries it", async () => {
    rememberEndpoints(PROJECT, ["bikes"]);
    const { started } = renderDock();
    const person = await openDock();
    await person.click(trigger());
    await person.click(preset(words.presets.read.title));
    expect(trigger()).toHaveTextContent(words.presets.read.title);

    expect(path("find-data")).not.toHaveAttribute("aria-disabled");
    for (const id of ["build-app", "share-data", "integrate-pipeline"]) {
      expect(path(id), id).toHaveAttribute("aria-disabled", "true");
      expect(path(id), id).toHaveAccessibleDescription(words.pathLeft.replace("{preset}", words.presets.read.title));
    }
    // Read changes nothing, so no endpoint may be written, whatever the grant says.
    expect(writes("bikes")).toBeDisabled();
    expect(writes("bikes")).toHaveAccessibleDescription(words.endpoint.readPreset);

    await person.click(path("build-app"));
    expect(started).toHaveLength(0);
    await person.type(screen.getByLabelText(en.assistant.empty.composer), "Which stations are empty?{Enter}");
    await waitFor(() => expect(started).toHaveLength(1));
    // The endpoint keeps its own choice for when Propose comes back; under Read the server
    // prepares no change on it whatever it says (`Capabilities::writes`).
    expect(started[0].access).toEqual({ preset: "read", endpoints: { bikes: "readWrite" } });
    expect(storedCapabilities(PROJECT)?.preset).toBe("read");
  });

  it("an endpoint the person's grant does not write stays read, with why; one it writes switches", async () => {
    rememberEndpoints(PROJECT, ["bikes", "air"]);
    const { started } = renderDock();
    const person = await openDock();
    await person.click(trigger());
    await person.click(preset(words.presets.propose.title));

    await waitFor(() => expect(writes("air")).toBeDisabled());
    expect(writes("air")).toHaveAccessibleDescription(words.endpoint.noWrite);
    await waitFor(() => expect(writes("bikes")).toBeEnabled());
    expect(writes("bikes")).toHaveAttribute("aria-checked", "true");
    await person.click(writes("bikes"));
    expect(writes("bikes")).toHaveAttribute("aria-checked", "false");
    await person.click(writes("bikes"));

    await person.type(screen.getByLabelText(en.assistant.empty.composer), "Fix the station names{Enter}");
    await waitFor(() => expect(started).toHaveLength(1));
    // air is sent as chosen: the choice only narrows, and the person's grant still refuses the
    // write at the endpoint's Policy (AG-70), which is what its switch said.
    expect(started[0].access).toEqual({ preset: "propose", endpoints: { bikes: "readWrite", air: "readWrite" } });
  });

  it("T-5958: a chosen segment runs the conversation as that connector, with its hint, and None takes it back", async () => {
    rememberEndpoints(PROJECT, ["bikes"]);
    const { started } = renderDock();
    const person = await openDock();
    await person.click(trigger());
    const tools = screen.getByRole("combobox", { name: words.segment.label });
    expect(tools).toHaveValue("");
    expect(tools).toHaveAccessibleDescription(words.segment.help);
    expect(within(tools).getAllByRole("option")).toHaveLength(10);
    await person.selectOptions(tools, "pipeline");
    expect(tools).toHaveAccessibleDescription(en.aiConnectors.segmentHint.pipeline);
    await person.selectOptions(tools, "");
    expect(storedCapabilities(PROJECT)).not.toHaveProperty("segment");
    await person.selectOptions(tools, "data-read");
    await person.click(preset(words.presets.read.title));
    expect(tools).toHaveValue("data-read");
    await person.type(screen.getByLabelText(en.assistant.empty.composer), "Which stations are empty?{Enter}");
    await waitFor(() => expect(started).toHaveLength(1));
    expect(started[0].access).toEqual({ preset: "read", endpoints: { bikes: "readWrite" }, segment: "data-read" });
  });

  it("opens and closes by keyboard, Escape hands focus back, and the open panel passes axe", async () => {
    rememberEndpoints(PROJECT, ["bikes"]);
    renderDock();
    const person = await openDock();
    trigger().focus();
    await person.keyboard("{Enter}");
    expect(trigger()).toHaveAttribute("aria-expanded", "true");
    const panel = document.getElementById(trigger().getAttribute("aria-controls")!)!;
    await waitFor(() => expect(within(panel).getByRole("switch")).toBeEnabled());
    await expectNoViolations(panel);
    await person.keyboard("{Escape}");
    expect(trigger()).toHaveAttribute("aria-expanded", "false");
    expect(trigger()).toHaveFocus();
  });

  it("in a running conversation a changed choice goes with the next message", async () => {
    rememberRun({ project: PROJECT, runId: RUN_ID });
    const { messages } = renderDock();
    const person = userEvent.setup();
    const box = await screen.findByLabelText(en.agentRun.conversation.placeholder);
    await act(async () => {
      await person.click(await screen.findByRole("button", { name: new RegExp(`^${words.button}`) }));
    });
    await person.click(preset(words.presets.propose.title));
    await person.type(box, "Draft a fix for the empty stations");
    await person.click(screen.getByRole("button", { name: en.agentRun.conversation.send }));
    await waitFor(() => expect(messages).toHaveLength(1));
    expect(messages[0]).toMatchObject({
      text: "Draft a fix for the empty stations",
      access: { preset: "propose", endpoints: { bikes: "readWrite" } },
    });
  });
});
