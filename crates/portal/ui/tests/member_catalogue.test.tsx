// covers (T-2137, the module gate in gate_modules.test.ts): src/pages/catalogue/ReadableDatasets.tsx,
// src/pages/catalogue/EndpointDatasetPage.tsx, src/components/layout/MemberFrame.tsx.
// ADR-N-067 §2.4 (T-6411): the signed-in catalogue lists what the server says the person may
// read, each row marked by what grants it; a dataset page by its Endpoint shows "sign in" to a
// visitor of a non-public Endpoint, "no access" to a person without a grant, rows to a person
// with one; a member gets the member shell.
import type { ReactNode } from "react";
import { fireEvent, screen, waitFor } from "@testing-library/react";
import type { Granted } from "../src/pages/catalogue/ReadableDatasets";
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
  useNavigate: () => () => Promise.resolve(),
  useSearch: () => ({}),
}));

const { AccessFacet, GrantedCard } = await import("../src/pages/catalogue/ReadableDatasets");
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

describe("the datasets a signed-in person may read (T-6411, T-7389)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  const granted = (datasets: ReturnType<typeof row>[]): Granted => ({ state: "ready", datasets, unanswered: 0 });

  it("marks each granted dataset and names what grants it", async () => {
    const { container } = renderPart(
      <ul>
        {ANALYST.slice(1).map((dataset) => (
          <li key={dataset.slug}>
            <GrantedCard dataset={dataset} />
          </li>
        ))}
      </ul>,
    );
    const counts = await screen.findByRole("link", { name: "Counts" });
    expect(counts).toHaveAttribute("href", "/catalogue/endpoint/countszzzzzzzzzzzzzzzzzzzzzz");
    expect(screen.getAllByText(en.catalogue.access.badge)).toHaveLength(2);
    expect(screen.getByText(`helsinki · ${en.catalogue.readable.access.organization}`)).toBeInTheDocument();
    expect(screen.getByText("helsinki · Group traffic-analysts")).toBeInTheDocument();
    await expectNoViolations(container);
  });

  it("counts the granted and the public datasets, and picks one of them", async () => {
    const onSelect = vi.fn();
    const { container } = renderPart(
      <AccessFacet granted={granted(ANALYST.slice(1))} publicCount={626} selected={undefined} onSelect={onSelect} />,
    );
    const mine = await screen.findByRole("checkbox", { name: /Granted to me/ });
    expect(mine.closest("label")?.textContent).toBe("Granted to me2");
    expect(screen.getByRole("checkbox", { name: /Public/ }).closest("label")?.textContent).toBe("Public626");
    fireEvent.click(mine);
    expect(onSelect).toHaveBeenCalledWith("granted");
    await expectNoViolations(container);
  });

  it("unticks the chosen access", async () => {
    const onSelect = vi.fn();
    renderPart(<AccessFacet granted={granted(ANALYST.slice(1))} publicCount={3} selected="public" onSelect={onSelect} />);
    fireEvent.click(await screen.findByRole("checkbox", { name: /Public/ }));
    expect(onSelect).toHaveBeenCalledWith(undefined);
  });

  it.each([
    ["ready", en.catalogue.access.none],
    ["noProject", en.catalogue.access.noProject],
    ["failed", en.catalogue.access.failed],
    ["loading", en.catalogue.access.loading],
  ] as const)("refuses Granted to me (0) when %s and says why", async (state, words) => {
    const onSelect = vi.fn();
    const { container } = renderPart(
      <AccessFacet granted={{ state, datasets: [], unanswered: 0 }} publicCount={626} selected={undefined} onSelect={onSelect} />,
    );
    const mine = await screen.findByRole("checkbox", { name: /Granted to me/ });
    expect(mine).toHaveAttribute("aria-disabled", "true");
    expect(mine).toHaveAccessibleDescription(words);
    fireEvent.click(mine);
    expect(onSelect).not.toHaveBeenCalled();
    expect(screen.getAllByText(words)).toHaveLength(2);
    await expectNoViolations(container);
  });

  it.each(SUPPORTED_LOCALES)("says everything in %s with no raw key", async (locale) => {
    await i18n.changeLanguage(locale);
    const { container } = renderPart(
      <>
        <AccessFacet granted={{ state: "noProject", datasets: [], unanswered: 0 }} publicCount={1} selected={undefined} onSelect={() => undefined} />
        <GrantedCard dataset={ANALYST[2]!} />
      </>,
    );
    await screen.findAllByRole("checkbox");
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

  it("names a public dataset to a visitor by its Endpoint's own DCAT title (T-7072)", async () => {
    status = "anonymous";
    renderPart(<EndpointDatasetPage slug="lightszzzzzzzzzzzzzzzzzzzzzz" />, {
      answer: (url) =>
        url.pathname === "/api/endpoint/lightszzzzzzzzzzzzzzzzzzzzzz/"
          ? json({ "dct:title": [{ "@value": "Pouličné svietidlá", "@language": "sk" }, { "@value": "Street lights", "@language": "en" }] })
          : url.pathname.startsWith("/api/endpoint/")
            ? refusal(401)
            : undefined,
    });
    expect(await screen.findByRole("heading", { level: 1, name: "Street lights" })).toBeInTheDocument();
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

  it("carries the skip link, the language switch and the accessibility statement every page has", async () => {
    const { container } = renderPart(
      <MemberFrame>
        <h1>page</h1>
      </MemberFrame>,
    );
    expect(screen.getByRole("link", { name: en.nav.skipToContent })).toHaveAttribute("href", "#main");
    expect(screen.getByRole("button", { name: en.lang.label })).toBeInTheDocument();
    expect(container.querySelector('a[href="/accessibility"]')).not.toBeNull();
    await expectNoViolations(container);
  });
});
