/**
 * An App opens inside the Portal (T-2689, AP-122, UI-44): the page frames the App's own address in
 * a sandbox without top navigation, offers it in a window of its own, and shows the state of an App
 * that has nothing to open instead of a frame. The App's own page and the catalog link here.
 */
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createRootRoute, createRoute, createRouter, Outlet, RouterProvider } from "@tanstack/react-router";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { expectDenied } from "./checks";
import { AuthProvider } from "../src/auth/AuthProvider";
import { BrandingProvider } from "../src/branding";
import {
  appFrameOrigin,
  AppOpenPage,
  FRAME_ANSWER_MS,
  OpenAppButton,
  SSO_CHECK_MS,
  SSO_CHECK_PATH,
  inLanguage,
  ssoAnswer,
} from "../src/pages/apps/AppOpenPage";

const PROJECT = "helsinki";
const COMMIT = "4f2a9c1e0b7d3a5f6c8e9d0a1b2c3d4e5f6a7b8c";

interface Stub {
  lifecycle?: string;
  build?: Record<string, unknown> | null;
  run?: Record<string, unknown> | null;
  status?: number;
  /** The origin the Portal serves Apps from, as `/api/v1/branding` names it. */
  appsOrigin?: string;
  /** The forge addresses `GET …/build` gives this person (T-3039); none by default. */
  links?: { repositoryUrl?: string | null; configurationUrl?: string | null };
}

function manifest(name: string, { lifecycle = "published", build = { commit: COMMIT } }: Stub) {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "App",
    metadata: { name, namespace: PROJECT, title: { en: "City bikes", sk: "Mestské bicykle" } },
    spec: { kind: "static", visibility: "internal", lifecycle, dataNeeds: [] },
    status: build ? { build } : {},
  };
}

function renderAt(path: string, stub: Stub = {}) {
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL) => {
      const url = new URL((input as Request).url, "http://localhost");
      const json = (body: unknown, status = 200) =>
        Promise.resolve(
          new Response(JSON.stringify(body), {
            status,
            headers: { "Content-Type": status >= 400 ? "application/problem+json" : "application/json" },
          }),
        );
      if (url.pathname.endsWith("/branding")) return json({ appsOrigin: stub.appsOrigin ?? null });
      if (url.pathname.endsWith("/auth/me")) return json({ subject: "s1", username: "steward", roles: [] });
      const build = /\/apps\/([^/]+)\/build$/.exec(url.pathname);
      if (build) {
        return json({
          repositoryUrl: null,
          configurationUrl: null,
          ...stub.links,
          packageUrl: null,
          run: stub.run ?? null,
          rebuild: { allowed: false },
        });
      }
      const app = /\/apps\/([^/]+)$/.exec(url.pathname);
      if (app) {
        if (stub.status) return json({ title: "Not Found", status: stub.status, detail: "app 'x' not found" }, stub.status);
        return json(manifest(app[1], stub));
      }
      return json({ items: [] });
    }),
  );
  const root = createRootRoute({ component: Outlet });
  const open = createRoute({
    getParentRoute: () => root,
    path: "/projects/$project/$plural/$name/open",
    component: function Open() {
      const { project, name } = open.useParams();
      return <AppOpenPage project={project} name={name} />;
    },
  });
  const detail = createRoute({
    getParentRoute: () => root,
    path: "/projects/$project/$plural/$name",
    component: function Detail() {
      const { project, name } = detail.useParams();
      return <OpenAppButton project={project} name={name} />;
    },
  });
  const router = createRouter({ routeTree: root.addChildren([open, detail]) });
  window.history.pushState({}, "", path);
  return render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>
        <AuthProvider>
          <BrandingProvider>
            <RouterProvider router={router} />
          </BrandingProvider>
        </AuthProvider>
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

const requestFullscreen = HTMLElement.prototype.requestFullscreen;
const exitFullscreen = document.exitFullscreen;

