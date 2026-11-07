/**
 * A person's trash of a space's entities on dev (API/01 §31, T-3107, ADR-N-042 §3.3).
 *
 * The data views keep the person's own copy of what they delete for 30 days. This journey writes
 * only to the steward's own trash and removes it again: it keeps a copy of an entity that is not in
 * the space, finds it in the list, forgets it and sees it gone. The gateway is never asked, so no
 * entity of the space is created or deleted.
 */
import { expect, test } from "@playwright/test";
import { STEWARD, csrf, signIn } from "./portal";

const TRASH = "/api/v1/projects/helsinki/spaces/helsinki/trash";
const URN = "urn:ngsi-ld:AirQualityObserved:journey-trash";

test("a person keeps a copy of what they delete and forgets it again", async ({ browser }) => {
  const { context, page } = await signIn(browser, STEWARD, "/projects/helsinki/apps?lang=en");
  try {
    const headers = { "x-csrf-token": await csrf(context), "content-type": "application/json" };
    const notAnEntity = await page.request.post(TRASH, { headers, data: { entity: { name: "no id" } } });
    expect(notAnEntity.status(), "a body that is no NGSI-LD entity is refused").toBe(400);

    const kept = await page.request.post(TRASH, {
      headers,
      data: { entity: { id: URN, type: "AirQualityObserved", name: { type: "Property", value: "journey" } } },
    });
    expect(kept.status()).toBe(201);
    const item = (await kept.json()) as { id: number | string };

    const listed = (await (await page.request.get(TRASH)).json()) as { id: number | string }[];
    expect(listed.some((each) => String(each.id) === String(item.id))).toBe(true);

    const forgotten = await page.request.delete(`${TRASH}/${item.id}`, { headers });
    expect(forgotten.status()).toBeLessThan(300);
    const after = (await (await page.request.get(TRASH)).json()) as { id: number | string }[];
    expect(after.some((each) => String(each.id) === String(item.id))).toBe(false);
  } finally {
    await context.close();
  }
});
