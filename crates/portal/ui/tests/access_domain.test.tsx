/**
 * The organization's domain on the Access page (T-2571, PF-41, Architecture/03 §3): whether it is
 * verified, how and when it was checked, why it failed, and — until it is verified — the TXT
 * record to publish, copied exactly.
 */
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { RouterProvider, createMemoryHistory, createRootRoute, createRouter } from "@tanstack/react-router";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { App } from "../src/App";
import { OrganizationDomain } from "../src/pages/access/OrganizationDomain";
import en from "../src/locales/en.json";
import { answeringChecks, expectNoViolations } from "./checks";

const LIST = "joinedcontext.com/v1alpha1";
const list = (items: unknown[]) => ({ apiVersion: LIST, kind: "List", items });
const RECORD = '_joinedcontext.hel.fi TXT "jc-verify=Q2hhbGxlbmdlLW9mLXRoaXMtaW5zdGFuY2U"';

function organization(domainVerification: unknown) {
  return {
    apiVersion: LIST,
    kind: "Organization",
    metadata: { name: "hel", namespace: "org" },
    spec: { domain: "hel.fi" },
    status: { phase: "Live", ...(domainVerification ? { domainVerification } : {}) },
  };
}

function renderAccess(domainVerification: unknown, page = false) {
  const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
    const request = typeof input === "string" || input instanceof URL ? null : input;
    const path = new URL(request ? request.url : String(input), window.location.origin).pathname;
    const json = (body: unknown) =>
      new Response(JSON.stringify(body), { status: 200, headers: { "Content-Type": "application/json" } });
    if (path.endsWith("/auth/me")) {
      return json({ subject: "b7c1e0f4", username: "jana.kovacova", name: "Jana Kováčová", roles: [] });
    }
    if (path === "/api/v1/projects/org/organizations") {
      return json(list([organization(domainVerification)]));
    }
    // The Organization page stands in the shell of the first project (T-2605).
    if (path === "/api/v1/projects") {
      return json(list([{ name: "banskabystrica" }]));
    }
    return json(list([]));
  });
  vi.stubGlobal("fetch", fetchMock);
  vi.stubGlobal("fetch", answeringChecks(globalThis.fetch));
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        {page ? (
          <App />
        ) : (
          // The section reads ?org= through the router (T-5248), so it stands in one as on the page.
          <RouterProvider
            router={createRouter({
              routeTree: createRootRoute({ component: OrganizationDomain }),
              history: createMemoryHistory({ initialEntries: ["/organization/settings"] }),
            })}
          />
        )}
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

async function section(): Promise<HTMLElement> {
  const heading = await screen.findByRole("heading", { name: en.access.domain.title });
  return heading.closest("section") as HTMLElement;
}

describe("the organization's domain on Organization → Settings", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/organization/settings");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("shows a pending domain with the record to publish, and copies it exactly", async () => {
    const user = userEvent.setup();
    renderAccess({ state: "pending", challenge: "Q2hhbGxlbmdlLW9mLXRoaXMtaW5zdGFuY2U", record: RECORD });
    const domain = await section();

    expect(within(domain).getByText("hel.fi")).toBeTruthy();
    expect(within(domain).getByText(en.access.domain.state.pending)).toBeTruthy();
    expect(within(domain).getByTestId("domain-record").textContent).toBe(RECORD);

    await user.click(within(domain).getByRole("button", { name: en.access.domain.copy }));
    expect(await navigator.clipboard.readText()).toBe(RECORD);
    expect(within(domain).getByRole("button", { name: en.access.domain.copied })).toBeTruthy();
    await expectNoViolations(domain);
  });

  it("shows why a failed domain failed, and the record to put right", async () => {
    renderAccess({
      state: "failed",
      checkedAt: "2026-09-21T08:30:00Z",
      reason: "_joinedcontext.hel.fi has no TXT record carrying this instance's challenge",
      challenge: "Q2hhbGxlbmdlLW9mLXRoaXMtaW5zdGFuY2U",
      record: RECORD,
    });
    const domain = await section();

    expect(within(domain).getByText(en.access.domain.state.failed)).toBeTruthy();
    expect(within(domain).getByText(/has no TXT record carrying this instance's challenge/)).toBeTruthy();
    expect(within(domain).getByText(/^Last checked /)).toBeTruthy();
    expect(within(domain).getByTestId("domain-record").textContent).toBe(RECORD);
  });

  it("shows how and when a verified domain was checked, and no record", async () => {
    renderAccess({
      state: "verified",
      method: "dns-txt",
      checkedAt: "2026-09-21T08:30:00Z",
      challenge: "Q2hhbGxlbmdlLW9mLXRoaXMtaW5zdGFuY2U",
      record: RECORD,
    });
    const domain = await section();

    expect(within(domain).getByText(en.access.domain.state.verified)).toBeTruthy();
    expect(within(domain).getByText(/^Verified through a DNS TXT record, checked /)).toBeTruthy();
    expect(within(domain).queryByTestId("domain-record")).toBeNull();
    expect(within(domain).queryByRole("button", { name: en.access.domain.copy })).toBeNull();
  });

  it("says a domain nobody has checked yet is unchecked, without inventing a state", async () => {
    renderAccess(null);
    const domain = await section();

    expect(within(domain).getByText(en.access.domain.unchecked)).toBeTruthy();
    expect(within(domain).queryByTestId("domain-record")).toBeNull();
  });

  it("stands on Organization → Settings", async () => {
    renderAccess({ state: "pending", challenge: "Q2hhbGxlbmdlLW9mLXRoaXMtaW5zdGFuY2U", record: RECORD }, true);
    const domain = await section();
    expect(within(domain).getByTestId("domain-record").textContent).toBe(RECORD);
  });

  it("reads the same in Slovak", async () => {
    await i18n.changeLanguage("sk");
    renderAccess({ state: "pending", challenge: "Q2hhbGxlbmdlLW9mLXRoaXMtaW5zdGFuY2U", record: RECORD });

    const heading = await screen.findByRole("heading", { name: "Doména" });
    const domain = heading.closest("section") as HTMLElement;
    expect(within(domain).getByText("Zatiaľ neoverená")).toBeTruthy();
    expect(within(domain).getByRole("button", { name: "Kopírovať záznam" })).toBeTruthy();
  });
});
