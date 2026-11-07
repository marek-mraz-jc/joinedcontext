/**
 * T-1777…T-1787: the page contract as one harness (UI-11, UI-15, UI-16, UI-44).
 *
 * Every page test in the `ui-pages` group asks the same six questions — the header, the four
 * states, the lists, landmarks and axe, the keyboard, the locales — and each answer needs the
 * same providers around the page: a query client that does not retry, i18n, the branding that
 * paints the tokens, and a router for the pages that navigate. Written once here so a page test
 * is the page's own cases and nothing else, and so a rule that changes changes in one file.
 *
 * What this file deliberately does not do: a dark-theme rendering. The Portal defines no dark
 * theme (no `prefers-color-scheme` block, no `.dark` variant, no `data-theme` hook in
 * `src/index.css`), so there is nothing to render in. A non-default brand is rendered instead,
 * which is the theming that does exist. See `/workspace/chyby.md`.
 */
import { render } from "@testing-library/react";
import type { RenderResult } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  createRootRoute,
  createRoute,
  createRouter,
  Outlet,
  RouterProvider,
} from "@tanstack/react-router";
import { I18nextProvider } from "react-i18next";
import axe from "axe-core";
import { expect, vi } from "vitest";
import type { ReactElement } from "react";
import i18n from "../src/i18n";
import { BrandingProvider } from "../src/branding";
import { resetDocumentTitle } from "../src/documentTitle";

/** The four languages the Portal ships; a string must exist in all of them (UI-11). */
export const LOCALES = ["en", "sk", "cs", "de"] as const;

/**
 * A brand no default holds, so a page that paints a colour by hand instead of through a token
 * is visible: the tokens move, a hard-coded `#1d4ed8` does not.
 */
export const OTHER_BRAND = {
  instanceName: "Helsinki Region Context",
  shortName: "Helsinki",
  colours: {
    primary: "#7c3aed",
    secondary: "#0f766e",
    accent: "#f59e0b",
    background: "#ffffff",
    text: "#0f172a",
  },
  primaryForeground: "#ffffff",
  languages: { default: "en", offered: ["en", "sk", "cs", "de"] },
};

export type Json = (body: unknown, status?: number) => Response;

/** A JSON (or `application/problem+json`) answer, as the API sends it. */
export const json: Json = (body, status = 200) =>
  new Response(JSON.stringify(body), {
    status,
    headers: {
      "Content-Type": status >= 400 ? "application/problem+json" : "application/json",
    },
  });

/** A `List` envelope, which is the shape every collection route answers with. */
export function list(items: unknown[]): unknown {
  return { apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items };
}

/** RFC 9457, as the Portal's API sends a refusal: the reason a person can act on. */
export function problem(status: number, detail: string, requestId = "req-7f3a91c4"): Response {
  return json(
    { type: "about:blank", title: "Error", status, detail, instance: `/req/${requestId}` },
    status,
  );
}

export interface PageHarness {
  /** How the page's own requests are answered; the path is what the test switches on. */
  answer: (url: URL, request: Request) => Response | undefined | Promise<Response | undefined>;
  /** The address the page is mounted at, so `PageHeader` reads the project out of it. */
  path?: string;
  /** The installation's branding; `OTHER_BRAND` proves the page paints through tokens. */
  branding?: unknown;
}

/**
 * The page under its providers at `path`, with `fetch` stubbed.
 *
 * `answer` sees every request and may return nothing, in which case the common answers below
 * are used: the signed-in person, the installation's branding, and an empty list for anything
 * else — so a page test writes only the requests its own page cares about.
 */
export function renderPage(element: ReactElement, harness: PageHarness): RenderResult {
  const { answer, path = "/projects/helsinki", branding = OTHER_BRAND } = harness;

  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const request =
        input instanceof Request ? input : new Request(new URL(String(input), window.location.origin), init);
      const url = new URL(request.url, window.location.origin);
      const own = await answer(url, request);
      if (own) return own;
      if (url.pathname.endsWith("/auth/me")) {
        return json({ subject: "b7c1e0f4", username: "jana.kovacova", name: "Jana Kováčová", roles: ["portal-editor"] });
      }
      if (url.pathname.endsWith("/branding")) return json(branding);
      return json(list([]));
    }),
  );

  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });

  const root = createRootRoute({ component: Outlet });
  const page = createRoute({
    getParentRoute: () => root,
    path: "$",
    component: () => element,
  });
  const router = createRouter({ routeTree: root.addChildren([page]) });

  window.history.pushState({}, "", path);
  resetDocumentTitle();

  return render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <BrandingProvider>
          <RouterProvider router={router as never} />
        </BrandingProvider>
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

