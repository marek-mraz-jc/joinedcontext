/**
 * A walk of every page as each demo person (T-0759): the Spaces page (React #31 on an object
 * reference) and the Models page (`filter is not a function`) crashed on dev and a person found
 * them first. Every route of the main navigation, in both projects, plus the detail pages and
 * the assistant, is opened; a console error, an uncaught error, the error screen, a 5xx or a 4xx
 * the person should not meet is a finding. The findings are attached as a report.
 */
import { expect, test } from "@playwright/test";
import type { Page, Response } from "@playwright/test";
import { APPROVER, STEWARD, VIEWER, hiddenSections, inHiddenSection, signIn } from "./portal";

const PROJECTS = ["helsinki", "banskabystrica"];

/** One seeded model of each project, whose own page the walk opens (T-2765). */
const MODELS: Record<string, string> = { helsinki: "helsinki", banskabystrica: "bb-air-quality" };

interface Finding {
  who: string;
  route: string;
  kind: "console" | "pageerror" | "error-screen" | "http";
  detail: string;
}

/**
 * A 4xx a person may meet without anything being wrong: a viewer or an approver refused a write or
 * a grant (the seed gives neither a data Policy, only the steward), and the space page asking the
 * space surface whether this person holds a grant at all, which a person with none is answered 404
 * so the surface says nothing about the space (SP-06, SpaceInside's SpaceData).
 */
function expected(who: string, response: Response): boolean {
  const status = response.status();
  if (status === 403 && (who === "viewer" || who === "approver")) {
    return true;
  }
  if (status === 404 && /\/cs\/[^/]+\/ngsi-ld\/v1\/entities\?/.test(response.url())) {
    return true;
  }
  // A feed or a run that has expired answers 404 or 410 on purpose; the pages say so in words.
  return status === 410 || (status === 404 && /\/agent-runs\/[^/]+\/(preview|events)/.test(response.url()));
}

