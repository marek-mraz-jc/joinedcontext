// covers (T-2137, the module gate in gate_modules.test.ts): src/pages/catalogue/ReadableDatasets.tsx,
// src/pages/catalogue/EndpointDatasetPage.tsx, src/components/layout/MemberFrame.tsx.
// ADR-N-067 §2.4 (T-6411): the signed-in catalogue lists what the server says the person may
// read, each row marked by what grants it; a dataset page by its Endpoint shows "sign in" to a
// visitor of a non-public Endpoint, "no access" to a person without a grant, rows to a person
// with one; a member gets the member shell.
import type { ReactNode } from "react";
import { screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n, { SUPPORTED_LOCALES } from "../src/i18n";
import en from "../src/locales/en.json";
import { expectNoRawKeys, expectNoViolations } from "./checks";
import { json, renderPart } from "./page_contract";

let status: "authenticated" | "anonymous" = "authenticated";
const signOut = vi.fn(() => Promise.resolve());
vi.mock("../src/auth/AuthProvider", () => ({
  useAuthStatus: () => status,
  useAuth: () => ({ status, identity: { username: "demo.analyst", name: "Demo Analyst" }, signOut }),
  useSignIn: () => () => undefined,
}));
vi.mock("@tanstack/react-router", () => ({
  Link: ({ children, to, params, activeProps: _active, ...rest }: { children: ReactNode; to: string; params?: Record<string, string>; activeProps?: unknown }) => (
    <a href={Object.entries(params ?? {}).reduce((path, [k, v]) => path.replace(`$${k}`, v), to)} {...rest}>
      {children}
    </a>
  ),
}));

const { ReadableDatasets } = await import("../src/pages/catalogue/ReadableDatasets");
const { EndpointDatasetPage } = await import("../src/pages/catalogue/EndpointDatasetPage");
const { MemberFrame } = await import("../src/components/layout/MemberFrame");

const row = (name: string, access: string, group?: string) => ({
  project: "helsinki",
  name,
  slug: `${name}zzzzzzzzzzzzzzzzzzzzzz`,
  title: name.replace(/^./, (c) => c.toUpperCase()),
  representations: ["ngsi-ld"],
  access,
  ...(group ? { group } : {}),
});

const ANALYST = [row("bikes", "public"), row("counts", "organization"), row("incidents", "group", "traffic-analysts")];

function readable(datasets: unknown[], unanswered = 0) {
  return (url: URL) => (url.pathname === "/api/v1/catalogue/readable" ? json({ datasets, unanswered }) : undefined);
}

const refusal = (code: number) =>
  new Response(JSON.stringify({ title: code === 401 ? "Unauthorized" : "Forbidden", status: code, detail: "no grant on this endpoint" }), {
    status: code,
    headers: { "Content-Type": "application/problem+json" },
  });

afterEach(() => {
  vi.unstubAllGlobals();
  status = "authenticated";
});

describe("the datasets a signed-in person may read (T-6411)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("lists each internal dataset with what grants it and leaves the public ones to the search below", async () => {
    const { container } = renderPart(<ReadableDatasets />, { answer: readable(ANALYST) });
    const counts = await screen.findByRole("link", { name: `${en.catalogue.readable.open}: Counts` });
    expect(counts).toHaveAttribute("href", "/catalogue/endpoint/countszzzzzzzzzzzzzzzzzzzzzz");
    expect(screen.getByText(en.catalogue.readable.access.organization)).toBeInTheDocument();
    expect(screen.getByText("Group traffic-analysts")).toBeInTheDocument();
    // The public one is listed by the open-data search below, not twice.
    expect(screen.queryByRole("link", { name: /Bikes/ })).toBeNull();
    // No count of public rows beside a CKAN list that may show another number (UI judge, T-6411).
    expect(screen.queryByText(/Public datasets/)).toBeNull();
    await expectNoViolations(container);
  });

  it("shows the failure state, not a crash, when the answer holds no dataset list", async () => {
    renderPart(<ReadableDatasets />, {
      answer: (url: URL) => (url.pathname === "/api/v1/catalogue/readable" ? json({ unanswered: 0 }) : undefined),
    });
    expect(await screen.findByRole("button", { name: en.app.error.retry })).toBeInTheDocument();
  });

  it("says what to do next when nothing internal is shared, and warns of what it could not check", async () => {
    renderPart(<ReadableDatasets />, { answer: readable([row("bikes", "public")], 2) });
    expect(await screen.findByText(en.catalogue.readable.empty.title)).toBeInTheDocument();
    expect(screen.getByText(en.catalogue.readable.empty.description)).toBeInTheDocument();
    expect(screen.getByText(/2 datasets could not be checked/)).toBeInTheDocument();
  });

  it.each(SUPPORTED_LOCALES)("says everything in %s with no raw key", async (locale) => {
    await i18n.changeLanguage(locale);
    const { container } = renderPart(<ReadableDatasets />, { answer: readable(ANALYST, 1) });
    await screen.findAllByRole("listitem");
    expectNoRawKeys(container);
  });
});

