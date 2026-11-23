/**
 * Endpoint page parts at their edges: the access matrix that failed and is asked again, the grants
 * section while loading and when its list failed, an admission that could not be read, a usage
 * export the server refused, an unknown sharing audience, and the copy button and the
 * representation summary on pointer and clipboard failures.
 */
import type { Binding } from "../src/components/endpoints/policyBinding";
import { render, screen, waitFor, fireEvent } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ReactNode } from "react";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { ApiError } from "../src/api/client";
import type { Manifest } from "../src/api/manifest";
import { AccessMatrix } from "../src/components/endpoints/AccessMatrix";
import { AssistantGuides } from "../src/components/endpoints/AssistantGuides";
import { PolicyGrants } from "../src/components/endpoints/PolicyGrants";
import { EndpointRefused, useEndpointAdmission } from "../src/components/endpoints/EndpointAdmission";
import { ProjectUsageSection } from "../src/components/endpoints/UsageSection";
import { admits } from "../src/components/endpoints/sharing";
import { CopyUrlButton, RepresentationSummary } from "../src/components/endpoints/links";
import { InRouter, jsonResponse, problem } from "./pageHarness";

let paths: string[] = [];
let route: (url: URL) => Response;

function mount(node: ReactNode) {
  return render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>
        <InRouter>{node}</InRouter>
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
  paths = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const request = input instanceof Request ? input : new Request(String(input), undefined);
      const url = new URL(request.url, "http://localhost");
      paths.push(url.pathname);
      return route(url);
    }),
  );
});
afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("AccessMatrix", () => {
  it("says the matrix could not be read and asks again on retry", async () => {
    route = () => problem(500, "the matrix is away");
    mount(<AccessMatrix project="helsinki" name="bikes-ops" />);
    expect(await screen.findByText(/the matrix is away/)).toBeInTheDocument();
    const before = paths.filter((p) => p.endsWith("/access/matrix")).length;
    await userEvent.click(screen.getByRole("button", { name: en.app.error.retry }));
    await waitFor(() => expect(paths.filter((p) => p.endsWith("/access/matrix")).length).toBeGreaterThan(before));
  });
});

describe("PolicyGrants", () => {
  const binding: Binding = { kind: "bound", policies: [] };

  it("says it is loading while the policies are read", async () => {
    route = () => jsonResponse({ items: [] });
    mount(<PolicyGrants project="helsinki" binding={binding} pending failure={undefined} />);
    expect(await screen.findByRole("status")).toHaveTextContent(en.app.loading);
  });

  it("says the list failed, never that nothing is granted, and retries on request", async () => {
    route = () => jsonResponse({ items: [] });
    const retry = vi.fn();
    mount(<PolicyGrants project="helsinki" binding={binding} pending={false} failure={{ error: new ApiError(500, "Server", { type: "about:blank", title: "Server", status: 500, detail: "policies unreadable" }), retry }} />);
    expect(await screen.findByText(/policies unreadable/)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: en.app.error.retry }));
    expect(retry).toHaveBeenCalledTimes(1);
  });
});

describe("PolicyGrants states", () => {
  const policy = (n: number): Manifest =>
    ({
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Policy",
      metadata: { name: `p-${n}`, namespace: "helsinki", title: { en: `Policy number ${n}` } },
      spec: { assignee: { kind: "user", id: "a@hel.fi" }, operations: ["retrieveOps"], information: [{ entities: [{ type: "Station" }] }] },
    }) as Manifest;

  it("warns that an unbound reference grants nothing, and says when no Policy exists", async () => {
    route = () => jsonResponse({ items: [] });
    const first = mount(<PolicyGrants project="helsinki" binding={{ kind: "unbound", urn: "urn:x:y" }} pending={false} />);
    expect(await screen.findByText(/urn:x:y/)).toBeInTheDocument();
    first.unmount();
    mount(<PolicyGrants project="helsinki" binding={{ kind: "all", policies: [] }} pending={false} />);
    expect(await screen.findByText(en.endpoints.page.noPolicy)).toBeInTheDocument();
  });

  it("folds a long list of Policies and opens and closes the rest on request", async () => {
    route = () => jsonResponse({ items: [] });
    const policies = Array.from({ length: 12 }, (_, n) => policy(n));
    mount(<PolicyGrants project="helsinki" binding={{ kind: "all", policies }} pending={false} />);
    const more = await screen.findByRole("button", { name: en.endpoints.page.policiesAll.replace("{count}", "12") });
    const folded = screen.getAllByRole("listitem").length;
    await userEvent.click(more);
    expect(screen.getAllByRole("listitem").length).toBeGreaterThan(folded);
    await userEvent.click(screen.getByRole("button", { name: en.endpoints.page.policiesFewer }));
    expect(screen.getAllByRole("listitem")).toHaveLength(folded);
  });
});