describe("AppOpenPage", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
    // What a full-screen case stubbed goes back to jsdom's own.
    HTMLElement.prototype.requestFullscreen = requestFullscreen;
    document.exitFullscreen = exitFullscreen;
    Reflect.deleteProperty(document, "fullscreenEnabled");
    Reflect.deleteProperty(document, "fullscreenElement");
  });

  it("frames the App's own address in a sandbox without top navigation", async () => {
    renderAt(`/projects/${PROJECT}/apps/city-bikes/open`);
    const frame = await screen.findByTitle("City bikes, the application");
    expect(frame.tagName).toBe("IFRAME");
    expect(frame.getAttribute("src")).toBe("/apps/city-bikes/?lang=en");
    expect(frame.getAttribute("sandbox")).toBe(
      "allow-scripts allow-forms allow-popups allow-downloads",
    );
    expect(frame.getAttribute("sandbox")).not.toContain("allow-top-navigation");
    expect(frame.getAttribute("referrerpolicy")).toBe("no-referrer");
    expect(screen.getByRole("heading", { level: 1, name: "City bikes" })).toBeTruthy();
  });

  // T-2908: a slim bar of the App's name and its actions, and the frame takes everything else.
  it("gives the frame all the room under a slim bar with the name and the actions alone", async () => {
    renderAt(`/projects/${PROJECT}/apps/city-bikes/open`);
    const frame = await screen.findByTitle("City bikes, the application");
    // The commit is developer information, on the App's details page, not a line of this one.
    expect(screen.queryByText(/4f2a9c1/)).toBeNull();
    expect(screen.queryByText(en.apps.openPage.lead)).toBeNull();
    for (const name of [en.apps.openPage.details, en.apps.openPage.fullscreen, en.apps.openPage.signInAgain]) {
      expect(screen.getByRole("button", { name })).toBeTruthy();
    }
    expect(screen.getByRole("link", { name: new RegExp(en.apps.openPage.newWindow) })).toBeTruthy();
    // No fixed height anywhere: the frame and its area grow into what the page has, and the page
    // is the Shell's viewport under the header (its `fill`).
    const area = screen.getByTestId("app-frame-area");
    expect(area.contains(frame)).toBe(true);
    for (const element of [area, frame]) {
      expect(element.className).toMatch(/\bflex-1\b/);
      expect(element.className).not.toMatch(/\b(min-)?h-(?!0\b)[\w[\]]+/);
      // No padding, rounding or border that would cage the App in a card; `border-0` takes the
      // iframe's own default border away.
      expect(element.className).not.toMatch(/\b(p|px|py)-[1-9]|\brounded\b|\brounded-|\bborder(?!-0\b)/);
    }
  });

  it("puts the frame in full screen and back, and follows an Escape the browser handles", async () => {
    const user = userEvent.setup();
    let current: Element | null = null;
    Object.defineProperty(document, "fullscreenEnabled", { configurable: true, get: () => true });
    Object.defineProperty(document, "fullscreenElement", { configurable: true, get: () => current });
    const enter = vi.fn(async () => {
      current = screen.getByTestId("app-frame-area");
      document.dispatchEvent(new Event("fullscreenchange"));
    });
    const leave = vi.fn(async () => {
      current = null;
      document.dispatchEvent(new Event("fullscreenchange"));
    });
    HTMLElement.prototype.requestFullscreen = enter;
    document.exitFullscreen = leave;
    renderAt(`/projects/${PROJECT}/apps/city-bikes/open`);
    await user.click(await screen.findByRole("button", { name: en.apps.openPage.fullscreen }));
    expect(enter).toHaveBeenCalledTimes(1);
    expect(enter.mock.contexts[0]).toBe(screen.getByTestId("app-frame-area"));
    const exit = await screen.findByRole("button", { name: en.apps.openPage.fullscreenExit });
    expect(exit.getAttribute("aria-pressed")).toBe("true");
    await user.click(exit);
    expect(leave).toHaveBeenCalledTimes(1);
    expect((await screen.findByRole("button", { name: en.apps.openPage.fullscreen })).getAttribute("aria-pressed")).toBe("false");
    // Escape is the browser's: the page learns it from the document's event alone.
    await user.click(screen.getByRole("button", { name: en.apps.openPage.fullscreen }));
    current = null;
    document.dispatchEvent(new Event("fullscreenchange"));
    expect((await screen.findByRole("button", { name: en.apps.openPage.fullscreen })).getAttribute("aria-pressed")).toBe("false");
  });

  it("keeps full screen in place, disabled with the reason, where the browser offers none", async () => {
    Object.defineProperty(document, "fullscreenEnabled", { configurable: true, get: () => false });
    renderAt(`/projects/${PROJECT}/apps/city-bikes/open`);
    expectDenied(await screen.findByRole("button", { name: en.apps.openPage.fullscreen }), en.apps.openPage.fullscreenUnavailable);
  });

  it("says so when the browser refuses full screen", async () => {
    const user = userEvent.setup();
    Object.defineProperty(document, "fullscreenEnabled", { configurable: true, get: () => true });
    Object.defineProperty(document, "fullscreenElement", { configurable: true, get: () => null });
    HTMLElement.prototype.requestFullscreen = vi.fn(async () => {
      throw new TypeError("Permissions check failed");
    });
    renderAt(`/projects/${PROJECT}/apps/city-bikes/open`);
    await user.click(await screen.findByRole("button", { name: en.apps.openPage.fullscreen }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Permissions check failed");
  });

  it("frames an App on the apps origin with its own origin, never the Portal's (T-2840)", async () => {
    renderAt(`/projects/${PROJECT}/apps/city-bikes/open`, { appsOrigin: "https://example.org" });
    await waitFor(() =>
      expect(screen.getByTitle("City bikes, the application").getAttribute("src")).toBe(
        "https://city-bikes.apps.example.org/?lang=en",
      ),
    );
    const frame = screen.getByTitle("City bikes, the application");
    expect(frame.getAttribute("sandbox")).toBe(
      "allow-scripts allow-forms allow-popups allow-downloads allow-same-origin",
    );
    expect(screen.getByRole("link", { name: /new window/i }).getAttribute("href")).toBe(
      "https://city-bikes.apps.example.org/?lang=en",
    );
  });

  it("opens the same address in a new window with no opener", async () => {
    renderAt(`/projects/${PROJECT}/apps/city-bikes/open`);
    const link = await screen.findByRole("link", { name: /Open in new window/ });
    expect(link.getAttribute("href")).toBe("/apps/city-bikes/?lang=en");
    expect(link.getAttribute("target")).toBe("_blank");
    expect(link.getAttribute("rel")).toBe("noopener noreferrer");
  });

  it("links the App's whole source and the project's configuration in new tabs when the API gives them (T-3039)", async () => {
    renderAt(`/projects/${PROJECT}/apps/city-bikes/open`, {
      links: {
        repositoryUrl: "https://forge.example/user/oauth2/keycloak?redirect_to=%2Fjc-apps%2Fhelsinki_city-bikes",
        configurationUrl: "https://forge.example/user/oauth2/keycloak?redirect_to=%2Fjc%2Fhelsinki",
      },
    });
    const source = await screen.findByRole("link", { name: new RegExp(`^${en.apps.sourceCode}`) });
    expect(source).toHaveAttribute("href", "https://forge.example/user/oauth2/keycloak?redirect_to=%2Fjc-apps%2Fhelsinki_city-bikes");
    expect(source).toHaveAttribute("target", "_blank");
    const configuration = screen.getByRole("link", { name: new RegExp(`^${en.apps.projectConfiguration}`) });
    expect(configuration).toHaveAttribute("href", "https://forge.example/user/oauth2/keycloak?redirect_to=%2Fjc%2Fhelsinki");
    expect(configuration).toHaveAttribute("rel", "noopener noreferrer");
  });

  it("offers no forge link the API left out, nor one of another scheme (T-3039)", async () => {
    renderAt(`/projects/${PROJECT}/apps/city-bikes/open`, {
      links: { repositoryUrl: "javascript:alert(1)", configurationUrl: "https://forge.example/jc/helsinki" },
    });
    // The configuration link arriving is the build's answer read; the source link never comes.
    await screen.findByRole("link", { name: new RegExp(`^${en.apps.projectConfiguration}`) });
    expect(screen.queryByText(en.apps.sourceCode)).toBeNull();
  });

  it("escapes a name into the address instead of leaving the apps path", async () => {
    renderAt(`/projects/${PROJECT}/apps/${encodeURIComponent("a/../../x")}/open`);
    const frame = await screen.findByTitle("City bikes, the application");
    expect(frame.getAttribute("src")).toBe("/apps/a%2F..%2F..%2Fx/?lang=en");
  });

  it("shows a retired App's state and no frame", async () => {
    renderAt(`/projects/${PROJECT}/apps/old-map/open`, { lifecycle: "retired" });
    expect(await screen.findByText("This application is retired")).toBeTruthy();
    expect(document.querySelector("iframe")).toBeNull();
    expect(screen.queryByRole("link", { name: /Open in new window/ })).toBeNull();
  });

  it("shows a published App whose build failed as failed, not as a frame answering 404", async () => {
    renderAt(`/projects/${PROJECT}/apps/city-bikes/open`, {
      build: null,
      run: { status: "completed", conclusion: "failure", commit: COMMIT, url: "https://git.example/run/1" },
    });
    expect(await screen.findByText(/The build failed/)).toBeTruthy();
    expect(document.querySelector("iframe")).toBeNull();
  });

  it("says the API's own sentence for an App that is not there", async () => {
    renderAt(`/projects/${PROJECT}/apps/nothing/open`, { status: 404 });
    expect(await screen.findByText(/app 'x' not found/)).toBeTruthy();
    expect(document.querySelector("iframe")).toBeNull();
  });

  it("takes the whole window through the login and back to this page when asked", async () => {
    const assign = vi.fn();
    vi.stubGlobal("location", { ...window.location, assign, pathname: `/projects/${PROJECT}/apps/city-bikes/open`, search: "" });
    renderAt(`/projects/${PROJECT}/apps/city-bikes/open`);
    await userEvent.click(await screen.findByRole("button", { name: "Sign in again" }));
    expect(assign).toHaveBeenCalledWith(
      `/api/v1/auth/login?redirect_to=${encodeURIComponent(`/projects/${PROJECT}/apps/city-bikes/open`)}&lang=${i18n.language.split("-")[0]}`,
    );
  });
});

