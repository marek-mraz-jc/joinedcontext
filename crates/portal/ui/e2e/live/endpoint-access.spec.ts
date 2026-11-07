/**
 * A person's access request on an Endpoint, on dev (T-5615, PL-115, API/01 §48.1): the viewer
 * asks on the Endpoint's page for a read of one type it serves; the request grants nothing; the
 * viewer cannot approve it; the approver approves it as a Change the steward then approves (four
 * eyes); the viewer reads it approved; the approver revokes it, so the journey leaves no grant.
 *
 * It takes the first Helsinki Endpoint whose request form offers a type and refuses nothing.
 */
import { expect, test } from "@playwright/test";
import type { BrowserContext, Page } from "@playwright/test";
import { APPROVER, STEWARD, VIEWER, approve, csrf, signIn } from "./portal";

type Session = { context: BrowserContext; page: Page };
type Mine = {
  types: string[];
  refusal: string | null;
  items: { id: string; state: string }[];
};

const PROJECT = "helsinki";
const SUFFIX =
  process.env.E2E_SUFFIX ??
  new Date().toISOString().slice(11, 16).replace(":", "");

let viewer: Session;
let steward: Session;
let approver: Session;

test.describe.configure({ mode: "serial" });
test.setTimeout(600_000);

async function post(session: Session, path: string, data?: unknown) {
  return session.page.request.post(path, {
    headers: { "X-CSRF-Token": await csrf(session.context) },
    data: data ?? {},
  });
}

test.beforeAll(async ({ browser }) => {
  viewer = await signIn(
    browser,
    VIEWER,
    `/projects/${PROJECT}/endpoints?lang=en`,
  );
  steward = await signIn(
    browser,
    STEWARD,
    `/projects/${PROJECT}/endpoints?lang=en`,
  );
  approver = await signIn(
    browser,
    APPROVER,
    `/projects/${PROJECT}/endpoints?lang=en`,
  );
});

test.afterAll(async () => {
  for (const session of [viewer, steward, approver])
    await session?.context.close();
});

test("a viewer asks on the Endpoint's page, an owner approves it and revokes it", async () => {
  const endpoints = (
    (await (
      await viewer.page.request.get(`/api/v1/projects/${PROJECT}/endpoints`)
    ).json()) as { items: { metadata: { name: string } }[] }
  ).items;
  let found: { name: string; mine: Mine } | undefined;
  for (const endpoint of endpoints) {
    const answer = await viewer.page.request.get(
      `/api/v1/projects/${PROJECT}/endpoints/${endpoint.metadata.name}/access-requests`,
    );
    if (!answer.ok()) continue;
    const mine = (await answer.json()) as Mine;
    const open = mine.items.some(
      (item) => item.state === "pending" || item.state === "approved",
    );
    if (mine.types.length > 0 && !mine.refusal && !open) {
      found = { name: endpoint.metadata.name, mine };
      break;
    }
  }
  expect(
    found,
    "a Helsinki Endpoint the viewer may ask a read of",
  ).toBeDefined();
  const requests = `/api/v1/projects/${PROJECT}/endpoints/${found!.name}/access-requests`;

  // A field the Endpoint does not serve is refused at the field, before anything is stored.
  const bad = await post(viewer, requests, {
    types: ["NoSuchTypeT5615"],
    reason: "live journey",
  });
  expect(bad.status(), await bad.text()).toBe(422);

  // The form on the page: the viewer picks the type and says why.
  await viewer.page.goto(
    `/projects/${PROJECT}/endpoints/${found!.name}?lang=en`,
  );
  const section = viewer.page.getByTestId("endpoint-access-request");
  const types = section.getByRole("combobox", { name: "Entity types" });
  await types.click();
  await viewer.page
    .getByRole("option", { name: found!.mine.types[0] })
    .first()
    .click();
  await types.press("Escape");
  await section.getByLabel(/^Reason/).fill(`T-5615 live journey ${SUFFIX}`);
  await section.getByRole("button", { name: "Send request" }).click();
  await expect(section.getByText("Waiting for the owner")).toBeVisible();
  const mine = (await (await viewer.page.request.get(requests)).json()) as Mine;
  const id = mine.items.find((item) => item.state === "pending")?.id ?? "";
  expect(id, "the request is pending").not.toBe("");

  const own = await post(
    viewer,
    `/api/v1/projects/${PROJECT}/access-requests/${id}/approve`,
  );
  expect([403, 404], await own.text()).toContain(own.status());

  const granted = await post(
    approver,
    `/api/v1/projects/${PROJECT}/access-requests/${id}/approve`,
  );
  expect(granted.status(), await granted.text()).toBe(202);
  const change =
    ((await granted.json()) as { metadata?: { name?: string } }).metadata
      ?.name ?? "";
  await approve(steward.page, PROJECT, change);
  const after = (await (
    await viewer.page.request.get(requests)
  ).json()) as Mine;
  expect(after.items.find((item) => item.id === id)?.state).toBe("approved");

  await expect
    .poll(
      async () =>
        (
          await post(
            approver,
            `/api/v1/projects/${PROJECT}/access-requests/${id}/revoke`,
            { reason: `T-5615 ${SUFFIX} done` },
          )
        ).status(),
      {
        timeout: 300_000,
        intervals: [10_000],
        message: "the grant reaches main and the revoke proposes its end",
      },
    )
    .toBe(202);
});