describe("EndpointRefused", () => {
  function Probe() {
    const admission = useEndpointAdmission("helsinki", "bikes-ops");
    return <EndpointRefused project="helsinki" name="bikes-ops" admission={admission} />;
  }

  it("says the standing could not be read when the admission answer fails", async () => {
    route = () => problem(500, "no admission");
    mount(<Probe />);
    expect(await screen.findByText(en.endpoints.refused.unavailable)).toBeInTheDocument();
    expect(paths).toContain("/api/v1/projects/helsinki/endpoints/bikes-ops/admission");
  });
});

describe("ProjectUsageSection export", () => {
  it("shows the server's refusal when the month cannot be exported", async () => {
    route = (url) =>
      url.pathname.endsWith("/export")
        ? problem(403, "usage export is not yours")
        : jsonResponse({ from: "2026-09-23", to: "2026-10-22", endpoints: [] });
    mount(<ProjectUsageSection project="mesto" />);
    await userEvent.click(await screen.findByRole("button", { name: en.endpointUsage.export }));
    expect(await screen.findByText(/usage export is not yours/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: en.endpointUsage.export })).toBeEnabled();
  });
});

describe("admits", () => {
  const endpoint = (spec: Record<string, unknown>) =>
    ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "Endpoint", metadata: { name: "e", namespace: "owner" }, spec }) as Manifest;

  it("admits the owner always, a listed project on a list, and nobody on an audience it does not know", () => {
    expect(admits(endpoint({ audience: "private" }), "owner", "owner")).toBe(true);
    expect(admits(endpoint({ audience: "private" }), "owner", "other")).toBe(false);
    expect(admits(endpoint({ audience: "project-list", allowedProjects: ["friend"] }), "owner", "friend")).toBe(true);
    expect(admits(endpoint({ audience: "project-list", allowedProjects: ["friend"] }), "owner", "other")).toBe(false);
    expect(admits(endpoint({ audience: "organization" }), "owner", "other")).toBe(true);
  });
});

describe("CopyUrlButton", () => {
  it("copies the address and then says so, and stays as it was when the clipboard refuses", async () => {
    const writeText = vi.fn(() => Promise.resolve());
    vi.stubGlobal("navigator", { ...navigator, clipboard: { writeText } });
    const first = mount(<CopyUrlButton url="https://x.example/a" />);
    await userEvent.click(await screen.findByRole("button", { name: en.endpoints.copyUrl }));
    expect(writeText).toHaveBeenCalledWith("https://x.example/a");
    expect(await screen.findByRole("button", { name: en.endpoints.copied })).toBeInTheDocument();
    first.unmount();

    const refuse = vi.fn(() => Promise.reject(new Error("denied")));
    vi.stubGlobal("navigator", { ...navigator, clipboard: { writeText: refuse } });
    mount(<CopyUrlButton url="https://x.example/b" />);
    await userEvent.click(await screen.findByRole("button", { name: en.endpoints.copyUrl }));
    expect(refuse).toHaveBeenCalledWith("https://x.example/b");
    expect(screen.getByRole("button", { name: en.endpoints.copyUrl })).toBeInTheDocument();
  });
});

describe("RepresentationSummary pointer and blur", () => {
  it("shows the rest while the pointer is over the count and hides it when it leaves or focus goes", async () => {
    mount(<RepresentationSummary served={["ngsi-ld", "mcp", "ogc-features", "sta", "ical"]} />);
    const more = await screen.findByText(en.endpoints.field.more.replace("{count}", "2"));
    const tip = screen.getByRole("tooltip", { hidden: true });
    fireEvent.mouseEnter(more);
    expect(tip).not.toHaveClass("sr-only");
    fireEvent.mouseLeave(more);
    expect(tip).toHaveClass("sr-only");
    fireEvent.focus(more);
    expect(tip).not.toHaveClass("sr-only");
    fireEvent.blur(more);
    expect(tip).toHaveClass("sr-only");
  });
});

describe("AssistantGuides", () => {
  it("links each assistant's section of the open data connector page, and names none without a host", async () => {
    route = (url) =>
      url.pathname === "/api/v1/organization/addresses"
        ? jsonResponse({ items: [{ organization: "zilina", title: "Zilina", addresses: [{ kind: "opendata", url: "https://opendata.zilina.example/" }] }] })
        : jsonResponse({});
    const first = mount(<AssistantGuides />);
    const list = await screen.findByRole("list", { name: en.assistantGuides.list });
    const hrefs = Array.from(list.querySelectorAll("a")).map((a) => a.getAttribute("href"));
    expect(hrefs).toEqual([
      "https://opendata.zilina.example/connector#claude",
      "https://opendata.zilina.example/connector#chatgpt",
      "https://opendata.zilina.example/connector#grok",
      "https://opendata.zilina.example/connector#other",
    ]);
    expect(screen.getByRole("link", { name: /ChatGPT/ })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: new RegExp(en.assistantGuides.other) })).toBeInTheDocument();
    first.unmount();

    route = () => problem(401, "sign in");
    mount(<AssistantGuides />);
    expect(await screen.findByText(en.assistantGuides.noHost)).toBeInTheDocument();
  });
});
