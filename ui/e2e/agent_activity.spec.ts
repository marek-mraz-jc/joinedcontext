import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { axeViolations } from "./axe";

// T-4851, AG-134…AG-136: an agent's Activity tab in a real browser at 1440 and 400, the API
// answered in the browser as the other mocked specs do: runs with their steps, the alerts, Stop
// behind a confirmation, the empty tab and the refusal. AGENTS_SHOTS names a directory for the
// screenshots the task attaches.

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "ida.admin",
  name: "Ida Admin",
  email: "ida@hel.fi",
  roles: [],
};
const LIST = (items: unknown[]) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items,
});
const ADMIN = {
  project: "org",
  bootstrap: false,
  grants: [
    {
      role: "org-admin",
      binding: "admins",
      rule: {
        kinds: ["AgentProfile", "App"],
        verbs: ["read", "propose", "approve"],
      },
    },
  ],
};
const PROFILE = {
  kind: "AgentProfile",
  name: "app-builder",
  project: "org",
  title: null,
  owner: { type: "project", name: "org" },
  actsAs: "caller",
  spaces: [],
  endpoints: [],
  lastActivity: "2026-10-21T02:12:00Z",
  spend: { used: 182400, cap: null, per: "day" },
  state: "active",
  runId: null,
  profile: null,
};
const RUNS = [
  {
    id: "run-7f3a",
    project: "helsinki",
    profile: "app-builder",
    title: "Air quality dashboard",
    startedBy: "jana.kovacova@hel.fi",
    status: "building",
    createdAt: "2026-10-21T02:10:00Z",
    spaces: ["air"],
    tokens: 341000,
    costUsd: 0.4182,
    refusals: 1,
    writes: 1,
    mayStop: true,
    steps: [
      {
        at: "2026-10-21T02:10:04Z",
        tool: "jc_resource_list",
        target: "Endpoint",
        verdict: "ok",
        write: false,
        durationMs: 41,
      },
      {
        at: "2026-10-21T02:10:09Z",
        tool: "jc_endpoint_propose",
        target: "air-quality",
        verdict: "refused",
        write: true,
        reason:
          "'jc_endpoint_propose' opens a Change of its own, which the assistant never does: the person proposes it from the form",
      },
      {
        at: "2026-10-21T02:11:30Z",
        tool: "jc_resource_propose",
        target: "air-dashboard",
        verdict: "ok",
        write: true,
        reason: "the dashboard reads the new PM2.5 attribute",
      },
    ],
  },
  {
    id: "run-61bd",
    project: "espoo",
    profile: "app-builder",
    title: "Bike stations map",
    startedBy: "ida@hel.fi",
    status: "published",
    createdAt: "2026-10-20T14:02:00Z",
    finishedAt: "2026-10-20T14:09:12Z",
    durationMs: 432000,
    spaces: ["mobility"],
    tokens: 128500,
    costUsd: 0.1311,
    refusals: 0,
    writes: 0,
    mayStop: false,
    steps: [
      {
        at: "2026-10-20T14:02:10Z",
        tool: "jc_resource_list",
        target: "Endpoint",
        verdict: "ok",
        write: false,
        durationMs: 22,
      },
    ],
  },
];
const ALERTS = [
  {
    kind: "spendShare",
    project: "helsinki",
    profile: "app-builder",
    runId: "run-7f3a",
    runTitle: "Air quality dashboard",
    count: 85,
    at: "2026-10-21T02:10:00Z",
  },
  {
    kind: "offHoursWrite",
    project: "helsinki",
    profile: "app-builder",
    runId: "run-7f3a",
    runTitle: "Air quality dashboard",
    count: 1,
    at: "2026-10-21T02:11:30Z",
  },
];

async function stub(
  page: Page,
  activity: {
    status: number;
    runs: unknown[];
    alerts: unknown[];
    alertProjects?: string[];
  },
): Promise<{ cancels: string[]; asked: string[]; subscribed: unknown[] }> {
  const cancels: string[] = [];
  const asked: string[] = [];
  const subscribed: unknown[] = [];
  await page.route("**/api/v1/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({
        status,
        contentType: "application/json",
        body: JSON.stringify(body),
      });
    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path.endsWith("/permissions/me")) return json(ADMIN);
    if (path === "/api/v1/projects")
      return json(LIST([{ name: "helsinki" }, { name: "espoo" }]));
    if (path === "/api/v1/organization/agents")
      return json({ items: [PROFILE] });
    if (path === "/api/v1/organization/agents/activity") {
      asked.push(request.url());
      return activity.status === 200
        ? json({
            runs: activity.runs,
            alerts: activity.alerts,
            choices: {
              people: ["jana.kovacova@hel.fi", "peter.novak@hel.fi"],
              spaces: ["air-quality"],
              alertProjects: activity.alertProjects ?? ["espoo", "helsinki"],
            },
          })
        : json(
            {
              type: "about:blank",
              title: "Forbidden",
              status: 403,
              detail:
                "Your roles read no project, so no agent run is yours to see. Ask a steward of the project or an organization administrator to grant read on it.",
            },
            403,
          );
    }
    if (path === "/api/v1/alerts" && request.method() === "GET")
      return json({
        items: subscribed.map((body) => ({ id: 7, mutedUntil: null, ...(body as object) })),
      });
    if (path === "/api/v1/alerts" && request.method() === "PUT") {
      const body = request.postDataJSON() as object;
      subscribed.push(body);
      return json({ id: 7, mutedUntil: null, ...body });
    }
    if (
      path === "/api/v1/projects/helsinki/agent-runs/run-7f3a/cancel" &&
      request.method() === "POST"
    ) {
      cancels.push(path);
      return json({ ...(RUNS[0] as object), status: "cancelled" });
    }
    return json(LIST([]));
  });
  return { cancels, asked, subscribed };
}

