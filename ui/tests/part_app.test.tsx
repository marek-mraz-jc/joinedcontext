/**
 * T-1797: the root of the Portal against the UI contract (UI-15, UI-16, UI-48).
 *
 * `App` is three things around one router: the branding, the session, and the one moment before
 * either is known. That moment is all the markup it owns, and it is the part that can go wrong
 * quietly — an anonymous visitor let through a route guard because the tree mounted before the
 * session settled, or a blank screen that says nothing while the answer is on its way.
 */
// covers (T-2137, the module gate in gate_modules.test.ts): the cases in this file drive
// src/api/projects.ts through the page they belong to; each was confirmed by
// making the module throw and watching this file go red.
import { render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n, { SUPPORTED_LOCALES } from "../src/i18n";
import { App } from "../src/App";
import { NEUTRAL_BRANDING } from "../src/branding";
import { expectNoRawKeys, expectNoViolations } from "./checks";

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  roles: ["portal-viewer"],
};

/** Every answer the shell needs, with the session held back until a test releases it. */
function stub(options: { session?: "held" | "none" | "in" } = {}) {
  const held = options.session === "held";
  let release: (() => void) | undefined;
  const waiting = new Promise<void>((resolve) => {
    release = resolve;
  });
  const json = (body: unknown, status = 200) =>
    new Response(JSON.stringify(body), {
      status,
      headers: { "Content-Type": status >= 400 ? "application/problem+json" : "application/json" },
    });

  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
      const path = new URL(url, window.location.origin).pathname;
      if (path.endsWith("/branding")) {
        return json(NEUTRAL_BRANDING);
      }
      if (path === "/api/v1/catalogue/readable") {
        return json({ datasets: [], unanswered: 0 });
      }
      if (path.endsWith("/auth/me")) {
        if (held) {
          await waiting;
        }
        return options.session === "in"
          ? json(IDENTITY)
          : json({ status: 401, title: "Unauthorized" }, 401);
      }
      return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
    }),
  );
  return { release: () => release?.() };
}

function show() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <App />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

describe("the Portal's root against the UI contract", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/");
    localStorage.clear();
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("says it is loading while the session is unknown, and mounts no route yet", async () => {
    const { release } = stub({ session: "held" });
    const { container } = show();

    // Announced, not a blank screen: `role="status"` is how a screen reader is told to wait.
    const status = await screen.findByRole("status");
    expect(status).toHaveTextContent(i18n.t("app.loading"));
    expect(screen.queryByRole("navigation")).toBeNull();
    await expectNoViolations(container);

    release();
    await waitFor(() => expect(screen.queryByRole("status")).toBeNull());
  });

  it("mounts the router once the answer is in, and holds the anonymous visitor at the door", async () => {
    stub({ session: "none" });
    show();

    // The route guards read the session in `beforeLoad`, so the answer has to be there first:
    // an anonymous visitor reaches the login page, never a project's page.
    await waitFor(() => expect(screen.queryByRole("status")).toBeNull());
    expect(window.location.pathname).not.toContain("/projects/");
  });

  it("puts the signed-in shell up, named after the installation", async () => {
    stub({ session: "in" });
    const { container } = show();

    await waitFor(() => expect(screen.queryByRole("status")).toBeNull());
    // The shell is what the router mounts into: the installation's name in the header, and the
    // route's own content below it. What each route puts there is that page's own task.
    // A member (no project) is sent on to the catalogue and the shell mounts again: read the live DOM.
    await waitFor(() => expect(screen.getByText(NEUTRAL_BRANDING.shortName!)).toBeInTheDocument());
    expect(window.location.pathname).toBe("/catalogue");
    expect(container.querySelector("main, [role=main]")).not.toBeNull();
  });

  it.each(SUPPORTED_LOCALES)("says the one word it owns in %s", async (locale) => {
    await i18n.changeLanguage(locale);
    const { release } = stub({ session: "held" });
    const { container } = show();

    expect(await screen.findByRole("status")).toHaveTextContent(i18n.t("app.loading"));
    expectNoRawKeys(container);
    release();
  });
});
