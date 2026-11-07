/**
 * UI-15, UI-16, UI-44 (T-2137): what a page shows while it waits, and when it could not be read.
 *
 * Both states used to be written by hand on every page and written smaller each time — a bare
 * "Loading…" on blank white, and a red line carrying one generic sentence, so "you may not read
 * this project" and "the store is away" read identically and neither offered anything to press.
 * These two components are the rule instead: the wait is announced once in the page's own words,
 * and a failure says what the API said plus the one thing worth doing about it.
 */
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { ApiError } from "../src/api/client";
import { PageFailed, PageLoading } from "../src/components/ui/PageState";
import type { AuthStatus } from "../src/auth/AuthProvider";
import { expectNoAxeViolations, inEveryLocale } from "./page_contract";

/** The session's status as the page sees it; `undefined` is a page outside any AuthProvider. */
const authStatus = vi.hoisted(() => ({ value: "authenticated" as AuthStatus | undefined }));
vi.mock("../src/auth/AuthProvider", () => ({ useAuthStatus: () => authStatus.value }));
beforeEach(() => {
  authStatus.value = "authenticated";
});

const show = (element: React.ReactElement) =>
  render(<I18nextProvider i18n={i18n}>{element}</I18nextProvider>);

describe("the page's waiting state", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("announces the wait once, in the page's own words", () => {
    const { container } = show(<PageLoading label="Reading the endpoints" />);
    const status = screen.getByRole("status");
    expect(status).toHaveAttribute("aria-busy", "true");
    expect(status).toHaveTextContent("Reading the endpoints");
    // The bars stand for the content and say nothing themselves.
    expect(container.querySelectorAll('[aria-hidden="true"]').length).toBeGreaterThan(1);
  });

  it("draws as many bars as the page asked for", () => {
    const { container } = show(<PageLoading label="Reading" lines={4} />);
    // One bar for the heading, then the page's own.
    expect(container.querySelectorAll('[aria-hidden="true"]')).toHaveLength(5);
    const single = render(
      <I18nextProvider i18n={i18n}>
        <PageLoading label="Reading" lines={1} />
      </I18nextProvider>,
    );
    expect(single.container.querySelectorAll('[aria-hidden="true"]')).toHaveLength(2);
  });

  it("has no axe violation", async () => {
    const { container } = show(<PageLoading label="Reading the endpoints" />);
    await expectNoAxeViolations(container);
  });
});

