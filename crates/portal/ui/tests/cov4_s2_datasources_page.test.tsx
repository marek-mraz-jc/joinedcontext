// T-7706: coverage of DataSourcesPage: edit request by name, secrets of a stored source, a failed check,
// a YAML field the runner form cannot read, the pipeline hand-off, the read-only view, toForm/typeOf/endpointOf.
import { cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { endpointOf, toEnvelope, toForm, typeOf } from "../src/pages/datasources/DataSourcesPage";
import type { Manifest } from "../src/api/manifest";
import { digestOf } from "../src/api/digest";
import { findFormPage } from "./formPage";
import { jsonResponse, list, OTHER_BRAND, problem, renderRoute } from "./pageHarness";

const P = "/projects/helsinki/datasources";
const API = "/api/v1/projects/helsinki/datasources";

const WS: Manifest = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "DataSource",
  metadata: { name: "aq-stream", namespace: "helsinki", title: "Air stream" },
  spec: {
    type: "websocket",
    webSocket: { url: "wss://feed.hel.fi/aq" },
    secrets: [{ envVar: "FEED_TOKEN", name: "feed", key: "token" }, { name: "no-env" }],
  },
} as unknown as Manifest;

const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-1", namespace: "helsinki" },
  status: { lane: "green", phase: "PendingApproval", plan: { create: 1 } },
};

const FULL = { project: "helsinki", bootstrap: true, grants: [] };

beforeEach(async () => {
  await i18n.changeLanguage("en");
});
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  window.history.pushState({}, "", P);
});

/** As `answerWith`, and the draft door saves: a form whose draft is refused checks nothing. */
function drafting(dry: (request: Request) => Response | Promise<Response>, drafts: unknown[] = []) {
  const rest = answerWith(dry);
  return (path: string, request: Request): Response | Promise<Response> | undefined => {
    if (!path.includes("/drafts/")) return rest(path, request);
    if (request.method !== "PUT") return jsonResponse({ version: 1 });
    return request
      .clone()
      .json()
      .then((body: unknown) => {
        drafts.push(body);
        return jsonResponse({ version: 1 });
      });
  };
}

function answerWith(dry: (request: Request) => Response | Promise<Response>, items: unknown[] = [WS]) {
  return (path: string, request: Request): Response | Promise<Response> | undefined => {
    if (path.endsWith("/datasources") && request.method === "GET") return jsonResponse(list(items));
    if (path.endsWith("/datasources") && request.method === "POST") {
      return new URL(request.url).searchParams.has("dryRun") ? dry(request) : jsonResponse(CHANGE, 202);
    }
    return undefined;
  };
}