const shots = process.env.AGENTS_SHOTS;
async function shoot(page: Page, name: string, width: number): Promise<void> {
  if (!shots) return;
  mkdirSync(shots, { recursive: true });
  await page.screenshot({
    path: join(shots, `activity-${name}-${width}.png`),
    fullPage: true,
  });
}

async function openActivity(page: Page): Promise<void> {
  await page.goto("/organization/agents/org/AgentProfile/app-builder");
  await page.getByRole("tab", { name: "Activity" }).click();
}

for (const width of [1440, 400]) {
  test.describe(`an agent's Activity tab at ${width}`, () => {
    test.use({ viewport: { width, height: width === 400 ? 860 : 900 } });

    test("shows the runs, their steps and the alerts, and stops a run after a confirmation", async ({
      page,
    }) => {
      const { cancels, asked } = await stub(page, {
        status: 200,
        runs: RUNS,
        alerts: ALERTS,
      });
      await openActivity(page);
      await expect(
        page.getByRole("heading", { name: "Air quality dashboard" }),
      ).toBeVisible();
      await expect(
        page.getByText(
          "Run “Air quality dashboard” of app-builder has used 85 % of its token cap.",
        ),
      ).toBeVisible();
      await page.getByText("Tool calls (3)").click();
      await expect(
        page.getByText("Reason: the dashboard reads the new PM2.5 attribute"),
      ).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
      const overflow = await page.evaluate(
        () => document.documentElement.scrollWidth > window.innerWidth,
      );
      expect(overflow).toBe(false);
      await shoot(page, "filled", width);
      // The person and space filters offer only the values the server sent.
      const person = page.getByLabel("Started by");
      await expect(person.locator("option")).toHaveText([
        "Anyone",
        "jana.kovacova@hel.fi",
        "peter.novak@hel.fi",
      ]);
      await person.selectOption("peter.novak@hel.fi");
      await page.getByLabel("Context Space").selectOption("air-quality");
      await page.getByRole("button", { name: "Apply" }).click();
      await expect
        .poll(() => {
          const query = new URL(asked.at(-1) ?? "http://x/").searchParams;
          return `${query.get("person")} ${query.get("space")}`;
        })
        .toBe("peter.novak@hel.fi air-quality");
      await page.getByRole("button", { name: "Clear filters" }).click();
      await page.getByRole("button", { name: "Stop", exact: true }).click();
      await expect(page.getByRole("alertdialog")).toBeVisible();
      await shoot(page, "stop", width);
      await page.getByRole("button", { name: "Stop the run" }).click();
      await expect.poll(() => cancels.length).toBe(1);
    });

    test("an approver asks for the agent's alerts in a project the server offered (T-5301)", async ({
      page,
    }) => {
      const { subscribed } = await stub(page, {
        status: 200,
        runs: RUNS,
        alerts: ALERTS,
      });
      await openActivity(page);
      await page.getByRole("button", { name: "Alert me…" }).click();
      const dialog = page.getByRole("dialog");
      await expect(dialog.getByLabel("Project")).toBeVisible();
      await dialog.getByLabel("Project").selectOption("helsinki");
      await expect(
        dialog.getByLabel("it has 5 or more tool calls refused within an hour"),
      ).toBeChecked();
      expect(await axeViolations(page)).toEqual([]);
      await shoot(page, "alert-dialog", width);
      await dialog.getByRole("button", { name: "Alert me" }).click();
      await expect.poll(() => subscribed.length).toBe(1);
      expect(subscribed[0]).toEqual({
        project: "helsinki",
        scope: "agent",
        target: "app-builder",
        events: ["refusalSpike", "spendShare", "offHoursWrite"],
        delivery: "portal",
      });
      await expect(dialog.getByRole("button", { name: "Save" })).toBeVisible();
      await shoot(page, "alert-subscribed", width);
      await page.unrouteAll();
      await stub(page, { status: 200, runs: RUNS, alerts: [], alertProjects: [] });
      await openActivity(page);
      const none = page.getByRole("button", { name: "Alert me…" });
      await expect(none).toHaveAttribute("aria-disabled", "true");
      await shoot(page, "alert-none", width);
    });

    test("says when no run is there and when the person reads no project", async ({
      page,
    }) => {
      await stub(page, { status: 200, runs: [], alerts: [] });
      await openActivity(page);
      await expect(
        page.getByText("No run of this agent that you may see"),
      ).toBeVisible();
      await shoot(page, "empty", width);
      await page.unrouteAll();
      await stub(page, { status: 403, runs: [], alerts: [] });
      await openActivity(page);
      await expect(page.getByText(/Your roles read no project/)).toBeVisible();
      await shoot(page, "refused", width);
    });
  });
}