/**
 * T-2941, T-3034: a frame the browser refused (the realm's sign-in form may not be framed) is a
 * white box that fires `load` all the same, and so is an App that renders without saying so. The
 * App says it is up with `{kind: "jc-ready"}`; when it stays silent, the page asks the realm
 * silently in a hidden frame of its own origin, and offers the sign-in only when the realm says
 * the person must sign in.
 */
describe("AppOpenPage when the App stays silent", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  async function loadedFrame(stub: Stub = {}) {
    renderAt(`/projects/${PROJECT}/apps/city-bikes/open`, stub);
    const frame = (await screen.findByTitle("City bikes, the application")) as HTMLIFrameElement;
    if (stub.appsOrigin) {
      await waitFor(() => expect(frame.getAttribute("src")).toMatch(/^https:/));
    }
    vi.useFakeTimers();
    fireEvent.load(frame);
    return frame;
  }
  const say = (source: MessageEventSource | null, origin: string, data: unknown = { kind: "jc-ready" }) =>
    act(() => {
      window.dispatchEvent(new MessageEvent("message", { data, origin, source }));
    });
  const wait = (ms = FRAME_ANSWER_MS) =>
    act(() => {
      vi.advanceTimersByTime(ms);
    });
  const prompt = () => screen.queryByText(en.apps.openPage.signedOutTitle);
  const check = () => screen.queryByTestId("app-sso-check") as HTMLIFrameElement | null;
  /** The check frame lands on `href`, as the browser would after the realm's redirect. */
  const land = (href: string) => {
    const frame = check();
    if (!frame) throw new Error("no sign-in check is running");
    Object.defineProperty(frame, "contentWindow", { configurable: true, value: { location: { href } } });
    fireEvent.load(frame);
  };
  const done = `${window.location.origin}/api/v1/auth/sso-check/done`;

  it("asks the realm silently, and offers the sign-in in the top window only when it says so", async () => {
    const assign = vi.fn();
    vi.stubGlobal("location", { ...window.location, assign, pathname: `/projects/${PROJECT}/apps/city-bikes/open`, search: "" });
    const frame = await loadedFrame();
    wait(FRAME_ANSWER_MS - 1);
    expect(check()).toBeNull();
    wait(1);
    const hidden = check();
    expect(hidden?.getAttribute("src")).toBe(SSO_CHECK_PATH);
    expect(hidden?.hidden).toBe(true);
    // Its own origin and no script: the page reads the address, the frame runs nothing.
    expect(hidden?.getAttribute("sandbox")).toBe("allow-same-origin");
    expect(prompt()).toBeNull();
    land(`${done}#error=login_required&iss=https%3A%2F%2Fidm.example`);
    expect(check()).toBeNull();
    const status = screen.getByRole("status");
    expect(status).toHaveTextContent(en.apps.openPage.signedOutTitle);
    expect(status).toHaveTextContent(en.apps.openPage.signedOutBody);
    // One "Open in new window" on the page, the bar's: a second would make its name ambiguous.
    expect(screen.getAllByRole("link", { name: new RegExp(en.apps.openPage.newWindow) })).toHaveLength(1);
    expect(document.body.contains(frame)).toBe(true);
    fireEvent.click(within(status).getByRole("button", { name: en.apps.openPage.signInAgain }));
    expect(assign).toHaveBeenCalledWith(
      `/api/v1/auth/login?redirect_to=${encodeURIComponent(`/projects/${PROJECT}/apps/city-bikes/open`)}&lang=${i18n.language.split("-")[0]}`,
    );
    fireEvent.click(within(status).getByRole("button", { name: en.apps.openPage.signedOutDismiss }));
    expect(prompt()).toBeNull();
  });

  it("stays quiet for a silent App while the realm session lives", async () => {
    await loadedFrame();
    wait();
    land(`${done}#code=abc.def&iss=https%3A%2F%2Fidm.example`);
    expect(check()).toBeNull();
    wait(SSO_CHECK_MS * 2);
    expect(prompt()).toBeNull();
  });

  it("claims nothing on an answer it cannot read or a check that never lands", async () => {
    await loadedFrame();
    wait();
    // A load on the realm's side is another origin: not an answer, the check goes on.
    land("https://idm.example/realms/dev/protocol/openid-connect/auth");
    expect(check()).not.toBeNull();
    land(`${done}#error=invalid_request`);
    expect(check()).toBeNull();
    expect(prompt()).toBeNull();

    // A new load of the App starts over; this time the realm never answers.
    fireEvent.load(screen.getByTitle("City bikes, the application"));
    wait();
    expect(check()).not.toBeNull();
    wait(SSO_CHECK_MS);
    expect(check()).toBeNull();
    expect(prompt()).toBeNull();
  });

  it("asks nothing of an App that answers from its own frame, before or after its load", async () => {
    const frame = await loadedFrame();
    // Same host as the Portal: the sandbox gives the App the opaque origin.
    say(frame.contentWindow, "null");
    wait();
    expect(check()).toBeNull();
    // A later load of the same visit (the App's own navigation) does not ask again.
    fireEvent.load(frame);
    wait();
    expect(check()).toBeNull();
    expect(prompt()).toBeNull();
  });

  it("ignores an answer from another origin, another window or of another kind", async () => {
    const frame = await loadedFrame({ appsOrigin: "https://example.org" });
    say(frame.contentWindow, "null");
    say(frame.contentWindow, "https://evil.example");
    say(window, "https://city-bikes.apps.example.org");
    say(frame.contentWindow, "https://city-bikes.apps.example.org", { kind: "jc-resize" });
    say(frame.contentWindow, "https://city-bikes.apps.example.org", "jc-ready");
    wait();
    expect(check()).not.toBeNull();
    // The App's own origin answering ends the check before the realm has said anything.
    say(frame.contentWindow, "https://city-bikes.apps.example.org");
    expect(check()).toBeNull();
    expect(prompt()).toBeNull();
  });

  it("reads only the realm's answer on the Portal's own check page", () => {
    expect(ssoAnswer(undefined)).toBeNull();
    expect(ssoAnswer("not a url")).toBeNull();
    expect(ssoAnswer(`https://evil.example/api/v1/auth/sso-check/done#error=login_required`)).toBeNull();
    expect(ssoAnswer(`${window.location.origin}/elsewhere#error=login_required`)).toBeNull();
    for (const error of ["login_required", "interaction_required", "consent_required"]) {
      expect(ssoAnswer(`${done}#error=${error}`)).toBe("signedOut");
    }
    expect(ssoAnswer(`${done}#code=x`)).toBe("signedIn");
    expect(ssoAnswer(`${done}#error=invalid_request&code=x`)).toBe("unknown");
    expect(ssoAnswer(done)).toBe("unknown");
  });

  it("expects the App's own origin on another host and the opaque one on the Portal's", () => {
    expect(appFrameOrigin("/apps/city-bikes/", "https://portal.example")).toBe("null");
    expect(appFrameOrigin("https://portal.example/apps/x/", "https://portal.example")).toBe("null");
    expect(appFrameOrigin("https://x.apps.example.org/", "https://portal.example")).toBe("https://x.apps.example.org");
  });
});

