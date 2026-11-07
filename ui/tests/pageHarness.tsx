/**
 * T-1788…T-1796: one harness for the page tests of the `ui-pages` group (UI-01, UI-11, UI-15,
 * UI-16, UI-44, PF-50).
 *
 * Every one of those tasks asks the same six things of a page — the four states, a list that
 * survives 0, 1 and 500 rows, the keyboard, four locales, the tokens under a brand that is not
 * the default, and axe — and nine copies of a fetch stub is how that checklist rots. The page
 * mounts through `App`, so the route, the shell, the landmarks and the providers are the real
 * ones; only the API is answered from here.
 */
import { createContext, useContext, useState } from "react";
import type { JSX, ReactNode } from "react";
import { render, screen, waitFor } from "@testing-library/react";
import type { RenderResult } from "@testing-library/react";
import axe from "axe-core";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { RouterProvider, createRootRoute, createRouter } from "@tanstack/react-router";
import { expect, vi } from "vitest";
import i18n from "../src/i18n";
import { App } from "../src/App";

export const VIEWER = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  email: "jana.kovacova@banskabystrica.sk",
  roles: ["portal-editor"],
};

/** The four languages the Portal ships; a string English in all four is a string nobody translated. */
export const LOCALES = ["en", "sk", "cs", "de"] as const;

export const list = (items: unknown[]) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items,
});

export function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

/** RFC 9457, which is what the Portal's client turns into the sentence a page shows. */
export function problem(status: number, detail: string): Response {
  return new Response(JSON.stringify({ type: "about:blank", title: detail, status, detail }), {
    status,
    headers: { "Content-Type": "application/problem+json" },
  });
}

export interface RouteOptions {
  path: string;
  /** What the API answers for a path under `/api/v1`; `undefined` falls through to an empty list. */
  answer?: (path: string, request: Request) => Response | Promise<Response> | undefined;
  identity?: Record<string, unknown> | null;
  permissions?: Record<string, unknown>;
  /** The page's own requests never settle, so it stays in its loading state for the assertion. */
  pending?: boolean;
  locale?: (typeof LOCALES)[number];
  /** What the project list answers, when not the one project `helsinki`. */
  projects?: () => Response;
  /** A brand that is not the default, to catch a colour written as a literal (UI-15). */
  brand?: Record<string, unknown>;
}

const DEFAULT_BRAND = {
  instanceName: "joinedcontext",
  shortName: "joinedcontext",
  city: "Helsinki",
  organisation: "City of Helsinki",
  orgDomain: "hel.fi",
  domain: "portal.hel.fi",
  contactEmail: "data@hel.fi",
  licenseDefault: "CC-BY-4.0",
  logo: "",
  favicon: "",
  colours: {
    primary: "#1d4ed8",
    secondary: "#0f766e",
    accent: "#f59e0b",
    background: "#ffffff",
    text: "#0f172a",
  },
  fonts: { heading: "system-ui, sans-serif", body: "system-ui, sans-serif" },
  languages: { default: "en", offered: ["en", "sk", "cs", "de"] },
  primaryForeground: "#ffffff",
};

/** A brand with none of the Portal's own colours in it: a literal hex shows up against this. */
export const OTHER_BRAND = {
  ...DEFAULT_BRAND,
  colours: { ...DEFAULT_BRAND.colours, primary: "#7c2d12", accent: "#4c1d95" },
};

export interface Rendered extends RenderResult {
  fetchMock: ReturnType<typeof vi.fn>;
  /** The requests the page sent, as `METHOD /path`. */
  calls(): string[];
  client: QueryClient;
}

export async function renderRoute(options: RouteOptions): Promise<Rendered> {
  await i18n.changeLanguage(options.locale ?? "en");
  window.history.pushState({}, "", options.path);

  const fetchMock = vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
    const request =
      input instanceof Request ? input : new Request(new URL(String(input), window.location.origin), init);
    const path = new URL(request.url, window.location.origin).pathname;
    if (path.endsWith("/auth/me")) {
      return Promise.resolve(
        options.identity === null
          ? new Response(null, { status: 401 })
          : jsonResponse(options.identity ?? VIEWER),
      );
    }
    if (path.endsWith("/branding")) {
      return Promise.resolve(jsonResponse(options.brand ?? DEFAULT_BRAND));
    }
    if (path.endsWith("/permissions/me")) {
      return Promise.resolve(
        jsonResponse(options.permissions ?? { project: "helsinki", bootstrap: true, grants: [] }),
      );
    }
    if (path === "/api/v1/projects") {
      return Promise.resolve(options.projects?.() ?? jsonResponse(list([{ name: "helsinki" }])));
    }
    // `pending` holds the page's own data, never the shell's: an identity that never arrives
    // renders the login page instead of the page under test.
    if (options.pending) {
      return new Promise<Response>(() => {});
    }
    const answered = options.answer?.(path, request);
    return Promise.resolve(answered ?? jsonResponse(list([])));
  });
  vi.stubGlobal("fetch", fetchMock);

  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const rendered = render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <App />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return Object.assign(rendered, {
    client,
    fetchMock,
    calls: () =>
      fetchMock.mock.calls.map((call) => {
        const request = call[0] as Request;
        return `${request.method} ${new URL(request.url, window.location.origin).pathname}`;
      }),
  });
}

/** Axe over what the page rendered, with the rules it broke in the failure message. */
export async function expectAxeClean(container: HTMLElement): Promise<void> {
  const results = await axe.run(container);
  const summary = results.violations
    .map((v) => `${v.id}: ${v.description} (${v.nodes.map((n) => n.html).join("; ")})`)
    .join("\n");
  expect(results.violations, summary).toEqual([]);
}

/** The page's one level-1 heading, once the shell has settled (UI-01). */
export async function expectOneH1(text?: string | RegExp): Promise<HTMLElement> {
  const headings = await waitFor(() => {
    const found = screen.getAllByRole("heading", { level: 1 });
    expect(found.length, "a page has exactly one h1").toBe(1);
    return found;
  });
  if (text !== undefined) {
    expect(headings[0]).toHaveTextContent(text);
  }
  return headings[0];
}

const Slot = createContext<ReactNode>(null);

function SlotOutlet(): JSX.Element {
  return <>{useContext(Slot)}</>;
}

/**
 * A component under test inside a router of one route, for a part that renders links (a type
 * that opens its model, T-2766) but is mounted without the App. Its children follow re-renders.
 */
export function InRouter({ children }: { children: ReactNode }): JSX.Element {
  const [router] = useState(() => createRouter({ routeTree: createRootRoute({ component: SlotOutlet }) }));
  return (
    <Slot.Provider value={children}>
      <RouterProvider router={router} />
    </Slot.Provider>
  );
}