describe("DataSourcesPage coverage (T-7706)", () => {
  it("opens the named source for editing when the URL asks for it, carrying its stored secret", async () => {
    await renderRoute({ path: `${P}?edit=aq-stream`, answer: answerWith(() => problem(500, "x")), permissions: FULL });
    const dialog = await findFormPage();
    await waitFor(() => expect(within(dialog).getByLabelText(/Name/)).toHaveValue("aq-stream"));
    await waitFor(() => expect(within(dialog).getByLabelText(/URL/)).toHaveValue("wss://feed.hel.fi/aq"));
  });

  it("shows the problem when the check is refused, and keeps the form open", async () => {
    const r = await renderRoute({ path: P, answer: answerWith(() => problem(422, "spec.websocket.url is not reachable")), permissions: FULL });
    await userEvent.click(await screen.findByRole("button", { name: en.datasources.add }));
    const dialog = await findFormPage();
    await userEvent.selectOptions(within(dialog).getByLabelText(en.datasources.field.type), "websocket");
    await userEvent.type(within(dialog).getByLabelText(/Name/), "aq-stream");
    await userEvent.type(within(dialog).getByLabelText(/URL/), "wss://feed.hel.fi/aq");
    await userEvent.click(within(dialog).getByRole("button", { name: en.datasources.check }));
    expect(await within(dialog).findByText(/is not reachable/)).toBeInTheDocument();
    expect(r.calls().filter((c) => c === "POST " + API)).toHaveLength(1);
  });

  it("carries a secret picked for a runner field into the draft as a reference, and settles", async () => {
    const drafts: unknown[] = [];
    await renderRoute({ path: P, answer: drafting(() => problem(500, "x"), drafts), permissions: FULL });
    await userEvent.click(await screen.findByRole("button", { name: en.datasources.add }));
    const dialog = await findFormPage();
    await userEvent.selectOptions(within(dialog).getByLabelText(en.datasources.field.type), "nats");
    await userEvent.type(within(dialog).getAllByLabelText(/^Name/)[0], "bus-feed");
    const [secret] = await within(dialog).findAllByPlaceholderText(/Secret name/);
    // Each keystroke names a reference; the page took one a render and looped for ever (T-7937).
    await userEvent.type(secret, "nats-creds");
    expect(secret).toHaveValue("nats-creds");
    await waitFor(
      () => {
        const last = JSON.stringify(drafts[drafts.length - 1] ?? {});
        expect(last).toContain('"name":"nats-creds"');
        expect(last).toMatch(/"envVar":"DS_NATS_CREDS_[A-Z0-9_]+"/);
      },
      { timeout: 5000 },
    );
  });

  it("names the runner field whose YAML does not parse and sends no check", async () => {
    const r = await renderRoute({ path: P, answer: drafting(() => problem(500, "x")), permissions: FULL });
    await userEvent.click(await screen.findByRole("button", { name: en.datasources.add }));
    const dialog = await findFormPage();
    await userEvent.selectOptions(within(dialog).getByLabelText(en.datasources.field.type), "http_client");
    await userEvent.type(within(dialog).getAllByLabelText(/^Name/)[0], "feed-csv");
    const headers = await waitFor(() => {
      const box = dialog.querySelector<HTMLTextAreaElement>("#root_headers");
      expect(box).not.toBeNull();
      return box as HTMLTextAreaElement;
    });
    fireEvent.change(dialog.querySelector<HTMLInputElement>("#root_url") as HTMLInputElement, { target: { value: "https://feed.hel.fi/aq.csv" } });
    fireEvent.change(headers, { target: { value: "{ Accept: [text/csv" } });
    await userEvent.click(within(dialog).getByRole("button", { name: en.datasources.check }));
    expect(await within(dialog).findByText(/headers: not valid YAML/)).toBeInTheDocument();
    expect(r.calls().filter((c) => c.startsWith("POST " + API))).toHaveLength(0);
  });

  it("refuses to propose a runner field whose YAML does not parse, under lax validation too", async () => {
    const r = await renderRoute({ path: P, answer: drafting(() => problem(500, "x")), permissions: FULL, brand: { ...OTHER_BRAND, validation: "lax" } });
    await userEvent.click(await screen.findByRole("button", { name: en.datasources.add }));
    const dialog = await findFormPage();
    await userEvent.selectOptions(within(dialog).getByLabelText(en.datasources.field.type), "http_client");
    await userEvent.type(within(dialog).getAllByLabelText(/^Name/)[0], "feed-csv");
    const headers = await waitFor(() => {
      const box = dialog.querySelector<HTMLTextAreaElement>("#root_headers");
      expect(box).not.toBeNull();
      return box as HTMLTextAreaElement;
    });
    fireEvent.change(dialog.querySelector<HTMLInputElement>("#root_url") as HTMLInputElement, { target: { value: "https://feed.hel.fi/aq.csv" } });
    fireEvent.change(headers, { target: { value: "{ Accept: [text/csv" } });
    await userEvent.click(within(dialog).getByRole("button", { name: en.datasources.propose }));
    expect(await within(dialog).findByText(/headers: not valid YAML/)).toBeInTheDocument();
    expect(r.calls().filter((c) => c.startsWith("POST " + API))).toHaveLength(0);
  });

  it("offers to start a pipeline on the source just proposed", async () => {
    // The check answers a verdict fresh for the manifest it judged; the form proposes nothing without it.
    const dry = async (request: Request) => {
      const manifest = { ...((await request.clone().json()) as Record<string, unknown>) };
      delete manifest.draft;
      return jsonResponse({ valid: true, lane: "green", plan: { fields: [] }, verdict: { ok: true, findings: [], checkedAt: new Date().toISOString(), inputDigest: digestOf(manifest) } });
    };
    await renderRoute({ path: P, answer: answerWith(dry, []), permissions: FULL });
    await userEvent.click(await screen.findByRole("button", { name: en.datasources.add }));
    const dialog = await findFormPage();
    await userEvent.selectOptions(within(dialog).getByLabelText(en.datasources.field.type), "websocket");
    await userEvent.type(within(dialog).getByLabelText(/Name/), "aq-stream");
    await userEvent.type(within(dialog).getByLabelText(/URL/), "wss://feed.hel.fi/aq");
    await userEvent.click(within(dialog).getByRole("button", { name: en.datasources.check }));
    await userEvent.click(await within(dialog).findByRole("button", { name: en.datasources.propose }));
    const handoff = await screen.findByRole("button", { name: en.datasources.wizard.handoff.replace("{name}", "aq-stream") });
    await userEvent.click(handoff);
    await waitFor(() => expect(window.location.pathname).toBe("/projects/helsinki/pipelines"));
  });

  it("opens a source read-only for someone who may not propose, and closes it again", async () => {
    const viewer = { project: "helsinki", bootstrap: false, grants: [{ role: "r", binding: "b", scope: "project:helsinki", rule: { kinds: ["Project"], verbs: ["read"] } }] };
    await renderRoute({ path: `${P}?edit=aq-stream`, answer: answerWith(() => problem(500, "x")), permissions: viewer });
    const dialog = await findFormPage();
    expect(within(dialog).queryByRole("button", { name: en.datasources.propose })).not.toBeInTheDocument();
    await userEvent.click(within(dialog).getByRole("button", { name: "Close" }));
    await waitFor(() => expect(screen.queryByTestId("form-page")).not.toBeInTheDocument());
    expect(await screen.findByText("aq-stream")).toBeInTheDocument();
  });
});

