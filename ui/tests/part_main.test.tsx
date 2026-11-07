/**
 * T-1825: the entry point, against the UI contract (UI-15, UI-16, UI-45).
 *
 * `main.tsx` renders no interface of its own; what it decides is the order of the wrappers around
 * the App, and that order is the contract. The boundary sits inside the providers and outside the
 * App, so a render error anywhere below leaves a page with words and a reference on it instead of
 * the white page React unmounts to — in the reader's own language, because i18n is loaded first.
 */
import { screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n, { SUPPORTED_LOCALES } from "../src/i18n";

/** A fresh document with the one element `index.html` provides, and a fresh module graph. */
async function boot(App: () => React.JSX.Element) {
  document.body.innerHTML = '<div id="root"></div>';
  vi.resetModules();
  vi.doMock("../src/App", () => ({ App }));
  await import("../src/main");
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
  vi.stubGlobal(
    "fetch",
    vi.fn(() => Promise.resolve(new Response("{}", { status: 200, headers: { "Content-Type": "application/json" } }))),
  );
});

afterEach(() => {
  vi.doUnmock("../src/App");
  vi.resetModules();
  vi.restoreAllMocks();
  document.body.innerHTML = "";
});

describe("the entry point against the UI contract", () => {
  it("renders the App into the one root the page provides", async () => {
    await boot(() => <p>the portal</p>);

    await waitFor(() =>
      expect(document.getElementById("root")).toHaveTextContent("the portal"),
    );
    // One root, mounted in place: nothing is appended beside it.
    expect(document.body.children).toHaveLength(1);
  });

  it("leaves words and a reference on the page when something below it throws", async () => {
    // React logs the error and the component stack itself; the test is about what the person sees.
    const logged = vi.spyOn(console, "error").mockImplementation(() => {});
    await boot(() => {
      throw new Error("a manifest that was not there");
    });

    const page = await screen.findByRole("alert");
    expect(page).toHaveTextContent(i18n.t("app.error.crashTitle"));
    expect(screen.getByTestId("error-reference").textContent).toMatch(/^[0-9a-z-]{4,}$/);
    expect(screen.getByRole("button", { name: i18n.t("app.error.crashReload") })).toBeEnabled();
    expect(screen.getByRole("link", { name: i18n.t("app.error.crashHome") })).toHaveAttribute("href", "/");
    // What threw it never reaches the page: a message carries URLs, bodies and tokens (UI-46).
    expect(page.textContent).not.toContain("a manifest that was not there");
    expect(logged).toHaveBeenCalled();
  });

  it("loads the page again when the Portal's own first chunk did not load (T-5731)", async () => {
    // A rollout swapped the asset names, or the edge refused the chunk with 429: the import of the
    // App rejects before React exists, so no boundary sees it. It reloads once like a lazy page
    // (T-4949), instead of an unhandled rejection and index.html's "did not load" notice.
    const logged = vi.spyOn(console, "error").mockImplementation(() => {});
    const unhandled = vi.fn();
    process.on("unhandledRejection", unhandled);
    const reload = vi.fn();
    vi.spyOn(window, "location", "get").mockReturnValue({ ...window.location, reload });
    window.sessionStorage.clear();
    document.body.innerHTML = '<div id="root"></div>';
    vi.resetModules();
    // vitest wraps an error its mock factory throws, so the module's one export throws the browser's words.
    vi.doMock("../src/App", () => ({
      get App() {
        throw new TypeError("Failed to fetch dynamically imported module: /assets/App-x.js");
      },
    }));
    await import("../src/main");

    await waitFor(() => expect(reload).toHaveBeenCalledOnce());
    await new Promise((resolve) => setTimeout(resolve, 0));
    process.off("unhandledRejection", unhandled);
    expect(unhandled).not.toHaveBeenCalled();
    expect(logged).not.toHaveBeenCalled();
  });

  it.each(SUPPORTED_LOCALES)("says that in %s, because i18n is loaded before the App", async (locale) => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    await i18n.changeLanguage(locale);
    await boot(() => {
      throw new Error("boom");
    });

    const page = await screen.findByRole("alert");
    expect(page).toHaveTextContent(i18n.t("app.error.crashTitle"));
    expect(page.textContent).not.toContain("app.error.crashTitle");
  });
});
