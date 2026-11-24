/**
 * Organization limits view: the shared grids sort the settings of a section and the quota of the
 * projects, and a project's origin is said in words.
 */
import { cleanup, fireEvent, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { OrganizationLimitsView } from "../src/pages/organization/OrganizationLimitsView";
import type { OrganizationLimits } from "../src/pages/organization/limits";
import { renderPage } from "./page_contract";

const LIMITS: OrganizationLimits = {
  entries: [],
  projects: [
    { project: "zeta", origin: "project", quota: { contextSpaces: { limit: 4, used: 1 } } },
    { project: "alpha", origin: "default", quota: {} },
  ],
  secondFactorFloor: "optional",
};

const bodyRows = (scope: HTMLElement) =>
  within(scope)
    .getAllByRole("row")
    .slice(1)
    .map((row) => row.textContent ?? "");

const sortBy = (scope: HTMLElement, label: string, item: string) => {
  fireEvent.click(within(scope).getByRole("button", { name: `${en.entityGrid.columnMenu}: ${label}` }));
  fireEvent.click(screen.getByRole("menuitemradio", { name: item }));
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

describe("Organization limits view: sorting the grids", async () => {
  it("sorts the settings of a section by their name, both ways", async () => {
    renderPage(<OrganizationLimitsView limits={LIMITS} />, { answer: () => undefined });
    const section = await screen.findByRole("region", { name: en.organization.limits.section.applications });
    const apps = en.organization.policy.publicApps;
    const services = en.appServices.field;
    const [first, second] = [apps, services].sort((a, b) => a.localeCompare(b));
    sortBy(section, en.organization.limits.setting, en.entityGrid.sortAsc);
    let rows = bodyRows(section);
    expect(rows[0]).toContain(first);
    expect(rows[1]).toContain(second);
    sortBy(section, en.organization.limits.setting, en.entityGrid.sortDesc);
    rows = bodyRows(section);
    expect(rows[0]).toContain(second);
    expect(rows[1]).toContain(first);
  });

  it("sorts the projects by name and by whose limit is in force", async () => {
    renderPage(<OrganizationLimitsView limits={LIMITS} />, { answer: () => undefined });
    const section = await screen.findByRole("region", { name: en.organization.limits.perProject });
    expect(bodyRows(section)).toHaveLength(2);
    sortBy(section, en.organization.limits.project, en.entityGrid.sortAsc);
    expect(bodyRows(section)[0]).toContain("alpha");
    sortBy(section, en.organization.limits.project, en.entityGrid.sortDesc);
    expect(bodyRows(section)[0]).toContain("zeta");
    // The origin is said in words, and the grid orders by those words.
    const own = en.organization.limits.origin.project;
    const inherited = en.organization.limits.origin.default;
    sortBy(section, en.organization.limits.whose, en.entityGrid.sortAsc);
    const [low] = [own, inherited].sort((a, b) => a.localeCompare(b));
    expect(bodyRows(section)[0]).toContain(low === own ? "zeta" : "alpha");
  });

  it("says what a project may use and has used, and that a limit is not set", async () => {
    renderPage(<OrganizationLimitsView limits={LIMITS} />, { answer: () => undefined });
    const section = await screen.findByRole("region", { name: en.organization.limits.perProject });
    const zeta = bodyRows(section).find((row) => row.includes("zeta"));
    expect(zeta).toContain("1 of 4");
    expect(bodyRows(section).find((row) => row.includes("alpha"))).toContain(en.organization.limits.noLimit);
  });

  it("says there are no projects when none is readable", async () => {
    renderPage(<OrganizationLimitsView limits={{ ...LIMITS, projects: [] }} />, { answer: () => undefined });
    expect(await screen.findByText(en.organization.limits.noProjects)).toBeInTheDocument();
  });
});