/**
 * axe over the rendered page, with the rules that need a whole document off: a fragment mounted
 * by the test carries no `<html lang>` of its own and no landmark shell around it, and failing a
 * page for the harness around it says nothing about the page.
 */
export async function expectNoAxeViolations(
  container: HTMLElement,
  /**
   * Selectors to leave out, for a violation that is already a task of its own. Each one is named
   * with that task in the case that passes it, so the exclusion disappears with the fix rather
   * than outliving it.
   */
  exclude: string[] = [],
): Promise<void> {
  const results = await axe.run(exclude.length > 0 ? { include: [container], exclude } : container, {
    rules: {
      "html-has-lang": { enabled: false },
      "landmark-one-main": { enabled: false },
      region: { enabled: false },
      "page-has-heading-one": { enabled: false },
    },
  });
  const summary = results.violations
    .map((v) => `${v.id}: ${v.nodes.map((n) => n.html).join(" | ")}`)
    .join("\n");
  expect(results.violations, summary).toEqual([]);
}

/**
 * The headings of the page in document order, as a screen reader's heading list reads them.
 * One H1, and no level skipped on the way down (UI-16).
 */
export function expectHeadingOutline(container: HTMLElement): void {
  const levels = [...container.querySelectorAll<HTMLElement>("h1,h2,h3,h4,h5,h6")].map((h) =>
    Number(h.tagName.slice(1)),
  );
  expect(levels.filter((level) => level === 1), "exactly one H1 on the page").toHaveLength(1);
  expect(levels[0], "the H1 comes before every other heading").toBe(1);
  const skipped = levels.flatMap((level, index) =>
    index > 0 && level > levels[index - 1] + 1 ? [`h${levels[index - 1]} → h${level}`] : [],
  );
  expect(skipped, "no heading level is skipped").toEqual([]);
}

/**
 * Tab from the top of the page and collect what takes focus, in the order the keyboard reaches
 * it. Every control a mouse can use has to appear here (UI-16).
 */
export async function tabOrder(container: HTMLElement, steps = 40): Promise<HTMLElement[]> {
  const user = userEvent.setup();
  const seen: HTMLElement[] = [];
  (container.ownerDocument.body as HTMLElement).focus();
  for (let step = 0; step < steps; step += 1) {
    await user.tab();
    const active = container.ownerDocument.activeElement as HTMLElement | null;
    if (!active || active === container.ownerDocument.body) break;
    if (seen.includes(active)) break;
    seen.push(active);
  }
  return seen;
}

/** Run `check` once per language, restoring English afterwards (UI-11). */
export async function inEveryLocale(check: (locale: string) => Promise<void> | void): Promise<void> {
  try {
    for (const locale of LOCALES) {
      await i18n.changeLanguage(locale);
      await check(locale);
    }
  } finally {
    await i18n.changeLanguage("en");
  }
}

/**
 * A part — a card, a timeline, a chip bar — under i18n and a query client, with no router and
 * no branding around it. A part that navigates or reads the installation's name uses
 * {@link renderPage} instead; most do neither, and the lighter mount keeps a part's test about
 * the part.
 */
export function renderPart(
  element: ReactElement,
  harness: Partial<PageHarness> = {},
): RenderResult {
  const { answer } = harness;
  if (answer) {
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
        const request =
          input instanceof Request
            ? input
            : new Request(new URL(String(input), window.location.origin), init);
        const url = new URL(request.url, window.location.origin);
        return (await answer(url, request)) ?? json(list([]));
      }),
    );
  }
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>{element}</I18nextProvider>
    </QueryClientProvider>,
  );
}
