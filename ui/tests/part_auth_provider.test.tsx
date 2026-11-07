/**
 * T-1800: the session the whole Portal reads, against the UI contract (UI-15, UI-16, UI-46).
 *
 * The provider renders no markup, so its part of the contract is what it publishes: nothing is
 * authenticated until the answer is in, anything other than a live session is anonymous — a
 * transport error must not open a door — and a logout empties this browser whatever the network
 * then does, because the next person at the same screen must not find the last one's projects.
 */
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AuthProvider, useAuth } from "../src/auth/AuthProvider";
import { expectNoViolations } from "./checks";
import i18n from "../src/i18n";

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  roles: ["portal-steward"],
  front: "portal",
};

/** What a component around the provider sees, as plain text so a test can read it. */
function Reader() {
  const auth = useAuth();
  return (
    <output aria-label="session">
      {auth.status} {auth.identity?.username ?? "nobody"} steward:
      {String(auth.hasRole("portal-steward"))}
    </output>
  );
}

function show() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <AuthProvider>
        <Reader />
      </AuthProvider>
    </QueryClientProvider>,
  );
}

const session = () => screen.getByLabelText("session").textContent?.replace(/\s+/g, " ").trim();

function answer(handler: (url: string, init?: RequestInit) => Response | Promise<Response>) {
  const calls: { url: string; init?: RequestInit }[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
      const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
      calls.push({ url, init: init ?? (input instanceof Request ? { method: input.method } : undefined) });
      return Promise.resolve(handler(url, init));
    }),
  );
  return calls;
}

const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });

describe("the auth provider against the UI contract", () => {
  const assign = vi.fn();

  beforeEach(() => {
    vi.stubGlobal("location", { ...window.location, assign, pathname: "/projects/helsinki", search: "" });
    assign.mockClear();
    localStorage.clear();
    sessionStorage.clear();
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("is loading until the answer is in, and nobody is authenticated meanwhile", async () => {
    answer(() => json(IDENTITY));
    show();

    expect(session()).toBe("loading nobody steward:false");
    await waitFor(() => expect(session()).toBe("authenticated jana.kovacova steward:true"));
  });

  it("is anonymous when there is no session, and draws nothing of its own", async () => {
    answer(() => json({ status: 401, title: "Unauthorized" }, 401));
    const { container } = show();

    await waitFor(() => expect(session()).toBe("anonymous nobody steward:false"));
    // One child, the reader: the provider adds no element, no landmark, no announcement.
    expect(container.children).toHaveLength(1);
    await expectNoViolations(container);
  });

  it("is anonymous when the session cannot be read at all: an error opens no door", async () => {
    answer(() => json({ status: 500, title: "Server Error" }, 500));
    show();

    await waitFor(() => expect(session()).toBe("anonymous nobody steward:false"));
  });

  it("empties this browser on the way out, before the request that may fail", async () => {
    const calls = answer((url) =>
      url.includes("/auth/logout")
        ? Promise.reject(new Error("the network went away"))
        : json(IDENTITY),
    );
    localStorage.setItem("jc-project", "helsinki");
    const user = userEvent.setup();

    function Out() {
      const auth = useAuth();
      return (
        <button type="button" onClick={() => void auth.signOut().catch(() => {})}>
          out
        </button>
      );
    }
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={client}>
        <AuthProvider>
          <Out />
        </AuthProvider>
      </QueryClientProvider>,
    );

    await user.click(screen.getByRole("button", { name: "out" }));

    // UI-46: the previous person's state is gone even though the logout request failed.
    await waitFor(() => expect(localStorage.getItem("jc-project")).toBeNull());
    expect(calls.some((call) => call.url.includes("/auth/logout"))).toBe(true);
  });

  it("asks the server for the login flow and says where to come back to", async () => {
    answer(() => json({ status: 401, title: "Unauthorized" }, 401));
    const user = userEvent.setup();

    function In() {
      const auth = useAuth();
      return (
        <button type="button" onClick={() => auth.signIn()}>
          in
        </button>
      );
    }
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={client}>
        <AuthProvider>
          <In />
        </AuthProvider>
      </QueryClientProvider>,
    );

    // The realm's page in the Portal's language (T-4199): a Slovak Portal asks for Slovak.
    const before = i18n.language;
    await i18n.changeLanguage("sk");
    try {
      await user.click(screen.getByRole("button", { name: "in" }));
    } finally {
      await i18n.changeLanguage(before);
    }
    expect(assign).toHaveBeenCalledWith(
      "/api/v1/auth/login?redirect_to=%2Fprojects%2Fhelsinki&lang=sk",
    );
  });
});

describe("on a public form's page (EP-102)", () => {
  afterEach(() => {
    window.history.replaceState(null, "", "/");
    vi.unstubAllGlobals();
  });

  it("asks nobody who is signed in and is anonymous at once", async () => {
    window.history.replaceState(null, "", "/f/zt4qm7ge2xdv6ksb3ncf5arw2y?test=1");
    const calls = answer(() => new Response(JSON.stringify(IDENTITY), { status: 200, headers: { "Content-Type": "application/json" } }));
    show();
    expect(session()).toBe("anonymous nobody steward:false");
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(calls).toHaveLength(0);
  });
});
