/**
 * Administration → Repositories: the grid sorts every column a person can sort, in the order the
 * column means (names and words as text, sizes as numbers, changes as dates).
 */
import { cleanup, fireEvent, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { Effective } from "../src/api/permissions";
import { Repositories } from "../src/pages/organization/Repositories";
import { json, renderPage } from "./page_contract";

const ADMIN = {
  project: "org",
  bootstrap: false,
  grants: [{ role: "org-admin", binding: "admins", rule: { kinds: ["RoleBinding"], verbs: ["propose", "approve", "delete"] } }],
} as unknown as Effective;

const repo = (name: string, extra: Record<string, unknown>) => ({
  name,
  role: "project",
  project: name,
  app: null,
  defaultBranch: "main",
  updatedAt: "2026-11-10T09:00:00Z",
  sizeBytes: 1000,
  archived: false,
  ...extra,
});

const LISTING = {
  organization: "bbsk",
  maxArchiveBytes: 67108864,
  repositories: [
    repo("bravo", { sizeBytes: 100, updatedAt: "2026-11-12T09:00:00Z", archived: true, role: "application", app: "map" }),
    repo("alpha", { sizeBytes: 9, updatedAt: "2026-11-18T09:00:00Z" }),
    repo("charlie", { sizeBytes: 20, updatedAt: "2026-11-01T09:00:00Z" }),
  ],
};

beforeEach(async () => {
  await i18n.changeLanguage("en");
  try {
    window.localStorage.clear();
  } catch {
    // no storage in this run
  }
});
afterEach(cleanup);

const order = () =>
  within(screen.getByRole("table"))
    .getAllByRole("row")
    .slice(1)
    .map((row) => /alpha|bravo|charlie/.exec(row.textContent ?? "")?.[0]);

const sortBy = (label: string, item: string) => {
  fireEvent.click(screen.getByRole("button", { name: `${en.entityGrid.columnMenu}: ${label}` }));
  fireEvent.click(screen.getByRole("menuitemradio", { name: item }));
};

async function show() {
  renderPage(<Repositories />, {
    path: "/organization/repositories",
    answer: (url) => {
      if (url.pathname.endsWith("/permissions/me")) return json(ADMIN);
      if (url.pathname === "/api/v1/organization/repositories") return json(LISTING);
      return undefined;
    },
  });
  await screen.findByText("bravo");
}

describe("Administration → Repositories: sorting", () => {
  const t = en.orgRepositories;

  it("sorts by the repository's name, both ways", async () => {
    await show();
    sortBy(t.name, en.entityGrid.sortAsc);
    expect(order()).toEqual(["alpha", "bravo", "charlie"]);
    sortBy(t.name, en.entityGrid.sortDesc);
    expect(order()).toEqual(["charlie", "bravo", "alpha"]);
  });

  it("sorts by what a repository holds and whose it is", async () => {
    await show();
    // "App source …" sorts before "Project configuration …".
    sortBy(t.what, en.entityGrid.sortAsc);
    expect(order()[0]).toBe("bravo");
    sortBy(t.what, en.entityGrid.sortDesc);
    expect(order()[2]).toBe("bravo");
  });

  it("sorts by state, the archived one apart from the active ones", async () => {
    await show();
    // "Active" sorts before "Archived".
    sortBy(t.state, en.entityGrid.sortAsc);
    expect(order()[2]).toBe("bravo");
    sortBy(t.state, en.entityGrid.sortDesc);
    expect(order()[0]).toBe("bravo");
  });

  it("sorts the size as a number: 9 before 20 before 100", async () => {
    await show();
    sortBy(t.size, en.entityGrid.sortAscNumber);
    expect(order()).toEqual(["alpha", "charlie", "bravo"]);
    sortBy(t.size, en.entityGrid.sortDescNumber);
    expect(order()).toEqual(["bravo", "charlie", "alpha"]);
  });

  it("sorts the last change as a date, earliest or latest first", async () => {
    await show();
    sortBy(t.updated, en.entityGrid.sortAscDate);
    expect(order()).toEqual(["charlie", "bravo", "alpha"]);
    sortBy(t.updated, en.entityGrid.sortDescDate);
    expect(order()).toEqual(["alpha", "bravo", "charlie"]);
  });
});
