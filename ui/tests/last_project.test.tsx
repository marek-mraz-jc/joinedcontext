/**
 * T-2753: a page that belongs to no project opens on the project the person last worked in.
 *
 * The organization's tabs, `/endpoints` and `/` took the first project of the list, so a person
 * working in helsinki who opened the Organization page found the header switched to banskabystrica
 * (the first one alphabetically) and every link from there led into the wrong project.
 */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";
import { lastAddress, preferredProject, rememberAddress, rememberProject } from "../src/api/projects";

const IDENTITY = { subject: "b7c1e0f4", username: "jana.kovacova", roles: ["portal-viewer"] };
const LIST = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
// banskabystrica is the sample every newcomer may read, listed first by name (T-5603).
const PROJECTS = LIST([{ name: "banskabystrica", sample: true }, { name: "helsinki", sample: false }]);

function renderAt(path: string) {
  window.history.pushState({}, "", path);
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL) => {
      const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
      const body = url.includes("/auth/me")
        ? IDENTITY
        : url.endsWith("/api/v1/projects")
          ? PROJECTS
          : url.includes("/permissions/me")
            ? { project: "helsinki", bootstrap: true, grants: [] }
            : LIST([]);
      return Promise.resolve(
        new Response(JSON.stringify(body), { status: 200, headers: { "Content-Type": "application/json" } }),
      );
    }),
  );
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <App />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

/** The project the header's switcher names. */
async function selected(): Promise<string> {
  const switcher = await screen.findByRole("button", { name: en.nav.projects });
  return switcher.textContent ?? "";
}

describe("the project a page outside any project opens on (T-2753)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.localStorage.clear();
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    window.localStorage.clear();
    window.history.pushState({}, "", "/");
  });

  it("remembers the project a page was drawn for", async () => {
    renderAt("/projects/helsinki/spaces");
    await waitFor(async () => expect(await selected()).toContain("helsinki"));
    expect(preferredProject(["banskabystrica", "helsinki"])).toBe("helsinki");
  });

  // Administration shows no project switcher since T-4831: its way back opens the project last
  // worked in, and visiting it changes nothing about which one that is.
  for (const path of ["/organization/settings", "/organization/members", "/endpoints"]) {
    it(`keeps helsinki on ${path}`, async () => {
      rememberProject("helsinki");
      renderAt(path);
      expect(await screen.findByRole("link", { name: en.organization.nav.back })).toHaveAttribute(
        "href",
        "/projects/helsinki/home",
      );
      expect(preferredProject(["banskabystrica", "helsinki"])).toBe("helsinki");
    });
  }

  it("sends / to the home of the project last worked in", async () => {
    rememberProject("helsinki");
    renderAt("/");
    // The project's home since T-3233, with its first-run checklist and the cards of the role.
    await waitFor(() => expect(window.location.pathname).toBe("/projects/helsinki/home"));
  });

  it("falls back to the first project when the remembered one is not readable any more", () => {
    rememberProject("praha");
    expect(preferredProject(["banskabystrica", "helsinki"])).toBe("banskabystrica");
    expect(preferredProject([])).toBeUndefined();
  });

  it("keeps working when the browser refuses storage", () => {
    const get = vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    const set = vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    expect(() => rememberProject("helsinki")).not.toThrow();
    expect(preferredProject(["banskabystrica", "helsinki"])).toBe("banskabystrica");
    get.mockRestore();
    set.mockRestore();
  });

  it("sends a first sign-in to the person's own project before a sample (T-5603)", async () => {
    renderAt("/");
    await waitFor(() => expect(window.location.pathname).toBe("/projects/helsinki/home"));
  });

  it("prefers the own project, and keeps the one last worked in, sample or not (T-5603)", () => {
    expect(preferredProject(["banskabystrica", "helsinki"], ["banskabystrica"])).toBe("helsinki");
    // Only samples to read: the first of them.
    expect(preferredProject(["banskabystrica"], ["banskabystrica"])).toBe("banskabystrica");
    rememberProject("banskabystrica");
    expect(preferredProject(["banskabystrica", "helsinki"], ["banskabystrica"])).toBe("banskabystrica");
  });
});

describe("the page a project switch reopens (T-5603)", () => {
  const ROWS = "/projects/banskabystrica/spaces/bb-ovzdusie?type=AirQualityObserved";

  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.localStorage.clear();
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    window.localStorage.clear();
    window.history.pushState({}, "", "/");
  });

  it("remembers the last page per project, and only a page of that project", () => {
    rememberAddress("banskabystrica", ROWS);
    rememberAddress("helsinki", "/projects/helsinki/endpoints");
    expect(lastAddress("banskabystrica")).toBe(ROWS);
    expect(lastAddress("helsinki")).toBe("/projects/helsinki/endpoints");
    // Another project's page, another origin or a protocol-relative path is never kept.
    rememberAddress("helsinki", "/projects/banskabystrica/spaces");
    rememberAddress("helsinki", "https://evil.example/projects/helsinki/");
    rememberAddress("helsinki", "/projects/helsinki//evil.example");
    expect(lastAddress("helsinki")).toBe("/projects/helsinki/endpoints");
    expect(lastAddress("praha")).toBeUndefined();
  });

  it("reads a tampered or blocked store as nothing remembered", () => {
    window.localStorage.setItem("jc.lastAddress", JSON.stringify({ helsinki: "//evil.example/x" }));
    expect(lastAddress("helsinki")).toBeUndefined();
    window.localStorage.setItem("jc.lastAddress", "not json");
    expect(lastAddress("helsinki")).toBeUndefined();
    const get = vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    expect(lastAddress("helsinki")).toBeUndefined();
    get.mockRestore();
  });

  it("reopens the space and type left in a project, and keeps the section in one never visited", async () => {
    rememberAddress("banskabystrica", ROWS);
    renderAt("/projects/helsinki/endpoints");
    await waitFor(() => expect(lastAddress("helsinki")).toBe("/projects/helsinki/endpoints"));
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: en.nav.projects }));
    const menu = await screen.findByRole("menu");
    expect(within(menu).getByRole("menuitem", { name: /banskabystrica/ })).toHaveAttribute("href", ROWS);
    expect(within(menu).getByRole("menuitem", { name: /helsinki/ })).toHaveAttribute(
      "href",
      "/projects/helsinki/endpoints",
    );
    window.localStorage.removeItem("jc.lastAddress");
  });
});
