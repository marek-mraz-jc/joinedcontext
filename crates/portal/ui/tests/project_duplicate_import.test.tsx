/**
 * Duplicating and importing a project (PF-89, MF-45, MF-46, CC-88, T-2644): Duplicate sits on the
 * project's General settings and is refused, with the reason, for a project without a repository
 * of its own; Import checks the archive first and draws the parameter form from what the project
 * declares, then sends the values through the same check before the import.
 */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { App } from "../src/App";
import { DuplicateProjectAction } from "../src/components/DuplicateProjectDialog";
import { numberProblems, parameterValues } from "../src/components/ImportProjectDialog";
import { expectDenied, expectOpen } from "./checks";

const IDENTITY = { subject: "b7c1e0f4", username: "jana.kovacova", name: "Jana Kováčová", roles: [] };
const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-0000002b", namespace: "doprava" },
  status: { lane: "yellow", phase: "PendingApproval", plan: { create: 1, update: 0, delete: 0 } },
};

interface Sent {
  path: string;
  search: string;
  body: string;
  form: FormData | null;
}

function renderAt(where: string, spec: Record<string, unknown>, importAnswer?: (dryRun: boolean) => Response) {
  const sent: Sent[] = [];
  const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const request = typeof input === "string" || input instanceof URL ? null : input;
    const url = new URL(request ? request.url : String(input), window.location.origin);
    const method = request?.method ?? init?.method ?? "GET";
    const json = (body: unknown, status = 200) =>
      new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
    if (url.pathname.endsWith("/auth/me")) {
      return json(IDENTITY);
    }
    if (url.pathname.endsWith("/permissions/me")) {
      // An organization administrator, whose page the import lives on (UI-87).
      return json({
        project: "banskabystrica",
        bootstrap: false,
        grants: [{ role: "org-admin", binding: "admins", rule: { kinds: ["RoleBinding"], verbs: ["approve", "delete"] } }],
        projects: { creation: { allowed: true } },
      });
    }
    if (method === "GET" && url.pathname === "/api/v1/projects") {
      return json(list([{ name: "banskabystrica" }]));
    }
    if (method === "GET" && url.pathname === "/api/v1/projects/org/projects/banskabystrica") {
      return json({
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "Project",
        metadata: { name: "banskabystrica", title: "Banská Bystrica" },
        spec,
      });
    }
    if (method === "GET" && url.pathname === "/api/v1/projects/banskabystrica/registry") {
      return json({ repository: { name: "banskabystrica" }, ref: "main", parameters: {}, declarations: {}, tags: [] });
    }
    if (method === "POST") {
      // The typed client sends a Request; a multipart one is read back as its form (UI-07).
      const multipart = request?.headers.get("content-type")?.startsWith("multipart/form-data") ?? false;
      const form =
        init?.body instanceof FormData ? init.body : multipart && request ? await request.clone().formData() : null;
      sent.push({
        path: url.pathname,
        search: url.search,
        body: form ? "" : request ? await request.clone().text() : String(init?.body ?? ""),
        form,
      });
      if (url.pathname.endsWith("/duplicate")) {
        return json(CHANGE, 202);
      }
      if (url.pathname.endsWith("/import") && importAnswer) {
        return importAnswer(url.searchParams.has("dryRun"));
      }
    }
    return json(list([]));
  });
  vi.stubGlobal("fetch", fetchMock);
  window.history.pushState({}, "", where);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <App />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return { sent };
}

const PLAN = {
  repositories: [{ name: "doprava", role: "project", repository: "org/doprava", head: "8c56954a1f0e2b3c" }],
  parameters: {
    city: { type: "string", description: "The city's name." },
    retries: { type: "integer", default: 3 },
    live: { type: "boolean" },
    token: { type: "secret" },
  },
};

