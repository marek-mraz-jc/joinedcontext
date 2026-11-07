// covers: src/api/projects.ts src/pages/access/RoleBindings.tsx
/**
 * ADR-N-058: an installation holds several Organizations, and a project's domain, locales and
 * organization scope are its own Organization's, never the first of the list nor a guess from the
 * domain (F-UI1-010, F-UI1-011).
 */
import { renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { organizationNameOf, useOrgDomain, useOrgLocales } from "../src/api/projects";
import { bindingOf } from "../src/pages/access/RoleBindings";

const organization = (name: string, domain: string, locales: string[]) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Organization",
  metadata: { name, namespace: "org" },
  spec: { domain, locales },
});

function wrapper({ children }: { children: ReactNode }) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
}

describe("the Organization of a project", () => {
  beforeEach(() => {
    vi.stubGlobal(
      "fetch",
      vi.fn((input: RequestInfo | URL) => {
        const url = new URL((input as Request).url ?? String(input), "http://localhost");
        const body = url.pathname === "/api/v1/projects"
          ? { items: [{ name: "helsinki", sample: false, organization: "hel" }, { name: "zilina", sample: false, organization: "za" }], organizations: [{ name: "hel" }, { name: "za" }] }
          : { items: [organization("hel", "hel.fi", ["fi"]), organization("za", "zilina.sk", ["sk", "en"])] };
        return Promise.resolve(new Response(JSON.stringify(body), { status: 200, headers: { "Content-Type": "application/json" } }));
      }),
    );
  });
  afterEach(() => vi.unstubAllGlobals());

  it("gives a project of the second Organization that Organization's domain and locales", async () => {
    const domain = renderHook(() => useOrgDomain("zilina"), { wrapper });
    await waitFor(() => expect(domain.result.current).toBe("zilina.sk"));
    const locales = renderHook(() => useOrgLocales("zilina"), { wrapper });
    await waitFor(() => expect(locales.result.current).toEqual(["sk", "en"]));
  });

  it("names the owner, the chosen one on the organization's pages, or the only one", () => {
    const list = { organizations: [{ name: "hel" }, { name: "za" }], ownerOf: new Map([["zilina", "za"]]) };
    expect(organizationNameOf(list, "zilina")).toBe("za");
    expect(organizationNameOf(list, "org", "hel")).toBe("hel");
    expect(organizationNameOf(list, "org")).toBeUndefined();
    expect(organizationNameOf({ organizations: [{ name: "mesto-bb" }], ownerOf: new Map() }, "org")).toBe("mesto-bb");
    expect(organizationNameOf(undefined, "zilina")).toBeUndefined();
  });

  it("scopes an organization binding by the Organization's own name", () => {
    const binding = bindingOf({ subjectKind: "user", subject: "jana@banskabystrica.sk", role: "viewer", place: "organization", until: "" }, "banskabystrica", "mesto-bb");
    expect((binding.spec as { scope: unknown }).scope).toEqual({ organization: "mesto-bb" });
  });
});
