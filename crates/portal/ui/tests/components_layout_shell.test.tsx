/**
 * T-7585: the shell's remaining paths: the Help menu's feedback item, signing out from the
 * account menu (the menu goes with the identity), and a page that fills the shell stopping with
 * its reference and a way back.
 */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";

const noIdentity = vi.hoisted(() => ({ on: false }));
vi.mock("../src/auth/AuthProvider", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../src/auth/AuthProvider")>();
  return {
    ...actual,
    useAuth: () => {
      const auth = actual.useAuth();
      return noIdentity.on ? { ...auth, identity: null } : auth;
    },
  };
});

vi.mock("../src/routes/ActivityPage", () => ({
  ActivityPage: () => {
    throw new Error("the activity page broke while drawing");
  },
}));

vi.mock("../src/pages/apps/AppOpenPage", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../src/pages/apps/AppOpenPage")>();
  return {
    ...actual,
    AppOpenPage: () => {
      throw new Error("the page broke while drawing");
    },
  };
});

const IDENTITY = { subject: "b7c1e0f4", username: "jana.kovacova", name: "Jana Kováčová", roles: ["portal-viewer"] };
const LIST = { apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] };
const PROJECTS = { ...LIST, items: [{ name: "helsinki", sample: false }] };

let requests: string[];
const realLocation = Object.getOwnPropertyDescriptor(window, "location");

beforeEach(async () => {
  await i18n.changeLanguage("en");
  requests = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
      const request = input instanceof Request ? input : new Request(new URL(String(input), "http://localhost"), init);
      const url = new URL(request.url);
      requests.push(`${request.method} ${url.pathname}`);
      const body = url.pathname.endsWith("/auth/me")
        ? IDENTITY
        : url.pathname.endsWith("/auth/logout")
          ? {}
          : url.pathname === "/api/v1/projects"
            ? PROJECTS
            : LIST;
      return Promise.resolve(
        new Response(JSON.stringify(body), { status: 200, headers: { "Content-Type": "application/json" } }),
      );
    }),
  );
});

afterEach(() => {
  vi.unstubAllGlobals();
  if (realLocation) Object.defineProperty(window, "location", realLocation);
  noIdentity.on = false;
  window.history.pushState({}, "", "/");
});

function show(path: string) {
  window.history.pushState({}, "", path);
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>
        <App />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

describe("the shell header", () => {
  it("opens the feedback dialog from the Help menu", async () => {
    show("/projects/helsinki/spaces");
    await userEvent.click(await screen.findByRole("button", { name: /^Help/ }));
    await userEvent.click(await screen.findByRole("menuitem", { name: en.feedback.button }));
    expect(await screen.findByRole("dialog", { name: en.feedback.title })).toBeInTheDocument();
  });

  it("signs out through the account menu", async () => {
    show("/projects/helsinki/spaces");
    await userEvent.click(await screen.findByRole("button", { name: /Jana/ }));
    const assign = vi.fn();
    Object.defineProperty(window, "location", {
      configurable: true,
      value: { ...window.location, assign, origin: "http://localhost:3000", pathname: "/projects/helsinki/spaces" },
    });
    await userEvent.click(await screen.findByRole("menuitem", { name: en.auth.signOut }));
    await waitFor(() => expect(assign).toHaveBeenCalledWith("/"));
    expect(requests).toContain("POST /api/v1/auth/logout");
  });
});

describe("the shell without an identity", () => {
  it("draws no account menu", async () => {
    noIdentity.on = true;
    show("/projects/helsinki/spaces");
    expect(await screen.findByRole("navigation", { name: "Main navigation" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Signed in as/ })).toBeNull();
    expect(screen.getByRole("button", { name: /^Help/ })).toBeInTheDocument();
  });
});

describe("a page that fills the shell and breaks", () => {
  it("stops with a reference and a way back to the start", async () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    show("/projects/helsinki/apps/city-bikes/open");
    const alert = await screen.findByText(en.app.error.crashTitle);
    expect(alert).toBeInTheDocument();
    expect(screen.getByText(en.app.error.crashBody)).toBeInTheDocument();
    expect(screen.getByTestId("error-reference")).not.toBeEmptyDOMElement();
    expect(screen.getByRole("link", { name: en.app.error.crashHome })).toHaveAttribute(
      "href",
      expect.stringMatching(/^\/projects\/helsinki\//),
    );
    await userEvent.click(screen.getByRole("button", { name: en.app.error.retry }));
    // The page breaks again, so the notice is still what the reader sees.
    expect(await within(screen.getByRole("main")).findByText(en.app.error.crashTitle)).toBeInTheDocument();
  });
});

describe("a page inside the shell's column that breaks", () => {
  it("stops under the header with the same reference and the shell around it", async () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    show("/projects/helsinki/activity");
    expect(await screen.findByText(en.app.error.crashTitle)).toBeInTheDocument();
    expect(screen.getByRole("navigation", { name: "Main navigation" })).toBeInTheDocument();
    expect(screen.getByTestId("error-reference")).not.toBeEmptyDOMElement();
  });
});
