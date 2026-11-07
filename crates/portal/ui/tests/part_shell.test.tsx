/**
 * T-1821: the shell every page of a project is rendered inside, against the UI contract
 * (UI-11, UI-15, UI-16).
 *
 * The shell is the only chrome a person never leaves, so its contract is the keyboard one: the
 * skip link before everything, one real control per action, the phone menu closing on Escape with
 * the focus back where it came from, and a footer whose address comes from a branding file and is
 * therefore rendered through `safeHref` (UI-48).
 */
// covers (T-2137, the module gate in gate_modules.test.ts): the cases in this file drive
// src/components/ui/icons.tsx through the page they belong to; each was confirmed by
// making the module throw and watching this file go red.
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n, { SUPPORTED_LOCALES } from "../src/i18n";
import { App } from "../src/App";
import { expectNoRawKeys, expectNoViolations } from "./checks";
import { NAV_SECTIONS } from "../src/components/layout/navigation";

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  roles: ["portal-viewer"],
};

const PROJECTS = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items: [{ name: "helsinki" }, { name: "banskabystrica" }],
};

const EMPTY_LIST = { apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] };

const BRANDING = {
  instanceName: "Mesto Banská Bystrica — joinedcontext",
  shortName: "BB kontext",
  organisation: "Mesto Banská Bystrica",
  contactEmail: "data@banskabystrica.sk",
};

let branding: Record<string, unknown>;

/** The shell of one project, up once its sidebar is; `answers` fills in what a page needs. */
async function shell(
  path = "/projects/banskabystrica/spaces",
  answers: Record<string, unknown> = {},
) {
  window.history.pushState({}, "", path);
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL) => {
      const href = input instanceof Request ? input.url : String(input);
      const url = new URL(href, window.location.origin);
      const body = answers[url.pathname] !== undefined
        ? answers[url.pathname]
        : url.pathname.endsWith("/auth/me")
        ? IDENTITY
        : url.pathname === "/api/v1/branding"
          ? branding
          : url.pathname === "/api/v1/projects"
            ? PROJECTS
            : EMPTY_LIST;
      return Promise.resolve(
        new Response(JSON.stringify(body), { status: 200, headers: { "Content-Type": "application/json" } }),
      );
    }),
  );
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const view = render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <App />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  await waitFor(() =>
    expect(screen.getByRole("navigation", { name: i18n.t("nav.main") })).toBeInTheDocument(),
  );
  return { ...view, user: userEvent.setup() };
}