describe("the page's failed state", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("says what the API said, not a sentence of its own", () => {
    show(
      <PageFailed
        error={
          new ApiError(403, "Forbidden", {
            type: "about:blank",
            status: 403,
            title: "Forbidden",
            detail: "You may not read the project banskabystrica.",
          })
        }
      />,
    );
    expect(screen.getByText("You may not read the project banskabystrica.")).toBeInTheDocument();
    expect(screen.queryByText(en.app.error.generic)).not.toBeInTheDocument();
  });

  it("falls back to one plain sentence when the failure carries none", () => {
    show(<PageFailed error={new TypeError("Failed to fetch")} />);
    expect(screen.getByText(en.app.error.generic)).toBeInTheDocument();
    // Never the raw exception: "TypeError: Failed to fetch" is a line for a log, not for a page.
    expect(screen.queryByText(/TypeError/)).not.toBeInTheDocument();
  });

  it("prefers the page's own sentence where the page knows better", () => {
    show(
      <PageFailed error={new ApiError(404, "Not found", {
        type: "about:blank",
        status: 404,
        title: "Not found",
        detail: "no such run",
      })}>
        This run has expired and its workspace is gone.
      </PageFailed>,
    );
    expect(screen.getByText("This run has expired and its workspace is gone.")).toBeInTheDocument();
    expect(screen.queryByText("no such run")).not.toBeInTheDocument();
  });

  it("offers Retry only where asking again can help, and presses it", async () => {
    const retry = vi.fn();
    const { unmount } = show(<PageFailed error={new Error("gone")} onRetry={retry} />);
    const button = screen.getByRole("button", { name: en.app.error.retry });
    await userEvent.click(button);
    expect(retry).toHaveBeenCalledTimes(1);
    unmount();

    // A 404 or a refusal will not change, so the page offers nothing to press rather than a
    // button that does nothing.
    show(<PageFailed error={new ApiError(404, "Not found", {
          type: "about:blank",
          status: 404,
          title: "Not found",
          detail: "no such space",
        })} />);
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
  });

  // T-2834: the pages passed `onRetry` for every failure, so a 404 offered a Retry that could
  // only answer 404 again. The rule lives here now, for every caller.
  it("drops a page's Retry for a failure that answers the same the second time", () => {
    const retry = vi.fn();
    for (const status of [400, 403, 404, 409, 422]) {
      const problem = { type: "about:blank", status, title: "No", detail: `answered ${status}` };
      const { unmount } = show(<PageFailed error={new ApiError(status, "No", problem)} onRetry={retry} />);
      expect(screen.getByText(`answered ${status}`)).toBeInTheDocument();
      expect(screen.queryByRole("button", { name: en.app.error.retry }), String(status)).toBeNull();
      unmount();
    }
    const problem = { type: "about:blank", status: 503, title: "Away", detail: "the store is away" };
    show(<PageFailed error={new ApiError(503, "Away", problem)} onRetry={retry} />);
    expect(screen.getByRole("button", { name: en.app.error.retry })).toBeInTheDocument();
  });

  it("says Retry in every locale the Portal ships", async () => {
    const retry = vi.fn();
    await inEveryLocale(async (locale) => {
      const view = render(
        <I18nextProvider i18n={i18n}>
          <PageFailed error={new Error("gone")} onRetry={retry} />
        </I18nextProvider>,
      );
      const label = i18n.t("app.error.retry");
      expect(label, locale).not.toMatch(/^app\./);
      expect(screen.getByRole("button", { name: label })).toBeInTheDocument();
      view.unmount();
    });
  });

  const problem = (status: number, detail: string) => ({ type: "about:blank", status, title: "x", detail });

  it("T-2747: a refusal says who gives access", () => {
    show(<PageFailed error={new ApiError(403, "Forbidden", problem(403, "You may not read the project helsinki."))} />);
    expect(screen.getByText("You may not read the project helsinki.")).toBeInTheDocument();
    expect(screen.getByText(en.app.error.forbiddenHint)).toBeInTheDocument();
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
  });

  it("T-2747: a missing object offers the page's way back to its list", () => {
    show(
      <PageFailed
        error={new ApiError(404, "Not found", problem(404, "There is no endpoint bikes."))}
        back={<a href="/projects/helsinki/endpoints">Back to the endpoints</a>}
      />,
    );
    expect(screen.getByRole("link", { name: "Back to the endpoints" })).toHaveAttribute("href", "/projects/helsinki/endpoints");
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
  });

  it("T-2747: a server failure offers Retry and its reference, never the stack", async () => {
    const retry = vi.fn();
    show(
      <PageFailed
        error={new ApiError(502, "Bad Gateway", problem(502, "The configuration store did not answer."), "3f2c1a9e6b1d4c3e")}
        onRetry={retry}
      />,
    );
    expect(screen.getByText("The configuration store did not answer.")).toBeInTheDocument();
    expect(screen.getByText("3f2c1a9e6b1d4c3e")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: en.app.error.retry }));
    expect(retry).toHaveBeenCalledTimes(1);
  });

  it("T-4165: a server failure with no sentence of its own says what failed in words, not the HTTP status text", () => {
    // T-4163 audit: Policies, Context Spaces and Organization showed only 'Internal Server Error'.
    for (const error of [
      new ApiError(500, "Internal Server Error"),
      new ApiError(500, "Internal Server Error", { type: "about:blank", status: 500, title: "Internal Server Error" }),
      new ApiError(503, "Service Unavailable"),
    ]) {
      const { unmount } = show(<PageFailed error={error} onRetry={vi.fn()} />);
      expect(screen.getByText(en.app.error.server)).toBeInTheDocument();
      expect(screen.queryByText(/Internal Server Error|Service Unavailable/)).not.toBeInTheDocument();
      expect(screen.getByRole("button", { name: en.app.error.retry })).toBeInTheDocument();
      unmount();
    }
  });

  it("T-2747: a lost connection says so and offers Retry", () => {
    show(<PageFailed error={new TypeError("Failed to fetch")} onRetry={vi.fn()} />);
    expect(screen.getByText(en.app.error.generic)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: en.app.error.retry })).toBeInTheDocument();
  });

  it("T-2747: an ended session says to sign in again rather than showing an empty page", () => {
    show(<PageFailed error={new ApiError(401, "Unauthorized", problem(401, "unauthenticated"))} onRetry={vi.fn()} />);
    expect(screen.getByText(en.app.error.session)).toBeInTheDocument();
    expect(screen.queryByText("unauthenticated")).not.toBeInTheDocument();
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
  });

  it("T-3244: the reference of any numbered answer is copied with one press, a refusal's too", async () => {
    const writeText = vi.fn(() => Promise.resolve());
    vi.stubGlobal("navigator", { ...navigator, clipboard: { writeText }, onLine: true });
    show(<PageFailed error={new ApiError(403, "Forbidden", problem(403, "Not yours."), "9d8c7b6a")} />);
    expect(screen.getByText("9d8c7b6a")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: en.app.error.retry })).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: "Copy the reference 9d8c7b6a" }));
    expect(writeText).toHaveBeenCalledWith("9d8c7b6a");
    expect(await screen.findByRole("button", { name: "Copy the reference 9d8c7b6a" })).toHaveTextContent(en.app.error.copied);
    vi.unstubAllGlobals();
  });

  it("T-3243: a problem says what to do about it, in the person's language, by its type", async () => {
    show(
      <PageFailed
        error={new ApiError(409, "Conflict", { ...problem(409, "changed on main"), type: "https://joinedcontext.com/errors/conflict" })}
      />,
    );
    expect(screen.getByText(en.problem.conflict)).toBeInTheDocument();
    await i18n.changeLanguage("sk");
    expect(await screen.findByText(i18n.t("problem.conflict"))).toBeInTheDocument();
    await i18n.changeLanguage("en");
  });

  it("T-3243: the NGSI-LD surface's code wins over its ETSI type, and an unknown slug falls back to the API's hint", () => {
    const ngsi = { ...problem(400, "bad id"), type: "https://uri.etsi.org/ngsi-ld/errors/BadRequestData", code: "urn-scheme" };
    const { unmount } = show(<PageFailed error={new ApiError(400, "Bad Request", ngsi)} />);
    expect(screen.getByText(en.problem["urn-scheme"])).toBeInTheDocument();
    unmount();
    const unknown = { ...problem(422, "no"), type: "https://joinedcontext.com/errors/brand-new", hint: "Do the new thing." };
    show(<PageFailed error={new ApiError(422, "Unprocessable", unknown)} />);
    expect(screen.getByText("Do the new thing.")).toBeInTheDocument();
  });

  it("T-3243: a slug that is not one never reaches a translation key, and a refusal keeps one hint", () => {
    const odd = { ...problem(400, "x"), type: "https://joinedcontext.com/errors/../app.error.generic" };
    const { container, unmount } = show(<PageFailed error={new ApiError(400, "Bad Request", odd)} />);
    expect(container).not.toHaveTextContent(en.app.error.generic);
    unmount();
    show(<PageFailed error={new ApiError(403, "Forbidden", { ...problem(403, "no"), type: "https://joinedcontext.com/errors/forbidden" })} />);
    expect(screen.getByText(en.app.error.forbiddenHint)).toBeInTheDocument();
    expect(screen.queryByText(en.problem.forbidden)).toBeNull();
  });

  it("T-4485: a problem of the platform links its section of the public error page, any other links nothing", () => {
    const conflict = { ...problem(409, "changed on main"), type: "https://joinedcontext.com/errors/draft-conflict" };
    const { unmount } = show(<PageFailed error={new ApiError(409, "Conflict", conflict)} />);
    expect(screen.getByRole("link", { name: en.app.error.problemPage })).toHaveAttribute("href", "/docs/API/09-errors#draft-conflict");
    unmount();
    const ngsi = { ...problem(400, "bad id"), type: "https://uri.etsi.org/ngsi-ld/errors/BadRequestData", code: "urn-scheme" };
    const second = show(<PageFailed error={new ApiError(400, "Bad Request", ngsi)} />);
    expect(screen.getByRole("link", { name: en.app.error.problemPage })).toHaveAttribute("href", "/docs/API/09-errors#urn-scheme");
    second.unmount();
    const foreign = { ...problem(400, "x"), type: "https://example.org/errors/forbidden" };
    const third = show(<PageFailed error={new ApiError(400, "Bad Request", foreign)} />);
    expect(screen.queryByRole("link")).toBeNull();
    third.unmount();
    const crafted = { ...problem(400, "x"), type: "https://joinedcontext.com/errors/x#y\"onclick" };
    show(<PageFailed error={new ApiError(400, "Bad Request", crafted)} />);
    expect(screen.queryByRole("link")).toBeNull();
  });

  it("T-6200: a visitor without a session is offered no docs link, the docs being signed-in only", () => {
    const conflict = { ...problem(409, "changed on main"), type: "https://joinedcontext.com/errors/draft-conflict" };
    authStatus.value = "anonymous";
    const { unmount } = show(<PageFailed error={new ApiError(409, "Conflict", conflict)} />);
    expect(screen.getByText("changed on main")).toBeInTheDocument();
    expect(screen.queryByRole("link")).toBeNull();
    unmount();
    authStatus.value = "authenticated";
    const signedIn = show(<PageFailed error={new ApiError(409, "Conflict", conflict)} />);
    expect(screen.getByRole("link", { name: en.app.error.problemPage })).toHaveAttribute("href", "/docs/API/09-errors#draft-conflict");
    signedIn.unmount();
    // T-6313: a page outside any AuthProvider (the public catalogue explorer) has no session either.
    authStatus.value = undefined;
    show(<PageFailed error={new ApiError(409, "Conflict", conflict)} />);
    expect(screen.queryByRole("link")).toBeNull();
  });

  it("T-3244: offline says so and reads again by itself once the connection is back", () => {
    vi.stubGlobal("navigator", { ...navigator, onLine: false });
    const retry = vi.fn();
    show(<PageFailed error={new TypeError("Failed to fetch")} onRetry={retry} />);
    expect(screen.getByText(en.app.error.offline)).toBeInTheDocument();
    expect(screen.queryByText(en.app.error.generic)).toBeNull();
    window.dispatchEvent(new Event("online"));
    expect(retry).toHaveBeenCalledTimes(1);
    vi.unstubAllGlobals();
  });

  it("T-3244: a refusal never asks again when the connection comes back", () => {
    const retry = vi.fn();
    show(<PageFailed error={new ApiError(403, "Forbidden", problem(403, "Not yours."))} onRetry={retry} />);
    window.dispatchEvent(new Event("online"));
    expect(retry).not.toHaveBeenCalled();
  });

  it("T-2747: every kind reads in every locale the Portal ships", async () => {
    await inEveryLocale(async (locale) => {
      for (const key of ["app.error.session", "app.error.forbiddenHint", "app.error.problemPage", "app.error.reference", "app.error.offline", "app.error.copy"]) {
        expect(i18n.t(key), `${locale} ${key}`).not.toMatch(/^app\./);
      }
    });
  });

  it("has no axe violation, with the button and without it", async () => {
    const withRetry = show(<PageFailed error={new Error("gone")} onRetry={() => {}} />);
    await expectNoAxeViolations(withRetry.container);
    withRetry.unmount();
    const without = show(<PageFailed error={new Error("gone")} />);
    await expectNoAxeViolations(without.container);
  });
});
