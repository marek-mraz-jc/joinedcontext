/** Work on a copy: what each scope names, the default name it proposes, and the title and lifetime sent. */
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";

vi.mock("@tanstack/react-router", () => ({
  Link: ({ children }: { children: ReactNode }) => <a href="/">{children}</a>,
  useNavigate: () => vi.fn(),
  useRouterState: ({ select }: { select: (s: { location: { search: unknown } }) => unknown }) =>
    select({ location: { search: {} } }),
}));
vi.mock("../src/auth/AuthProvider", () => ({
  useAuthStatus: () => "authenticated",
  useAuth: () => ({ identity: { email: "jana@hel.fi", username: "jana" }, status: "authenticated" }),
}));

const { WorkOnCopyDialog } = await import("../src/components/WorkOnCopyDialog");
type Scope = Parameters<typeof WorkOnCopyDialog>[0]["scope"];

let posted: { path: string; body: unknown }[];

function show(scope: Scope) {
  return render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>
        <WorkOnCopyDialog project="helsinki" scope={scope} open onOpenChange={() => {}} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

describe("work on a copy, by scope", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    posted = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
        const request = input instanceof Request ? input : new Request(new URL(String(input), "http://localhost"), init);
        posted.push({ path: new URL(request.url).pathname, body: await request.clone().json().catch(() => undefined) });
        return new Response(JSON.stringify({ name: "x", project: "helsinki" }), { status: 201, headers: { "Content-Type": "application/json" } });
      }),
    );
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("a space names itself and is sent with the title and lifetime typed", async () => {
    show({ kind: "space", name: "Air Quality" });
    expect(screen.getByLabelText(/^Name/)).toHaveValue("air-quality");
    expect(screen.getByText(en.workspaces.open.scopeSpace.replace("{name}", "Air Quality"))).toBeInTheDocument();
    await userEvent.type(screen.getByLabelText(en.workspaces.open.titleField), "  Spring trial ");
    await userEvent.selectOptions(screen.getByLabelText(en.workspaces.open.ttl), "14");
    await userEvent.click(screen.getByRole("button", { name: en.workspaces.open.submit }));
    await waitFor(() => expect(posted).toHaveLength(1));
    expect(posted[0]?.body).toMatchObject({
      name: "air-quality",
      title: "Spring trial",
      ttlDays: 14,
      scope: { kind: "space", name: "Air Quality" },
    });
  });

  it("one resource names the copy after itself and lists itself", async () => {
    show({ kind: "resources", items: [{ kind: "Endpoint", name: "Air.Quality" }] });
    expect(screen.getByLabelText(/^Name/)).toHaveValue("air-quality");
    expect(screen.getByText("Endpoint/Air.Quality")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: en.workspaces.open.submit }));
    await waitFor(() => expect(posted).toHaveLength(1));
    expect(posted[0]?.body).toMatchObject({ scope: { kind: "resources", items: [{ kind: "Endpoint", name: "Air.Quality" }] }, ttlDays: 7 });
  });

  it("several resources are listed together under the name 'copy'", () => {
    show({ kind: "resources", items: [{ kind: "Endpoint", name: "a" }, { kind: "Pipeline", name: "b" }] });
    expect(screen.getByLabelText(/^Name/)).toHaveValue("copy");
    expect(screen.getByText("Endpoint/a, Pipeline/b")).toBeInTheDocument();
  });

  it("a resources scope with no items says nothing and still proposes 'copy'", () => {
    show({ kind: "resources" });
    expect(screen.getByLabelText(/^Name/)).toHaveValue("copy");
  });

  it("the whole project is named as such", () => {
    show({ kind: "project" });
    expect(screen.getByText(en.workspaces.open.scopeProject)).toBeInTheDocument();
  });
});