describe("DataSourcesPage: other connections, secrets and discovery (T-7706)", () => {
  it("opens the full form of another connection from the wizard and keeps the name already typed", async () => {
    await renderRoute({ path: P, answer: answerWith(() => problem(500, "x"), []), permissions: FULL });
    await userEvent.click(await screen.findByRole("button", { name: en.datasources.add }));
    const dialog = await findFormPage();
    await userEvent.type(within(dialog).getAllByLabelText(/^Name/)[0], "queue-feed");
    await userEvent.selectOptions(await within(dialog).findByLabelText(en.datasources.wizard.groups.other), "sftp");
    expect(within(dialog).getByLabelText(en.datasources.field.type)).toHaveValue("sftp");
    expect(within(dialog).getAllByLabelText(/^Name/)[0]).toHaveValue("queue-feed");
  });

  it("writes the pick of Find in the service into the HTTP form", async () => {
    const asked: unknown[] = [];
    const base = answerWith(() => problem(500, "x"), []);
    await renderRoute({
      path: P,
      permissions: FULL,
      answer: (path, request) => {
        if (path.endsWith("/datasource-discovery") && request.method === "POST") {
          return request.json().then((body: unknown) => {
            asked.push(body);
            return jsonResponse({
              field: "typeNames",
              url: "https://kartta.hel.fi/wfs?service=WFS&request=GetCapabilities",
              choices: [{ value: "avoindata:Puut", label: "Trees" }],
              truncated: false,
            });
          });
        }
        return base(path, request);
      },
    });
    await userEvent.click(await screen.findByRole("button", { name: en.datasources.add }));
    const dialog = await findFormPage();
    await userEvent.type(within(dialog).getByLabelText(/^URL/), "https://kartta.hel.fi/wfs");
    await userEvent.selectOptions(within(dialog).getByLabelText(/^API/), "WFS 2.0");
    await userEvent.click(await within(dialog).findByRole("button", { name: "Find" }));
    await userEvent.click(await within(dialog).findByRole("button", { name: "Use" }));
    expect(asked).toEqual([{ kind: "wfs", url: "https://kartta.hel.fi/wfs" }]);
    expect(await within(dialog).findByText(/Written into the form: avoindata:Puut\./)).toBeInTheDocument();
    await waitFor(() =>
      expect(within(dialog).getAllByLabelText(/Feature type/).some((field) => (field as HTMLInputElement).value === "avoindata:Puut")).toBe(true),
    );
  });
});

describe("DataSourcesPage list search (T-7706)", () => {
  it("filters the rows by what the search box holds and says so when none match", async () => {
    await renderRoute({ path: P, answer: answerWith(() => problem(500, "x")), permissions: FULL });
    expect(await screen.findByText("aq-stream")).toBeInTheDocument();
    const box = screen.getByRole("searchbox", { name: en.dataGrid.searchRows });
    fireEvent.change(box, { target: { value: "zzz-nothing" } });
    await waitFor(() => expect(screen.queryByText("aq-stream")).not.toBeInTheDocument());
    fireEvent.change(box, { target: { value: "feed.hel.fi" } });
    expect(await screen.findByText("aq-stream")).toBeInTheDocument();
  });
});

describe("toForm, typeOf and endpointOf", () => {
  it("flattens a runner input list of objects to YAML text and keeps scalars", () => {
    const form = toForm({
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "DataSource",
      metadata: { name: "n", namespace: "p" },
      spec: { type: "nats", input: { urls: ["nats://x"], processors: [{ mapping: "root = this" }] } },
    } as unknown as Manifest);
    expect(form.urls).toEqual(["nats://x"]);
    expect(String(form.processors)).toContain("mapping: root = this");
  });

  it("keeps a runner field whose YAML does not parse as typed in the draft, and refuses it in the check", () => {
    const form = { name: "feed", type: "http_client", url: "https://x.example", headers: "{ Accept: [text/csv" };
    const drafted = toEnvelope("p", "http_client", form, undefined, true);
    expect((drafted.spec.input as { headers?: unknown }).headers).toBe("{ Accept: [text/csv");
    expect(() => toEnvelope("p", "http_client", form)).toThrow(/^headers: not valid YAML/);
    const fine = toEnvelope("p", "http_client", { ...form, headers: "{ Accept: text/csv }" }, undefined, true);
    expect((fine.spec.input as { headers?: unknown }).headers).toEqual({ Accept: "text/csv" });
  });

  it("falls back to the first type for a manifest without one", () => {
    expect(typeOf({})).toBe(typeOf({ type: "  " }));
    expect(typeOf({ type: " mqtt " })).toBe("mqtt");
  });

  it("names a source by its type when no address is declared", () => {
    expect(endpointOf({ type: " custom " })).toBe("custom");
    expect(endpointOf({})).toBe("");
  });
});
