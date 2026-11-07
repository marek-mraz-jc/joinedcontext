/**
 * A person's saved data views of a space on dev (API/01, T-3104, ADR-N-042 §3.2).
 *
 * This journey saves one personal view, which only its owner sees, changes it against its
 * version, is refused a change against a stale version, deletes it and finds it gone. A `finally`
 * deletes it again if a step failed, so dev keeps no view of the journey. No entity is written.
 */
import { expect, test } from "@playwright/test";
import { STEWARD, csrf, signIn } from "./portal";

const VIEWS = "/api/v1/projects/helsinki/spaces/helsinki/views";

test("a person saves a view, changes it against its version and deletes it again", async ({ browser }) => {
  const { context, page } = await signIn(browser, STEWARD, "/projects/helsinki/apps?lang=en");
  let id: string | undefined;
  try {
    const headers = { "x-csrf-token": await csrf(context), "content-type": "application/json" };
    const unknownKey = await page.request.post(VIEWS, {
      headers,
      data: { type: "AirQualityObserved", kind: "grid", mode: "personal", title: "journey", colour: "red" },
    });
    expect(unknownKey.status(), "a key the view does not have is refused").toBe(400);

    const saved = await page.request.post(VIEWS, {
      headers,
      data: { type: "AirQualityObserved", kind: "grid", mode: "personal", title: "Journey view", config: { q: "pm10>40" } },
    });
    expect(saved.status()).toBe(201);
    const view = (await saved.json()) as { id: string; version: number };
    id = view.id;

    const listed = (await (await page.request.get(VIEWS)).json()) as { items: { id: string }[] };
    expect(listed.items.some((each) => each.id === id)).toBe(true);

    const changed = await page.request.put(`${VIEWS}/${id}`, {
      headers,
      data: { kind: "grid", mode: "personal", title: "Journey view, renamed", config: { q: "pm10>50" }, expectedVersion: view.version },
    });
    expect(changed.status()).toBe(200);
    const stale = await page.request.put(`${VIEWS}/${id}`, {
      headers,
      data: { kind: "grid", mode: "personal", title: "Journey view, stale", config: {}, expectedVersion: view.version },
    });
    expect(stale.status(), "a save against a newer view is refused").toBe(409);

    const deleted = await page.request.delete(`${VIEWS}/${id}`, { headers });
    expect(deleted.status()).toBeLessThan(300);
    id = undefined;
    const gone = await page.request.get(`${VIEWS}/${view.id}`);
    expect(gone.status()).toBe(404);
  } finally {
    if (id !== undefined) {
      await page.request.delete(`${VIEWS}/${id}`, { headers: { "x-csrf-token": await csrf(context) } });
    }
    await context.close();
  }
});