describe("Duplicate on the project's settings", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it("is refused, with the reason, for a project without a repository of its own", async () => {
    renderAt("/projects/banskabystrica/settings/general", {});
    const button = await screen.findByRole("button", { name: "Duplicate" });
    expectDenied(button, "Only a project in a repository of its own can be duplicated.");
  });

  it("opens nothing while refused", async () => {
    const user = userEvent.setup();
    render(
      <QueryClientProvider client={new QueryClient()}>
        <I18nextProvider i18n={i18n}>
          <DuplicateProjectAction project="banskabystrica" inOwnRepository={false} />
        </I18nextProvider>
      </QueryClientProvider>,
    );
    await user.click(screen.getByRole("button", { name: "Duplicate" }));
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("refuses the original's name and proposes the copy as one request", async () => {
    const { sent } = renderAt("/projects/banskabystrica/settings/general", { repository: "org/banskabystrica" });
    const user = userEvent.setup();
    const button = await screen.findByRole("button", { name: "Duplicate" });
    await waitFor(() => expectOpen(button));
    await user.click(button);
    const dialog = await screen.findByRole("dialog", { name: /Duplicate banskabystrica/ });
    const name = within(dialog).getByLabelText(/Name/);
    await user.type(name, "banskabystrica");
    expect(within(dialog).getByText("Choose a name other than the original's.")).toBeInTheDocument();
    expectDenied(within(dialog).getByRole("button", { name: "Propose the copy" }), "A usable name is needed to open a project.");

    await user.clear(name);
    await user.type(name, "doprava");
    await user.click(within(dialog).getByRole("button", { name: "Propose the copy" }));
    await waitFor(() => expect(sent).toHaveLength(1));
    expect(sent[0].path).toBe("/api/v1/projects/banskabystrica/duplicate");
    expect(JSON.parse(sent[0].body)).toEqual({ name: "doprava", parameters: {} });
    expect(await within(dialog).findByText(/chg-0000002b/)).toBeInTheDocument();
  });
});

describe("Import project from Administration → Projects (UI-87)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it("checks the archive, draws the declared parameters and sends only the values typed", async () => {
    const { sent } = renderAt("/organization/projects", {}, (dryRun) =>
      dryRun
        ? new Response(JSON.stringify(PLAN), { status: 200, headers: { "Content-Type": "application/json" } })
        : new Response(JSON.stringify(CHANGE), { status: 202, headers: { "Content-Type": "application/json" } }),
    );
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: "Import project" }));
    const dialog = await screen.findByRole("dialog", { name: /Import a project/ });
    expectDenied(within(dialog).getByRole("button", { name: "Check the archive" }), "Choose an export archive first.");

    const archive = new File([new Uint8Array([80, 75, 3, 4])], "doprava.zip", { type: "application/zip" });
    await user.upload(within(dialog).getByLabelText("Choose the export archive"), archive);
    await user.type(within(dialog).getByLabelText(/Name/), "doprava");
    await user.click(within(dialog).getByRole("button", { name: "Check the archive" }));

    expect(await within(dialog).findByText("org/doprava at 8c56954")).toBeInTheDocument();
    expect(within(dialog).getByText(/Type the name of a secret, never its value/)).toBeInTheDocument();
    expect(within(dialog).getByText(/Default: 3/)).toBeInTheDocument();
    expect(sent[0].search).toBe("?format=git&dryRun=All");
    expect(sent[0].form?.get("parameters")).toBe("{}");

    await user.type(within(dialog).getByLabelText("city"), "Doprava");
    await user.type(within(dialog).getByLabelText("retries"), "5");
    await user.selectOptions(within(dialog).getByLabelText("live"), "false");
    await user.click(within(dialog).getByRole("button", { name: "Import" }));

    expect(await within(dialog).findByText(/chg-0000002b/)).toBeInTheDocument();
    // The values pass the dry run again before the import itself (PF-57).
    expect(sent.map((one) => one.search)).toEqual(["?format=git&dryRun=All", "?format=git&dryRun=All", "?format=git"]);
    expect(JSON.parse(String(sent[2].form?.get("parameters")))).toEqual({ city: "Doprava", retries: 5, live: false });
    // Read back from the Request, the part is undici's File, not jsdom's: its content is the check.
    const sentArchive = sent[2].form?.get("file");
    expect(typeof sentArchive === "object" && sentArchive !== null ? sentArchive.type : sentArchive).toBe("application/zip");
  });

  it("shows what the import does with each organization model the archive carries (MF-50)", async () => {
    const models = [
      { name: "stations", version: "1.3.0", action: "map" },
      { name: "bikes", version: "2.0.1", action: "land" },
    ];
    renderAt("/organization/projects", {}, () =>
      new Response(JSON.stringify({ ...PLAN, models }), { status: 200, headers: { "Content-Type": "application/json" } }),
    );
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: "Import project" }));
    const dialog = await screen.findByRole("dialog", { name: /Import a project/ });
    await user.upload(
      within(dialog).getByLabelText("Choose the export archive"),
      new File([new Uint8Array([80, 75, 3, 4])], "doprava.zip", { type: "application/zip" }),
    );
    await user.type(within(dialog).getByLabelText(/Name/), "doprava");
    await user.click(within(dialog).getByRole("button", { name: "Check the archive" }));

    expect(await within(dialog).findByRole("heading", { name: "Organization models" })).toBeInTheDocument();
    expect(within(dialog).getByText(/stations 1\.3\.0: this organization holds it byte for byte/)).toBeInTheDocument();
    expect(within(dialog).getByText(/bikes 2\.0\.1: lands as a model of the project/)).toBeInTheDocument();
  });

  it("draws no model list for an archive that carries none", async () => {
    renderAt("/organization/projects", {}, () =>
      new Response(JSON.stringify({ ...PLAN, models: [] }), { status: 200, headers: { "Content-Type": "application/json" } }),
    );
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: "Import project" }));
    const dialog = await screen.findByRole("dialog", { name: /Import a project/ });
    await user.upload(within(dialog).getByLabelText("Choose the export archive"), new File(["x"], "a.zip"));
    await user.type(within(dialog).getByLabelText(/Name/), "doprava");
    await user.click(within(dialog).getByRole("button", { name: "Check the archive" }));
    expect(await within(dialog).findByText("org/doprava at 8c56954")).toBeInTheDocument();
    expect(within(dialog).queryByRole("heading", { name: "Organization models" })).toBeNull();
  });

  it("shows the API's refusal of the archive and draws no form", async () => {
    renderAt(
      "/organization/projects",
      {},
      () =>
        new Response(JSON.stringify({ title: "Bad Request", detail: "doprava.bundle does not match its sha256" }), {
          status: 400,
          headers: { "Content-Type": "application/problem+json" },
        }),
    );
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: "Import project" }));
    const dialog = await screen.findByRole("dialog", { name: /Import a project/ });
    await user.upload(within(dialog).getByLabelText("Choose the export archive"), new File(["x"], "bad.zip"));
    await user.type(within(dialog).getByLabelText(/Name/), "doprava");
    await user.click(within(dialog).getByRole("button", { name: "Check the archive" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("doprava.bundle does not match its sha256");
    expect(within(dialog).queryByRole("button", { name: "Import" })).toBeNull();
  });
});

describe("parameterValues", () => {
  it("leaves an empty field to the default and types numbers and booleans", () => {
    expect(
      parameterValues(
        { a: { type: "string" }, n: { type: "number" }, b: { type: "boolean" }, s: { type: "secret" } },
        { a: "  ", n: "2.5", b: "true", s: "db-password" },
      ),
    ).toEqual({ n: 2.5, b: true, s: "db-password" });
    expect(parameterValues({}, { stray: "x" })).toEqual({});
  });

  it("names the number parameters whose value is no number, which would go out as null (T-6230)", () => {
    const declarations = { n: { type: "number" }, i: { type: "integer" }, a: { type: "string" } } as const;
    expect(numberProblems(declarations, { n: "abc", i: "2.5", a: "abc" })).toEqual(["n", "i"]);
    expect(numberProblems(declarations, { n: "-2.5", i: "30", a: "" })).toEqual([]);
    expect(numberProblems(declarations, { n: "", i: " " })).toEqual([]);
  });
});
