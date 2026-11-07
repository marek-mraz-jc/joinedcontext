/**
 * A viewer is refused with a reason on every page, on dev (T-1584; UI-44, PF-50).
 *
 * `ui/e2e/live/walk.spec.ts` proves a viewer's pages *open*, and asserts that no Edit or Delete is
 * offered — absence, not a refusal a person can read. `joinedcontext-conformance/e2e/journeys/16-role-permissions.spec.ts`
 * checks two controls on two pages. Server-side the whole matrix is proved
 * (`tests/roles_matrix_tests.rs::every_role_meets_every_route_and_operation_exactly_as_the_table_says`).
 * What was proved nowhere: that on **every** page of the navigation, every control a viewer may not
 * use carries the reason, and that the same calls made by hand answer 403.
 *
 * The rule this holds the UI to (`PermissionGuard`, UI-44): a control the person may not use stays
 * where it is, disabled, and says which verb on which kind their role lacks — by pointer, in the
 * wrapper's `title`, and by keyboard, in the element `aria-describedby` names. A control that is
 * simply absent is not a refusal: the person cannot tell it from a page that does not have it.
 *
 * The walk found one page whose write controls went through no guard at all — the assistant's
 * `Start`, `Continue` and the agent profile's `Edit`, disabled by an empty form and nothing else.
 * That is fixed in the same commit, with `tests/assistant_permissions.test.tsx` holding it.
 *
 * Nothing is written, so nothing is cleaned up: a viewer cannot write.
 *
 * One sign-in for both halves, on purpose: the walk over every page costs a few hundred requests
 * from one address, and the Portal's edge routes take 300 a minute per address (`limit-count`,
 * `components/portal/apisix-plugins.yaml`). A second OIDC login right behind the walk met a 429 and
 * failed the test for the edge's reason instead of the page's. The limit is doing its job; the
 * journey fits inside it.
 */
import { expect, test } from "@playwright/test";
import type { Locator, Page } from "@playwright/test";
import { VIEWER, csrf, signIn } from "./portal";

const PROJECT = "helsinki";

/** The words a control carries when a viewer may not use it: the verb and the kind, or a lane. */
const REASON = /permit|role|approve|propose|delete|not allowed|no binding/i;

test.setTimeout(1_800_000);

/** The reason a control carries, read the way a screen reader reads it. */
async function describedBy(control: Locator): Promise<string> {
  const id = await control.getAttribute("aria-describedby");
  if (!id) {
    return "";
  }
  return (await control.page().locator(`[id="${id}"]`).first().textContent()) ?? "";
}

/** The reason a pointer gets: the guard's focusable wrapper carries it as a native tooltip. */
async function titled(control: Locator): Promise<string> {
  const own = await control.getAttribute("title");
  if (own) {
    return own;
  }
  const wrapper = control.locator("xpath=ancestor::span[@title][1]");
  return (await wrapper.count()) ? ((await wrapper.first().getAttribute("title")) ?? "") : "";
}

/** Every route of the main navigation, as the viewer's own nav lists them. */
async function routes(page: Page): Promise<string[]> {
  const nav = page.getByRole("navigation", { name: "Main navigation" });
  const hrefs = await nav
    .locator("a[href^='/']")
    .evaluateAll((links) =>
      links.map((link) => (link as HTMLAnchorElement).getAttribute("href") ?? ""),
    );
  return [...new Set(hrefs.filter((href) => href.startsWith(`/projects/${PROJECT}`)))];
}

test("a viewer is refused with a reason on every page, and at the door", async ({ browser }) => {
  const { context, page } = await signIn(browser, VIEWER, `/projects/${PROJECT}/spaces?lang=en`);
  const offered: string[] = [];
  const mute: string[] = [];
  try {
    const pages = await routes(page);
    expect(pages.length, "the viewer's navigation lists the project's pages").toBeGreaterThan(4);

    for (const route of pages) {
      await page.goto(`${route}?lang=en`, { waitUntil: "load" });
      // Every control that changes something, by the words the pages use for them.
      // The verb, then a word break: a list of model classes holds buttons named
      // `StopCommandMessage`, which is a type and not a control (found on the models page).
      const controls = page.getByRole("button", {
        name: /^(New|Grant|Approve|Reject|Delete|Edit|Remove|Propose|Try it|Start|Stop|Bring back|Rotate|Revoke)(\b|$)/i,
      });
      const count = await controls.count();
      for (let index = 0; index < count; index += 1) {
        const control = controls.nth(index);
        if (!(await control.isVisible())) {
          continue;
        }
        const label = ((await control.textContent()) ?? "").trim().slice(0, 40);
        if (await control.isEnabled()) {
          // A viewer holds no verb in this project, so an enabled control that changes something
          // is either a refusal the page will only give after the click, or a real hole.
          offered.push(`${route} → ${label}`);
          continue;
        }
        const reason = `${await describedBy(control)} ${await titled(control)}`.trim();
        if (!REASON.test(reason)) {
          mute.push(`${route} → ${label} (${reason || "no reason at all"})`);
        }
      }
    }

    test.info().annotations.push({
      type: "viewer",
      description: `${pages.length} pages; offered: ${offered.join("; ") || "none"}; mute: ${mute.join("; ") || "none"}`,
    });
    expect(mute, "every control a viewer may not use names the verb and the kind (UI-44)").toEqual(
      [],
    );
    expect(offered, "no control that changes something is offered to a viewer").toEqual([]);

    // And the same calls made by hand are refused at the door: a disabled button is a courtesy,
    // the 403 is the control. A page that only hid the button would pass the half above.
    const token = await csrf(context);
    const manifest = (kind: string, plural: string) => ({
      plural,
      body: {
        apiVersion: "joinedcontext.com/v1alpha1",
        kind,
        metadata: { name: `t1584-${Date.now().toString(36)}`, namespace: PROJECT },
        spec: {},
      },
    });
    for (const { plural, body } of [
      manifest("ContextSpace", "spaces"),
      manifest("Endpoint", "endpoints"),
      manifest("DataSource", "datasources"),
      manifest("Pipeline", "pipelines"),
      manifest("Dashboard", "dashboards"),
    ]) {
      const answer = await page.request.post(`/api/v1/projects/${PROJECT}/${plural}`, {
        headers: { "x-csrf-token": token },
        data: body,
      });
      // 403 for the verb the viewer lacks. Never 202, and never 500: a refusal is an answer.
      expect(answer.status(), `${plural}: ${await answer.text()}`).toBe(403);
    }

    // Reading is what a viewer may do, and it still works: a refusal is not a wall.
    for (const plural of ["spaces", "endpoints", "datasources", "pipelines", "dashboards"]) {
      const answer = await page.request.get(`/api/v1/projects/${PROJECT}/${plural}`);
      expect(answer.status(), `reading ${plural}`).toBe(200);
    }
  } finally {
    await context.close();
  }
});
