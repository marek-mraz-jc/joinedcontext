import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";

// `vite preview` serves the built bundle with no portal API behind it, so this journey answers
// the API from the browser. The clicks, the routing and the rendering are the real ones; only
// the Rust process is absent (its own journey arrives with the ci-full task that runs the binary).
// The project is `helsinki`: the root route redirects to the first project `GET /api/v1/projects`
// lists (PF-05), so a journey that starts at `/` lands there once the stub answers that list.

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  email: "jana.kovacova@banskabystrica.sk",
  roles: ["portal-approver"],
};

const PROJECTS = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ProjectList",
  items: [{ name: "helsinki" }],
};

const PROPOSAL = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ChangeProposal",
  metadata: { name: "chg-1a2b3c4d", namespace: "helsinki" },
  summary: {
    key: "change.summary.update",
    params: { kind: "Endpoint", name: "air-quality", fields: 2 },
  },
  author: { name: "Marek Mráz", email: "marek@banskabystrica.sk" },
  createdAt: "2026-03-03T12:00:00Z",
  status: {
    lane: "yellow",
    phase: "PendingApproval",
    plan: { update: 1 },
    mergeRequest: "https://gitea.example/city/city-config/pulls/7",
  },
  planFields: [
    { path: "spec.audience", from: "public", to: "internal" },
    { path: "spec.credentials.token", from: "[REDACTED]", to: "[REDACTED]" },
  ],
};

const APPROVED = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: PROPOSAL.metadata,
  status: { lane: "yellow", phase: "Deploying", plan: { update: 1 } },
};

async function stubApi(page: Page): Promise<{ approvals: string[] }> {
  const approvals: string[] = [];
  await page.route("**/api/v1/**", async (route) => {
    const url = new URL(route.request().url());
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });

    if (url.pathname.endsWith("/auth/me")) {
      return json(IDENTITY);
    }
    if (url.pathname.endsWith("/approve")) {
      approvals.push(url.pathname);
      return json(APPROVED, 202);
    }
    if (url.pathname === "/api/v1/projects") {
      return json(PROJECTS);
    }
    if (url.pathname.endsWith("/changes")) {
      return json({
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "ChangeList",
        items: [PROPOSAL],
      });
    }
    if (url.pathname.endsWith(`/changes/${PROPOSAL.metadata.name}`)) {
      return json(PROPOSAL);
    }
    return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
  });
  return { approvals };
}

