/**
 * Release on the project's General settings (PF-86, CC-88, T-3432): what the registry entry runs
 * and its values, and "Pin a release", which offers the repository's tags and the declared
 * parameters and proposes one `PUT …/registry`. A person who may not propose is told why.
 */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { App } from "../src/App";
import { ProjectRelease } from "../src/components/ProjectRelease";
import { expectDenied, expectNoViolations, expectOpen } from "./checks";

const IDENTITY = { subject: "b7c1e0f4", username: "jana.kovacova", name: "Jana Kováčová", roles: [] };
const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-00000029", namespace: "ovzdusie" },
  status: { lane: "red", phase: "PendingApproval", plan: { create: 0, update: 1, delete: 0 } },
};
const ENTRY = {
  repository: { name: "ovzdusie" },
  ref: "main",
  parameters: { audience: "public" },
  declarations: {
    audience: { type: "string", default: "public", enum: ["public", "organization"] },
    city: { type: "string", description: "The city's name." },
  },
  tags: [
    { name: "v0.2.0", commit: "c0ffee2" },
    { name: "v0.1.0", commit: "c0ffee1" },
  ],
};

function renderAt(verbs: string[], answer: (body: unknown) => Response) {
  const sent: { method: string; path: string; body: unknown }[] = [];
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
      return json({
        project: "ovzdusie",
        bootstrap: false,
        grants: [{ role: "org-admin", binding: "jana", rule: { kinds: ["*", "Project"], verbs } }],
      });
    }
    if (method === "GET" && url.pathname === "/api/v1/projects") {
      return json(list([{ name: "ovzdusie" }]));
    }
    if (method === "GET" && url.pathname === "/api/v1/projects/org/projects/ovzdusie") {
      return json({
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "Project",
        metadata: { name: "ovzdusie", title: "Ovzdušie" },
        spec: { organizationRef: "bb", repository: { name: "ovzdusie" }, ref: "main" },
      });
    }
    if (method === "GET" && url.pathname === "/api/v1/projects/ovzdusie/registry") {
      return json(ENTRY);
    }
    if (method === "PUT" && url.pathname === "/api/v1/projects/ovzdusie/registry") {
      const body: unknown = JSON.parse(request ? await request.clone().text() : String(init?.body ?? "{}"));
      sent.push({ method, path: url.pathname, body });
      return answer(body);
    }
    return json(list([]));
  });
  vi.stubGlobal("fetch", fetchMock);
  window.history.pushState({}, "", "/projects/ovzdusie/settings/general");
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

const accepted = () =>
  new Response(JSON.stringify(CHANGE), { status: 202, headers: { "Content-Type": "application/json" } });

describe("Release on the project's settings", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it("shows what runs and pins a tag with the values as one proposal", async () => {
    const { sent } = renderAt(["read", "propose"], accepted);
    const user = userEvent.setup();
    const section = (await screen.findByRole("heading", { level: 2, name: "Release" })).closest("section");
    if (!section) throw new Error("the Release section");
    expect(await within(section).findByText("main")).toBeInTheDocument();
    expect(within(section).getByText("audience")).toBeInTheDocument();

    const button = within(section).getByRole("button", { name: "Pin a release" });
    await waitFor(() => expectOpen(button));
    await user.click(button);
    const dialog = await screen.findByRole("dialog", { name: /Pin a release of ovzdusie/ });
    await expectNoViolations(dialog);
    const release = within(dialog).getByLabelText(/^Release/);
    expect(within(release).getAllByRole("option").map((option) => option.textContent)).toEqual([
      "main (runs now)",
      "v0.2.0",
      "v0.1.0",
    ]);
    await user.selectOptions(release, "v0.2.0");
    expect(within(dialog).getByLabelText("audience")).toHaveValue("public");
    await user.type(within(dialog).getByLabelText("city"), "Zvolen");
    await user.click(within(dialog).getByRole("button", { name: "Propose" }));

    expect(await within(dialog).findByText(/chg-00000029/)).toBeInTheDocument();
    expect(sent).toEqual([
      {
        method: "PUT",
        path: "/api/v1/projects/ovzdusie/registry",
        body: { ref: "v0.2.0", parameters: { audience: "public", city: "Zvolen" } },
      },
    ]);
  });

  it("says what the server refused, in the dialog", async () => {
    renderAt(["read", "propose"], () =>
      new Response(
        JSON.stringify({ title: "Bad Request", status: 400, detail: "unknown parameter 'city' (CC-88)" }),
        { status: 400, headers: { "Content-Type": "application/problem+json" } },
      ),
    );
    const user = userEvent.setup();
    const button = await screen.findByRole("button", { name: "Pin a release" });
    await waitFor(() => expectOpen(button));
    await user.click(button);
    const dialog = await screen.findByRole("dialog", { name: /Pin a release of ovzdusie/ });
    await user.click(within(dialog).getByRole("button", { name: "Propose" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("unknown parameter 'city'");
  });

  it("refuses Pin a release with the reason to a person who may only read", async () => {
    renderAt(["read"], accepted);
    const user = userEvent.setup();
    const button = await screen.findByRole("button", { name: "Pin a release" });
    expectDenied(button, "Only someone who may change this project at the organization, such as an organization administrator, can pin a release.");
    await user.click(button);
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("refuses Pin a release with the reason for a project authored outside the forge", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL) => {
        const url = new URL(typeof input === "string" || input instanceof URL ? String(input) : input.url, window.location.origin);
        const body = url.pathname.endsWith("/registry")
          ? { repository: { url: "https://git.example/ovzdusie.git" }, ref: "v1.0.0", parameters: {}, declarations: {}, tags: [] }
          : { project: "org", bootstrap: false, grants: [{ role: "org-admin", binding: "jana", rule: { kinds: ["Project"], verbs: ["propose"] } }] };
        return new Response(JSON.stringify(body), { status: 200, headers: { "Content-Type": "application/json" } });
      }),
    );
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <I18nextProvider i18n={i18n}>
          <ProjectRelease project="ovzdusie" />
        </I18nextProvider>
      </QueryClientProvider>,
    );
    expect(await screen.findByText("v1.0.0")).toBeInTheDocument();
    expect(screen.getByText("Every parameter at its default.")).toBeInTheDocument();
    expectDenied(
      screen.getByRole("button", { name: "Pin a release" }),
      "This project is authored outside the forge, so the Portal lists no releases for it.",
    );
  });
});