describe("OpenAppButton", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("links a served App to its page inside the Portal", async () => {
    renderAt(`/projects/${PROJECT}/apps/city-bikes`);
    const link = await screen.findByRole("link", { name: "Open app" });
    expect(link.getAttribute("href")).toBe(`/projects/${PROJECT}/apps/city-bikes/open`);
    expect(link.getAttribute("target")).toBeNull();
  });

  it("says why an App that serves nothing cannot open", async () => {
    renderAt(`/projects/${PROJECT}/apps/city-bikes`, { lifecycle: "draft", build: null });
    const button = await screen.findByRole("button", { name: "Open app" });
    await waitFor(() => expect(button.getAttribute("aria-disabled") ?? button.hasAttribute("disabled")).toBeTruthy());
    expect(screen.queryByRole("link", { name: "Open app" })).toBeNull();
  });

  it("offers nothing on a retired App", async () => {
    const view = renderAt(`/projects/${PROJECT}/apps/old-map`, { lifecycle: "retired" });
    const read = () =>
      vi.mocked(fetch).mock.calls.some(([input]) => (input as Request).url.endsWith(`/apps/old-map`));
    await waitFor(() => expect(read()).toBe(true));
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(view.container.textContent).toBe("");
    expect(screen.queryByRole("button", { name: "Open app" })).toBeNull();
  });
});

describe("the framed App in the Portal's language (T-4264, T-4160 #39)", () => {
  it("adds the language the SDK reads first, and keeps the address otherwise as it was", () => {
    expect(inLanguage("/apps/bbsk-data/", "sk")).toBe("/apps/bbsk-data/?lang=sk");
    expect(inLanguage("https://bbsk-data.apps.example.org/", "sk-SK")).toBe("https://bbsk-data.apps.example.org/?lang=sk");
    expect(inLanguage("/apps/x/?page=2#top", "cs")).toBe("/apps/x/?page=2&lang=cs#top");
    expect(inLanguage("/apps/x/", undefined)).toBe("/apps/x/");
    expect(inLanguage("/apps/x/", "x&y=1")).toBe("/apps/x/");
  });
});

