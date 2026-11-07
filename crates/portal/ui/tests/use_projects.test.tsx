/**
 * PF-05, T-0794 (T-2137): the two reads every page starts from — which projects there are, and
 * which domain the organization writes its ids under.
 *
 * `useOrgDomain` is the one that matters beyond a label: an id minted under a fabricated
 * `<project>.sk` names an IRI nobody owns, so the order it resolves in — the Organization
 * manifest, then the installation's branding, then the project name — is held here against the
 * order the API itself uses.
 */
import { renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ReactNode } from "react";
import { BrandingProvider } from "../src/branding";
import { useOrgDomain, useProjects } from "../src/api/projects";
import { json, list } from "./page_contract";

const BRANDING = {
  instanceName: "Helsinki Region Context",
  shortName: "Helsinki",
  orgDomain: "hel.fi",
  colours: { primary: "#1d4ed8", secondary: "#0f766e", accent: "#f59e0b", background: "#fff", text: "#000" },
  primaryForeground: "#ffffff",
  languages: { default: "en", offered: ["en"] },
};

function organization(domain?: string) {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Organization",
    metadata: { name: "org", namespace: "org" },
    spec: domain ? { domain } : {},
  };
}

function answering(answer: (url: URL) => Response | undefined) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      // The generated client calls `fetch` with a `Request`, whose `String()` is not its URL.
      const raw = input instanceof Request ? input.url : String(input);
      const url = new URL(raw, window.location.origin);
      return answer(url) ?? json(list([]));
    }),
  );
}

const wrapper = ({ children }: { children: ReactNode }) => (
  <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
    <BrandingProvider>{children}</BrandingProvider>
  </QueryClientProvider>
);

afterEach(() => vi.restoreAllMocks());

describe("the projects a person can open", () => {
  it("is the names the API listed, in its order", async () => {
    answering((url) =>
      url.pathname === "/api/v1/projects"
        ? json(list([{ name: "helsinki" }, { name: "banskabystrica" }]))
        : undefined,
    );
    const { result } = renderHook(() => useProjects(), { wrapper });
    await waitFor(() => expect(result.current.data).toEqual(["helsinki", "banskabystrica"]));
  });

  it("is an empty list, not a failure, for a person who may open none", async () => {
    answering((url) => (url.pathname === "/api/v1/projects" ? json(list([])) : undefined));
    const { result } = renderHook(() => useProjects(), { wrapper });
    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    expect(result.current.data).toEqual([]);
  });

  it("is a failure a page can show when the read is refused", async () => {
    answering((url) =>
      url.pathname === "/api/v1/projects"
        ? json({ type: "about:blank", title: "Forbidden", status: 403, detail: "no" }, 403)
        : undefined,
    );
    const { result } = renderHook(() => useProjects(), { wrapper });
    await waitFor(() => expect(result.current.isError).toBe(true));
  });
});

describe("the domain an organization writes its ids under", () => {
  const withOrganizations = (items: unknown[]) =>
    answering((url) =>
      url.pathname.endsWith("/branding")
        ? json(BRANDING)
        : url.pathname.endsWith("/organizations")
          ? json(list(items))
          : undefined,
    );

  it("is the Organization manifest's own domain when the repository holds one", async () => {
    withOrganizations([organization("banskabystrica.sk")]);
    const { result } = renderHook(() => useOrgDomain("helsinki"), { wrapper });
    await waitFor(() => expect(result.current).toBe("banskabystrica.sk"));
  });

  it("falls back to the installation's branding, and never to a fabricated name", async () => {
    withOrganizations([organization(undefined)]);
    const { result } = renderHook(() => useOrgDomain("helsinki"), { wrapper });
    await waitFor(() => expect(result.current).toBe("hel.fi"));
  });

  it("is the project's own name only when neither says anything", async () => {
    answering((url) => (url.pathname.endsWith("/organizations") ? json(list([])) : undefined));
    const { result } = renderHook(() => useOrgDomain("helsinki"), { wrapper });
    await waitFor(() => expect(result.current).toBe("helsinki"));
  });

  it("answers the project's name while the reads are still in flight", () => {
    answering(() => undefined);
    const { result } = renderHook(() => useOrgDomain("helsinki"), { wrapper });
    // Never `undefined` and never an empty string: an id is minted from this.
    expect(result.current).toBe("helsinki");
  });
});