describe("a dataset by its Endpoint (T-6411)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("asks a visitor of a non-public Endpoint to sign in, never an empty grid", async () => {
    status = "anonymous";
    renderPart(<EndpointDatasetPage slug="countszzzzzzzzzzzzzzzzzzzzzz" />, {
      answer: (url) => (url.pathname.startsWith("/api/endpoint/") ? refusal(401) : undefined),
    });
    expect(await screen.findByText(en.entityGrid.signedOutTitle)).toBeInTheDocument();
    expect(screen.queryByRole("grid")).toBeNull();
    expect(screen.getByRole("heading", { level: 1, name: en.catalogue.readable.endpoint.visitorTitle })).toBeInTheDocument();
    // A visitor's page never asks what a signed-in person may read.
    const asked = (globalThis.fetch as unknown as { mock: { calls: unknown[][] } }).mock.calls.map((call) => String(call[0]));
    expect(asked.some((url) => url.includes("/catalogue/readable"))).toBe(false);
  });

  it("tells a signed-in person without a grant they have no access", async () => {
    renderPart(<EndpointDatasetPage slug="countszzzzzzzzzzzzzzzzzzzzzz" />, {
      answer: (url) => (url.pathname.startsWith("/api/endpoint/") ? refusal(403) : readable([row("bikes", "public")])(url)),
    });
    expect(await screen.findByText(en.entityGrid.noAccessTitle)).toBeInTheDocument();
    expect(screen.queryByRole("grid")).toBeNull();
    expect(screen.queryByRole("button", { name: en.entityGrid.signIn })).toBeNull();
    // The page says why it cannot name the dataset, never a bare "Dataset".
    expect(await screen.findByRole("heading", { level: 1, name: en.catalogue.readable.endpoint.refusedTitle })).toBeInTheDocument();
    expect(screen.getByText(en.catalogue.readable.endpoint.refusedLead)).toBeInTheDocument();
  });

  it("names the dataset and what grants it to a person who may read it", async () => {
    renderPart(<EndpointDatasetPage slug={ANALYST[2]?.slug ?? ""} />, {
      answer: readable(ANALYST),
    });
    expect(await screen.findByRole("heading", { level: 1, name: "Incidents" })).toBeInTheDocument();
    expect(screen.getByText("Group traffic-analysts")).toBeInTheDocument();
    await waitFor(() => expect(screen.queryByText(en.entityGrid.noAccessTitle)).toBeNull());
  });
});

describe("the member shell (T-6411)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("offers the catalogue, AI connectors and settings, and no project page", async () => {
    const { container } = renderPart(
      <MemberFrame>
        <h1>page</h1>
      </MemberFrame>,
    );
    const nav = screen.getByRole("navigation", { name: en.member.nav });
    const links = [...nav.querySelectorAll("a")].map((a) => [a.textContent, a.getAttribute("href")]);
    expect(links).toEqual([
      [en.member.catalogue, "/catalogue"],
      [en.member.connectors, "/ai-connectors"],
      [en.member.settings, "/settings"],
    ]);
    expect(container.querySelector('a[href^="/projects/"]')).toBeNull();
    screen.getByRole("button", { name: en.member.signOut }).click();
    expect(signOut).toHaveBeenCalled();
    await expectNoViolations(container);
  });
});