/** The chrome itself: the header, the sidebar and the footer, without the page inside it. */
function chrome(): HTMLElement[] {
  const header = document.querySelector("header") as HTMLElement;
  const sidebar = document.getElementById("portal-sidebar") as HTMLElement;
  const footer = document.querySelector("footer") as HTMLElement;
  return [header, sidebar, footer].filter(Boolean);
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
  window.localStorage.removeItem("jc-lang");
  branding = { ...BRANDING };
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("the shell against the UI contract", () => {
  it("puts the skip link before everything, and it lands on the page", async () => {
    const { user } = await shell();

    await user.tab();
    const first = document.activeElement as HTMLElement;
    expect(first).toHaveTextContent(i18n.t("nav.skipToContent"));
    expect(first.getAttribute("href")).toBe("#main");
    expect(document.getElementById("main")).not.toBeNull();
    // Out of sight until it has the focus, never out of the tab order.
    expect(first.className).toContain("sr-only");
    expect(first.className).toContain("focus:not-sr-only");
  });

  it("has no axe violation in the header, the sidebar or the footer", async () => {
    await shell();

    for (const part of chrome()) {
      await expectNoViolations(part);
    }
  });

  it("names every control of the chrome, and hand-makes none of them", async () => {
    await shell();

    const header = document.querySelector("header") as HTMLElement;
    for (const control of Array.from(header.querySelectorAll("button, a"))) {
      expect(control, control.outerHTML).toHaveAccessibleName();
    }
    // Every acting control of the header is the shared Button: one focus ring, one size scale.
    for (const button of Array.from(header.querySelectorAll("button"))) {
      expect(button.className, button.outerHTML).toContain("focus-ring");
    }
    expect(header.querySelectorAll("div[onclick], span[onclick]")).toHaveLength(0);
  });

  it("closes the phone menu with Escape and gives the focus back to the toggle", async () => {
    const { user } = await shell();

    const toggle = screen.getByRole("button", { name: i18n.t("nav.menu") });
    expect(toggle).toHaveAttribute("aria-controls", "portal-sidebar");
    expect(toggle).toHaveAttribute("aria-expanded", "false");

    await user.click(toggle);
    expect(toggle).toHaveAttribute("aria-expanded", "true");

    await user.keyboard("{Escape}");
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    // The way out leaves the focus where the way in was, not at the top of the document.
    expect(document.activeElement).toBe(toggle);
  });

  it("opens the project switcher and the account menu from the keyboard", async () => {
    const { user } = await shell();

    const switcher = screen.getByRole("button", { name: i18n.t("nav.projects") });
    switcher.focus();
    await user.keyboard("{Enter}");
    const projects = await screen.findByRole("menu");
    expect(within(projects).getByRole("menuitem", { name: "helsinki" })).toBeInTheDocument();
    await user.keyboard("{Escape}");

    const account = screen.getByRole("button", {
      name: i18n.t("auth.signedInAs", { name: IDENTITY.name }),
    });
    account.focus();
    await user.keyboard("{Enter}");
    expect(await screen.findByRole("menu")).toBeInTheDocument();
  });

  it("keeps the chrome up when the page inside it throws, and says what to do", async () => {
    // The router renders the shell and the page as one component, so before T-2426 an error in
    // any page replaced the whole application with the router's own "Something went wrong!" —
    // no sidebar, no project switcher, nothing but the browser's Back button.
    const { user } = await shell("/projects/banskabystrica/spaces/air-quality");

    // The page below the chrome is a detail page with no resource behind it, which is what
    // throws: the shell's stub answers a list where an object was expected.
    const panel = await screen.findByRole("alert");
    expect(within(panel).getByText(i18n.t("app.error.crashTitle"))).toBeInTheDocument();
    expect(within(panel).getByTestId("error-reference").textContent).toMatch(/^[0-9a-z-]{4,}$/);
    // Nothing of what threw reaches the page: no stack, no path, no message of the error.
    expect(panel.textContent).not.toMatch(/dataModelRef|TypeError|http|\.tsx/);

    // The chrome is still there and still works.
    expect(screen.getByRole("navigation", { name: i18n.t("nav.main") })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: i18n.t("nav.projects") }));
    const menu = await screen.findByRole("menu");
    expect(within(menu).getByRole("menuitem", { name: "helsinki" })).toHaveAttribute(
      "href",
      expect.stringContaining("/projects/helsinki/"),
    );
    await user.keyboard("{Escape}");

    // And the two controls the panel offers: the way to a page that works, and drawing this one
    // again without reloading the application.
    const again = within(panel).getByRole("button", { name: i18n.t("app.error.retry") });
    const start = within(panel).getByRole("link", { name: i18n.t("app.error.crashHome") });
    expect(start).toHaveAttribute("href", `/projects/banskabystrica/${NAV_SECTIONS[0].plural}`);
    await user.click(again);
    // It threw for the same reason, so the panel is back rather than a white page — a new
    // element, because the retry drew the page again and it failed again.
    const drawnAgain = await screen.findByRole("alert");
    expect(drawnAgain).toBeInTheDocument();

    // It is words a person reads, and axe sees nothing wrong with it.
    expectNoRawKeys(drawnAgain);
    await expectNoViolations(drawnAgain);
  });

  it("says the same thing in Slovak, where the page that failed is read in Slovak", async () => {
    window.localStorage.setItem("jc-lang", "sk");
    await i18n.changeLanguage("sk");
    const { user } = await shell("/projects/banskabystrica/spaces/air-quality");

    const panel = await screen.findByRole("alert");
    expect(within(panel).getByText(i18n.t("app.error.crashTitle"))).toBeInTheDocument();
    expect(within(panel).getByRole("button", { name: i18n.t("app.error.retry") })).toBeInTheDocument();
    expectNoRawKeys(panel);
    // The chrome is Slovak too, and still there.
    expect(screen.getByRole("button", { name: i18n.t("nav.projects") })).toBeInTheDocument();
    await user.keyboard("{Escape}");
  });

  /** The other project's entry of the open switcher, and the active one beside it. */
  async function switcherLinks(user: ReturnType<typeof userEvent.setup>) {
    await user.click(screen.getByRole("button", { name: i18n.t("nav.projects") }));
    const menu = await screen.findByRole("menu");
    return {
      other: within(menu).getByRole("menuitem", { name: "helsinki" }) as HTMLAnchorElement,
      active: within(menu).getByRole("menuitem", { name: "banskabystrica" }) as HTMLAnchorElement,
    };
  }

  it("switches project without leaving the page a person is on", async () => {
    // The menu used to send every switch to Spaces, so comparing two projects' endpoints cost
    // two clicks back every time (T-2425, UI-05).
    const { user } = await shell("/projects/banskabystrica/endpoints");
    const { other, active } = await switcherLinks(user);

    expect(other).toHaveAttribute("href", "/projects/helsinki/endpoints");
    // The active project keeps its mark, and its own entry stays on this page too.
    // `page` when the active entry is the very page in hand, which is what the router marks it
    // with; the explicit `true` stays for a detail page, where the entry points at the list.
    expect(active.getAttribute("aria-current")).toBeTruthy();
    expect(other).not.toHaveAttribute("aria-current");
    expect(active).toHaveAttribute("href", "/projects/banskabystrica/endpoints");
  });

  it("falls back to the section's list when the page named one resource", async () => {
    const { user } = await shell("/projects/banskabystrica/endpoints/air-quality", {
      "/api/v1/projects/banskabystrica/endpoints/air-quality": {
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "Endpoint",
        metadata: { name: "air-quality", namespace: "banskabystrica" },
        spec: { contextSpaceRef: "air", audience: "public", enabledRepresentations: ["ngsi-ld"] },
        status: { slug: "abcdefghijklmnopqrstuvwxyz", phase: "Ready" },
      },
    });
    expect((await switcherLinks(user)).other).toHaveAttribute("href", "/projects/helsinki/endpoints");
  });

  it("carries no name of the project being left into the other project's URL", async () => {
    // `space` and `q` are banskabystrica's; helsinki's Explore has neither, and a name that
    // happens to exist in both is a different resource (T-2425).
    const { user } = await shell("/projects/banskabystrica/explore?space=air&q=bus&entityId=urn:x");
    const href = (await switcherLinks(user)).other.getAttribute("href") ?? "";

    expect(href).toBe("/projects/helsinki/explore");
    for (const carried of ["space", "entityId", "air", "urn", "bus"]) {
      expect(href, carried).not.toContain(carried);
    }
  });

  // T-4243 (photo audit T-4157 #3): the switcher and the trail showed the slug alone, so a person
  // read "banskabystrica" where the city's own name belongs.
  it("names each project by its title, in the page's language, with the slug beside it", async () => {
    await i18n.changeLanguage("sk");
    const { user } = await shell("/projects/banskabystrica/spaces", {
      "/api/v1/projects": {
        ...PROJECTS,
        items: [
          { name: "helsinki", sample: false },
          { name: "banskabystrica", sample: false, title: { sk: "Dáta mesta Banská Bystrica", en: "Banská Bystrica city data" } },
        ],
      },
    });

    const switcher = screen.getByRole("button", { name: i18n.t("nav.projects") });
    await waitFor(() => expect(switcher).toHaveTextContent("Dáta mesta Banská Bystrica"));
    expect(within(switcher).getByText("banskabystrica")).toBeInTheDocument();
    const trail = screen.getByRole("navigation", { name: i18n.t("nav.breadcrumb") });
    expect(within(trail).getByRole("link", { name: "Dáta mesta Banská Bystrica" })).toHaveAttribute(
      "href",
      "/projects/banskabystrica/spaces",
    );

    await user.click(switcher);
    const menu = await screen.findByRole("menu");
    const titled = within(menu).getByRole("menuitem", { name: /Dáta mesta Banská Bystrica/ });
    expect(titled).toHaveTextContent("banskabystrica");
    // No title: the slug alone, said once.
    const bare = within(menu).getByRole("menuitem", { name: "helsinki" });
    expect(bare.textContent?.match(/helsinki/g)).toHaveLength(1);
  });

  it("keeps the slug alone where the manifest names no title", async () => {
    await shell();

    const switcher = screen.getByRole("button", { name: i18n.t("nav.projects") });
    expect(within(switcher).getAllByText("banskabystrica")).toHaveLength(1);
    const trail = screen.getByRole("navigation", { name: i18n.t("nav.breadcrumb") });
    expect(within(trail).getByRole("link", { name: "banskabystrica" })).toBeInTheDocument();
  });

  it("writes the installation's address as a mail link, never as whatever the file said", async () => {
    // A branding file is edited by hand, so the footer's address is untrusted text (UI-48).
    branding = { ...BRANDING, contactEmail: "javascript:alert(1)" };
    await shell();

    const link = within(document.querySelector("footer") as HTMLElement).getByRole("link", { name: /javascript/ });
    expect(link.getAttribute("href")).toBe("mailto:javascript:alert(1)");
    expect(link.getAttribute("href")).not.toMatch(/^javascript:/);
  });

  // T-4236 (photo audit T-4163 #8): the footer under the whole shell made every page the window
  // plus 41 px, so a scroll moved the window-tall sidebar up over the sticky header and hid the
  // brand. The footer ends the page's own column, so a short page does not scroll at all.
  it("ends the page's column with the footer, not the whole shell", async () => {
    await shell();

    const footer = document.querySelector("footer") as HTMLElement;
    expect(footer.parentElement).toBe(document.getElementById("main")?.parentElement);
    expect(footer.parentElement?.contains(document.getElementById("portal-sidebar"))).toBe(false);
  });

  it("says only the accessibility statement and the glossary in the footer when the installation configured nothing (T-6344)", async () => {
    branding = { instanceName: "joinedcontext", shortName: "joinedcontext" };
    await shell();

    const footer = document.querySelector("footer") as HTMLElement;
    expect(within(footer).getAllByRole("link").map((link) => link.getAttribute("href"))).toEqual(["/accessibility", "/glossary"]);
    expect(footer.querySelector("span")).toBeNull();
  });

  it("shows the installation's own name and keeps a long project name in its own width", async () => {
    await shell();

    expect(screen.getAllByText(BRANDING.shortName).length).toBeGreaterThan(0);
    const switcher = screen.getByRole("button", { name: i18n.t("nav.projects") });
    // The name is cut in the button rather than pushing the account menu off the header.
    expect(switcher.querySelector(".truncate")).not.toBeNull();
    const logo = document.querySelector("header img");
    if (logo) {
      expect(logo).toHaveAttribute("alt", "");
      expect(logo).toHaveAttribute("aria-hidden", "true");
    }
  });

  it.each(SUPPORTED_LOCALES)("writes the chrome's own words in %s", async (locale) => {
    window.localStorage.setItem("jc-lang", locale);
    await i18n.changeLanguage(locale);
    await shell();

    for (const part of chrome()) {
      // The signed-in person's name and username are marked translate="no": data, not strings.
      expectNoRawKeys(part, ['[translate="no"]']);
    }
  });
});