async function routesOf(page: Page, project: string): Promise<string[]> {
  const model = MODELS[project];
  await page.goto(`/projects/${project}/spaces?lang=en`, { waitUntil: "load" });
  const nav = page.getByRole("navigation", { name: "Main navigation" });
  const hrefs = await nav.locator("a[href^='/']").evaluateAll((links) =>
    links.map((link) => (link as HTMLAnchorElement).getAttribute("href") ?? ""),
  );
  const details = await page
    .locator("main a[href*='/spaces/']:not([href*='/spaces/complete']):not([href*='/spaces/upload'])")
    .evaluateAll((links) => links.slice(0, 1).map((link) => (link as HTMLAnchorElement).getAttribute("href") ?? ""));
  return [
    ...new Set([
      ...hrefs.map((href) => href.replace(/\/projects\/[^/]+\//, `/projects/${project}/`)),
      ...details,
      `/projects/${project}/spaces/complete`,
      `/projects/${project}/spaces/upload`,
      `/projects/${project}/assistant`,
      `/projects/${project}/explore`,
      `/projects/${project}/models`,
      `/projects/${project}/models/${model}`,
    ]),
  ].filter((href) => href.startsWith("/"));
}

for (const [who, person] of [
  ["steward", STEWARD],
  ["approver", APPROVER],
  ["viewer", VIEWER],
] as const) {
  test(`every page opens cleanly for the ${who}`, async ({ browser }) => {
    test.setTimeout(900_000);
    const { context, page } = await signIn(browser, person, "/projects/helsinki/spaces?lang=en");
    const findings: Finding[] = [];
    // The browser logs every refused fetch as a console error of its own; one the walk excuses
    // below is the same answer said twice, so its echo is dropped once the walk is over.
    const excused = new Set<string>();
    const echoes: { finding: Finding; url: string }[] = [];
    let route = "";
    page.on("console", (message) => {
      if (message.type() !== "error") {
        return;
      }
      const finding: Finding = { who, route, kind: "console", detail: message.text().slice(0, 300) };
      if (message.text().startsWith("Failed to load resource")) {
        echoes.push({ finding, url: message.location().url });
      } else {
        findings.push(finding);
      }
    });
    page.on("pageerror", (error) => findings.push({ who, route, kind: "pageerror", detail: error.message.slice(0, 300) }));
    page.on("response", (response) => {
      const status = response.status();
      if (status >= 400 && expected(who, response)) {
        excused.add(response.url());
      }
      if (!response.url().includes("/api/") || status < 400 || expected(who, response)) {
        return;
      }
      findings.push({ who, route, kind: "http", detail: `${status} ${response.request().method()} ${new URL(response.url()).pathname}` });
    });

    for (const project of PROJECTS) {
      for (const next of await routesOf(page, project)) {
        route = next;
        await page.goto(`${next}${next.includes("?") ? "&" : "?"}lang=en`, { waitUntil: "load" });
        if (await page.getByText("Something went wrong!").count()) {
          findings.push({ who, route, kind: "error-screen", detail: (await page.locator("body").innerText()).slice(0, 300) });
        }
      }
      // The assistant opens and closes on a project page without an error.
      route = `/projects/${project}/spaces (assistant)`;
      await page.goto(`/projects/${project}/spaces?lang=en`, { waitUntil: "load" });
      const bubble = page.getByRole("button", { name: "Open the assistant" });
      if (await bubble.count()) {
        await bubble.first().click();
        await page.getByRole("button", { name: "Close the assistant" }).first().click();
      }
    }

    findings.push(...echoes.filter(({ url }) => !excused.has(url)).map(({ finding }) => finding));
    await test.info().attach(`walk-${who}.json`, { body: JSON.stringify(findings, null, 2), contentType: "application/json" });
    await context.close();
    expect(findings).toEqual([]);
  });
}

/** The list pages and the plural each lists (T-0742). */
const LISTS: [string, string][] = [
  ["spaces", "spaces"],
  ["endpoints", "endpoints"],
  ["shared", "shared"],
  ["datasources", "datasources"],
  ["pipelines", "pipelines"],
  ["dashboards", "dashboards"],
  ["apps", "apps"],
  ["syncsources", "syncsources"],
  ["csrs", "csrs"],
  ["ckan", "ckaninstances"],
  ["settings/service-accounts", "serviceaccounts"],
];

for (const [who, person] of [
  ["steward", STEWARD],
  ["viewer", VIEWER],
] as const) {
  // UI-26, UI-44 (T-2618, T-2631): a row's Edit and Remove sit in its "More actions" menu, or
  // beside it where a page has no menu; they are there for everyone, enabled for the steward and
  // disabled with the reason for the viewer.
  test(`every resource list offers Edit and Remove to the ${who} exactly as the role allows`, async ({ browser }) => {
    test.setTimeout(600_000);
    const { context, page } = await signIn(browser, person, "/projects/helsinki/spaces?lang=en");
    const missing: string[] = [];
    const hidden = await hiddenSections(page.request);
    for (const project of PROJECTS) {
      for (const [route, plural] of LISTS) {
        if (inHiddenSection(`/projects/${project}/${route}`, hidden)) {
          continue;
        }
        const listed = await page.request.get(`/api/v1/projects/${project}/${plural}`);
        const items = listed.ok() ? (((await listed.json()) as { items?: unknown[] }).items ?? []) : [];
        if (items.length === 0) {
          continue;
        }
        await page.goto(`/projects/${project}/${route}?lang=en`, { waitUntil: "load" });
        const found = await rowActions(page);
        const wrong = found.length < 2 || found.some((action) => action.enabled !== (who === "steward"));
        if (wrong) {
          const shown = found.map((action) => `${action.name} ${action.enabled ? "enabled" : "disabled"}`).join(", ") || "none";
          missing.push(`${project}/${route}: ${shown} for ${items.length} listed`);
        }
      }
    }
    await context.close();
    expect(missing).toEqual([]);
  });
}

/**
 * Edit and Remove of the first row of a list: its "More actions for …" menu opened, or the two
 * buttons of a page that shows them in the row. A disabled item carries its reason (UI-44).
 */
async function rowActions(page: Page): Promise<{ name: string; enabled: boolean }[]> {
  const main = page.locator("main");
  const more = main.getByRole("button", { name: /^More actions for / }).first();
  await more.or(main.getByRole("button", { name: /^(Edit|Remove)\b/ }).first()).first().waitFor({ timeout: 15_000 }).catch(() => undefined);
  if ((await more.count()) > 0) {
    await more.click();
    const menu = page.getByRole("menu");
    const items = menu.getByRole("menuitem", { name: /^(Edit|Remove)\b/ });
    await items.first().waitFor({ timeout: 5_000 }).catch(() => undefined);
    const found = await items.evaluateAll((nodes) =>
      nodes.map((node) => ({ name: (node.textContent ?? "").trim().split(/\s/)[0], enabled: node.getAttribute("aria-disabled") !== "true" })),
    );
    await page.keyboard.press("Escape");
    return found;
  }
  const buttons = main.getByRole("button", { name: /^(Edit|Remove)\b/ });
  return buttons.evaluateAll((nodes) =>
    nodes
      .slice(0, 2)
      .map((node) => ({ name: (node.textContent ?? "").trim(), enabled: !(node as HTMLButtonElement).disabled && node.getAttribute("aria-disabled") !== "true" })),
  );
}
