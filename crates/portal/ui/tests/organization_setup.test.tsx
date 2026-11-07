/**
 * T-2748: Organization → Setup (PF-90, UI-82, API/01 §25).
 *
 * The tab lists what a new organization still lacks, each step with the page that does it, and
 * what the installation's operator provides; the other tabs remind an administrator while
 * anything is open. Only a person who approves changes to the organization (`org-admin`) is
 * shown it, and nobody else causes the read at all.
 */
import { cleanup, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { Effective } from "../src/api/permissions";
import { OrganizationPage } from "../src/pages/organization/OrganizationPage";
import type { OrganizationTab } from "../src/pages/organization/OrganizationPage";
import { OrganizationSetup } from "../src/pages/organization/OrganizationSetup";
import { expectNoAxeViolations, json, problem, renderPage } from "./page_contract";

const STEPS = ["organization", "domain", "people", "project", "publishers", "policies"] as const;
const OPERATOR = ["branding", "loginTheme", "smtp", "backups"] as const;

function setup(done: (id: string) => boolean) {
  const steps = STEPS.map((id) => ({ id, done: done(id) }));
  const operator = OPERATOR.map((id) => ({ id, done: done(id) }));
  return { complete: [...steps, ...operator].every((item) => item.done), steps, operator };
}

const ADMIN = {
  project: "org",
  bootstrap: false,
  grants: [
    {
      role: "org-admin",
      binding: "admins",
      rule: { kinds: ["Organization", "Project", "RoleBinding"], verbs: ["propose", "approve", "delete"] },
    },
  ],
} as unknown as Effective;

const VIEWER = {
  project: "org",
  bootstrap: false,
  grants: [{ role: "viewer", binding: "everyone", rule: { kinds: ["Organization", "Project"], verbs: ["read"] } }],
} as unknown as Effective;

function renderAt(tab: OrganizationTab, permissions: Effective, answer: () => Response) {
  const requests: string[] = [];
  const result = renderPage(<OrganizationPage tab={tab} anchor="helsinki" />, {
    path: `/organization/${tab}`,
    answer: (url, request) => {
      requests.push(`${request.method} ${url.pathname}`);
      if (url.pathname.endsWith("/permissions/me")) return json(permissions);
      if (url.pathname === "/api/v1/organization/setup") return answer();
      return undefined;
    },
  });
  return { ...result, requests };
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("Organization → Setup", () => {
  it("lists every step in order, its state in words, and the page that does it", async () => {
    const { container } = renderAt("setup", ADMIN, () => json(setup((id) => id === "organization" || id === "smtp")));
    const items = (await screen.findAllByRole("heading", { level: 3 })).map((heading) => heading.textContent);
    expect(items.slice(0, STEPS.length)).toEqual(
      STEPS.map((id, index) => `Step ${index + 1}: ${en.organization.setup.step[id].title}`),
    );
    expect(items.slice(STEPS.length)).toEqual(OPERATOR.map((id) => en.organization.setup.operator[id].title));

    const first = screen.getByRole("heading", { name: /Step 1:/ }).closest("li") as HTMLElement;
    expect(within(first).getByText(en.organization.setup.done)).toBeInTheDocument();
    const second = screen.getByRole("heading", { name: /Step 2:/ }).closest("li") as HTMLElement;
    expect(within(second).getByText(en.organization.setup.todo)).toBeInTheDocument();

    const links = screen.getAllByRole("link").map((link) => [link.textContent, link.getAttribute("href")]);
    expect(links).toEqual([
      [en.organization.setup.step.organization.action, "/organization/settings"],
      [en.organization.setup.step.domain.action, "/organization/settings"],
      [en.organization.setup.step.people.action, "/organization/people"],
      [en.organization.setup.step.project.action, "/organization/projects"],
      [en.organization.setup.step.publishers.action, "/projects/helsinki/ckan"],
      [en.organization.setup.step.policies.action, "/organization/settings"],
    ]);
    expect(screen.queryByText(en.organization.setup.complete)).not.toBeInTheDocument();
    await expectNoAxeViolations(container);
  });

  it("says the organization is set up once every step and operator item is done", async () => {
    renderAt("setup", ADMIN, () => json(setup(() => true)));
    expect(await screen.findByText(en.organization.setup.complete)).toBeInTheDocument();
    expect(screen.queryByText(en.organization.setup.todo)).not.toBeInTheDocument();
  });

  it("tells anyone else who sees it, and never asks the API", async () => {
    // The page itself answers a non-administrator first (UI-75); the tab keeps its own refusal.
    const requests: string[] = [];
    renderPage(<OrganizationSetup anchor="helsinki" />, {
      path: "/organization/setup",
      answer: (url, request) => {
        requests.push(`${request.method} ${url.pathname}`);
        if (url.pathname.endsWith("/permissions/me")) return json(VIEWER);
        if (url.pathname === "/api/v1/organization/setup") return json(setup(() => false));
        return undefined;
      },
    });
    expect(await screen.findByText(en.organization.setup.hidden)).toBeInTheDocument();
    expect(requests).not.toContain("GET /api/v1/organization/setup");
  });

  it("opens the catalogue of the project the shell is in", async () => {
    renderPage(<OrganizationSetup anchor="espoo" />, {
      path: "/organization/setup",
      answer: (url) => {
        if (url.pathname.endsWith("/permissions/me")) return json(ADMIN);
        if (url.pathname === "/api/v1/organization/setup") return json(setup(() => false));
        return undefined;
      },
    });
    expect(await screen.findByRole("link", { name: en.organization.setup.step.publishers.action })).toHaveAttribute(
      "href",
      "/projects/espoo/ckan",
    );
  });

  it("shows the API's refusal in its own words", async () => {
    renderAt("setup", ADMIN, () => problem(403, "Only an administrator of the organization reads its setup"));
    expect(await screen.findByText(/Only an administrator of the organization reads its setup/)).toBeInTheDocument();
  });
});

describe("the setup reminder", () => {
  it("counts what is open on the other tabs and leads to the setup", async () => {
    renderAt("people", ADMIN, () => json(setup((id) => id !== "domain" && id !== "backups")));
    expect(await screen.findByText("2 setup items are still open.")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: en.organization.setup.continue })).toHaveAttribute(
      "href",
      "/organization/setup",
    );
  });

  it("is gone once the setup is complete, and is never shown to anybody else", async () => {
    const done = renderAt("people", ADMIN, () => json(setup(() => true)));
    await vi.waitFor(() => expect(done.requests).toContain("GET /api/v1/organization/setup"));
    expect(screen.queryByText(/setup items? (is|are) still open/)).not.toBeInTheDocument();
    cleanup();

    const viewer = renderAt("people", VIEWER, () => json(setup(() => false)));
    await vi.waitFor(() => expect(viewer.requests.some((request) => request.endsWith("/permissions/me"))).toBe(true));
    expect(screen.queryByRole("link", { name: en.organization.setup.continue })).not.toBeInTheDocument();
    expect(viewer.requests).not.toContain("GET /api/v1/organization/setup");
  });
});