test.describe("approvals", () => {
  test("an approver reviews a change and approves it", async ({ page }) => {
    const { approvals } = await stubApi(page);

    await page.goto("/?lang=en");
    await page.getByRole("navigation", { name: "Main navigation" }).getByRole("link", {
      name: "Approvals",
    }).click();

    await expect(page).toHaveURL(/\/projects\/helsinki\/approvals$/);
    const summary = 'Update Endpoint "air-quality" (2 fields changed)';
    await expect(page.getByRole("link", { name: summary })).toBeVisible();
    // `exact`, because the lane word is also inside the glossary tooltip the badge carries
    // since T-1609 — without it the locator matches the badge and the definition (T-2409).
    await expect(page.getByText("Yellow", { exact: true })).toBeVisible();
    await expect(page.getByText("Marek Mráz")).toBeVisible();

    await page.getByRole("link", { name: summary }).click();

    await expect(page).toHaveURL(/\/approvals\/chg-1a2b3c4d$/);
    await expect(page.getByRole("heading", { name: summary })).toBeVisible();
    // The field in words (its path only on hover), the value as text (T-1385, T-4991).
    await expect(page.getByRole("cell", { name: "Audience", exact: true })).toBeVisible();
    await expect(page.getByRole("cell", { name: "internal", exact: true })).toBeVisible();
    // The token appears only as the redaction marker, never as its value.
    await expect(page.getByText("Redacted").first()).toBeVisible();

    await page.getByRole("button", { name: "Approve" }).click();

    await expect(page.getByText("Deploying")).toBeVisible();
    expect(approvals).toEqual(["/api/v1/projects/helsinki/changes/chg-1a2b3c4d/approve"]);
  });

  test("at 400 px a long merge-request link and file path wrap inside the page (T-4976)", async ({ page }) => {
    await page.setViewportSize({ width: 400, height: 900 });
    // The walker found 101-126 px of sideways scroll on dev: Gitea's login-redirect link and a
    // policy path are each one unbroken word wider than a phone.
    const long = {
      ...PROPOSAL,
      status: {
        ...PROPOSAL.status,
        mergeRequest:
          "https://dev.joinedcontext.com/git/user/oauth2/keycloak?redirect_to=%2Fgit%2Fjoinedcontext%2Fhelsinki%2Fpulls%2F185",
      },
      files: [
        { path: "projects/helsinki/spaces/helsinki/policies/t4843-hro7z1-write.yaml", kind: "Policy", lane: "red", operation: "Create" },
        { path: "projects/helsinki/spaces/helsinki/endpoints/air-quality-measurements-v2.yaml", kind: "Endpoint", lane: "yellow", operation: "Update", fields: PROPOSAL.planFields },
      ],
    };
    await stubApi(page);
    await page.route(`**/changes/${PROPOSAL.metadata.name}`, (route) =>
      route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(long) }),
    );

    await page.goto(`/projects/helsinki/approvals/${PROPOSAL.metadata.name}?lang=en`);
    await expect(page.getByRole("button", { name: "Approve" })).toBeVisible();
    await expect(page.getByTestId("change-file")).toHaveCount(2);
    const sideways = await page.evaluate(
      () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
    );
    expect(sideways).toBe(0);
    await page.screenshot({ path: test.info().outputPath("approval-detail-400.png"), fullPage: true });
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.screenshot({ path: test.info().outputPath("approval-detail-1440.png"), fullPage: true });
  });

  test("the proposer withdraws their own model change, at 400 and 1440 px (T-6535)", async ({ page }) => {
    const own = {
      ...PROPOSAL,
      author: { name: IDENTITY.name, email: IDENTITY.email },
      summary: { key: "change.summary.update", params: { kind: "DataModel", name: "air", fields: 1 } },
      planFields: [{ path: "spec.version", from: "1.0.0", to: "1.0.1" }],
      files: [{ path: "projects/helsinki/spaces/air/datamodels/air.yaml", kind: "DataModel", lane: "yellow", operation: "Update" }],
    };
    const withdrawn = { ...APPROVED, status: { lane: "yellow", phase: "Rejected", plan: { update: 1 } } };
    const rejects: string[] = [];
    await stubApi(page);
    await page.route(`**/changes/${PROPOSAL.metadata.name}`, (route) =>
      route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(own) }),
    );
    await page.route(`**/changes/${PROPOSAL.metadata.name}/reject`, (route) => {
      rejects.push(route.request().postData() ?? "");
      return route.fulfill({ status: 202, contentType: "application/json", body: JSON.stringify(withdrawn) });
    });
    const shot = (name: string) => page.screenshot({ path: test.info().outputPath(name), fullPage: true });
    for (const width of [1440, 400]) {
      await page.setViewportSize({ width, height: 900 });
      await page.goto(`/projects/helsinki/approvals/${PROPOSAL.metadata.name}?lang=en`);
      await expect(page.getByRole("button", { name: "Approve" })).toHaveAttribute("aria-disabled", "true");
      await expect(page.getByRole("button", { name: "Reject" })).toHaveCount(0);
      await shot(`withdraw-pending-${width}.png`);
      await page.getByRole("button", { name: "Withdraw" }).click();
      const dialog = page.getByRole("dialog", { name: "Withdraw your change?" });
      await dialog.getByLabel("Why are you withdrawing it?").fill("Proposed the wrong model version");
      await shot(`withdraw-dialog-${width}.png`);
      await dialog.getByRole("button", { name: "Withdraw the change" }).click();
      await expect(page.getByText("You withdrew this change.", { exact: false })).toBeVisible();
      await shot(`withdraw-done-${width}.png`);
    }
    expect(rejects.map((body) => JSON.parse(body))).toEqual([
      { reason: "Proposed the wrong model version" },
      { reason: "Proposed the wrong model version" },
    ]);
  });

  test("an approver sees a change came through an AI client over MCP, at 400 and 1440 px (T-5609)", async ({ page }) => {
    const viaMcp = { ...PROPOSAL, status: { ...PROPOSAL.status, proposedVia: { channel: "mcp", client: "mcp-mobile" } } };
    await stubApi(page);
    await page.route(`**/changes/${PROPOSAL.metadata.name}`, (route) =>
      route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(viaMcp) }),
    );
    for (const width of [400, 1440]) {
      await page.setViewportSize({ width, height: 900 });
      await page.goto(`/projects/helsinki/approvals/${PROPOSAL.metadata.name}?lang=en`);
      await expect(page.getByTestId("change-proposed-via")).toHaveText("Proposed through mcp-mobile (MCP)");
      await expect(page.getByText("An AI client (mcp-mobile) wrote this proposal over MCP")).toBeVisible();
      const sideways = await page.evaluate(
        () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
      );
      expect(sideways).toBe(0);
      await page.screenshot({ path: test.info().outputPath(`approval-via-mcp-${width}.png`), fullPage: true });
    }
  });

  test("an approver sees the connector and the plan a change came through, at 400 and 1440 px (T-5973)", async ({ page }) => {
    const viaConnector = {
      ...PROPOSAL,
      status: {
        ...PROPOSAL.status,
        proposedVia: {
          channel: "mcp",
          client: "claude-code",
          connector: "abcdefghijklmnopqrstuvwxyz",
          connectorName: "doprava-in-claude",
          plan: "p2345234523452345234523452",
        },
      },
    };
    await stubApi(page);
    await page.route(`**/changes/${PROPOSAL.metadata.name}`, (route) =>
      route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(viaConnector) }),
    );
    for (const width of [400, 1440]) {
      await page.setViewportSize({ width, height: 900 });
      await page.goto(`/projects/helsinki/approvals/${PROPOSAL.metadata.name}?lang=en`);
      await expect(page.getByTestId("change-proposed-via")).toHaveText("Via Claude Code connector doprava-in-claude");
      await expect(page.getByTestId("change-proposed-plan")).toContainText("Plan p2345234523452345234523452");
      const sideways = await page.evaluate(
        () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
      );
      expect(sideways).toBe(0);
      await page.screenshot({ path: test.info().outputPath(`approval-via-connector-${width}.png`), fullPage: true });
    }
  });

  test("at 375 px a create's field table fits the screen and shows labels, not spec paths (T-4991)", async ({ page }) => {
    await page.setViewportSize({ width: 375, height: 900 });
    const create = {
      ...PROPOSAL,
      summary: { key: "change.summary.create", params: { kind: "KnowledgeSource", name: "t4869-files-mvhnojg5", fields: 6 } },
      planFields: [
        { path: "metadata.name", to: "t4869-files-mvhnojg5" },
        { path: "metadata.namespace", to: "helsinki" },
        { path: "spec.maxPages", to: 1000 },
        { path: "spec.pdf.maxBytes", to: 52428800 },
        { path: "spec.pdf.maxPages", to: 500 },
        { path: "spec.source", to: "https://www.hel.fi/static/liitteet-2019/Kaupunginkanslia/Rap/kaupunkiymparisto.pdf" },
      ],
    };
    await stubApi(page);
    await page.route(`**/changes/${PROPOSAL.metadata.name}`, (route) =>
      route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(create) }),
    );
    await page.goto(`/projects/helsinki/approvals/${PROPOSAL.metadata.name}?lang=en`);
    const table = page.getByRole("table", { name: "Change type" });
    await expect(table.getByText("Pdf › Max pages")).toBeVisible();
    await expect(page.getByText("spec.pdf.maxPages", { exact: true })).toHaveCount(0);
    await expect(table.getByRole("columnheader", { name: "Before" })).toHaveCount(0);
    const inside = await table.evaluate((el) => {
      const box = el.closest(".overflow-x-auto") ?? el;
      return box.scrollWidth - box.clientWidth;
    });
    expect(inside).toBe(0);
    const sideways = await page.evaluate(
      () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
    );
    expect(sideways).toBe(0);
    await page.screenshot({ path: test.info().outputPath("approval-create-375.png"), fullPage: true });
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.screenshot({ path: test.info().outputPath("approval-create-1440.png"), fullPage: true });
  });

  // T-5114: a steward finds who changed a Policy, filters the history and takes it away as CSV.
  for (const width of [1440, 400]) {
    test(`a steward exports the change history as CSV at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      const policy = {
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "Change",
        metadata: { name: "chg-0000002a", namespace: "helsinki" },
        summary: { key: "change.summary.update", params: { kind: "Policy", name: "air-quality-write" } },
        author: { name: "Marek Mráz" },
        createdAt: "2026-10-20T08:00:00Z",
        status: { lane: "red", phase: "Rejected", plan: {}, mergeRequest: "https://gitea.example/city/city-config/pulls/42" },
        decision: { by: "jana.kovacova@banskabystrica.sk", at: "2026-10-20T09:30:00Z", reason: "Writes must stay with the steward group" },
      };
      const asked: string[] = [];
      await stubApi(page);
      await page.route("**/changes/history**", (route) => {
        const url = new URL(route.request().url());
        asked.push(`${url.pathname.split("/").pop()}${url.search}`);
        if (url.pathname.endsWith("/export")) {
          return route.fulfill({
            status: 200,
            headers: { "content-type": "text/csv; charset=utf-8" },
            body: "closedAt,change,outcome,kind,name\r\n2026-10-20T09:30:00Z,chg-0000002a,Rejected,Policy,air-quality-write\r\n",
          });
        }
        return route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ChangeList", items: [policy] }),
        });
      });
      await page.goto("/projects/helsinki/approvals?lang=en");
      await page.getByRole("tab", { name: "History" }).click();
      await expect(page.getByText("Reason: Writes must stay with the steward group")).toBeVisible();
      await page.getByLabel("Name contains").fill("air");
      await page.getByRole("button", { name: "Filter", exact: true }).click();
      await expect.poll(() => asked).toContain("history?page=1&name=air");
      const download = page.waitForEvent("download");
      await page.getByRole("button", { name: "Export CSV" }).click();
      const file = await download;
      expect(file.suggestedFilename()).toBe("helsinki-changes.csv");
      expect(asked).toContain("export?name=air");
      const sideways = await page.evaluate(
        () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
      );
      expect(sideways).toBe(0);
      expect(await axeViolations(page)).toEqual([]);
      await page.screenshot({ path: test.info().outputPath(`approvals-history-export-${width}.png`), fullPage: true });
    });
  }

  test("the approvals routes have no axe violations", async ({ page }) => {
    await stubApi(page);

    await page.goto("/projects/helsinki/approvals?lang=sk");
    await expect(page.getByRole("table")).toBeVisible();
    expect(await axeViolations(page)).toEqual([]);

    await page.goto("/projects/helsinki/approvals/chg-1a2b3c4d?lang=sk");
    await expect(page.getByRole("button", { name: "Schváliť" })).toBeVisible();
    expect(await axeViolations(page)).toEqual([]);
  });
});
