/**
 * Comments on a space's entities and the notifications their mentions send, on dev (API/01 §35,
 * T-3106, ADR-N-042 §3.1).
 *
 * The steward comments on an entity URN of the helsinki space and mentions the editor, who reads
 * the space: the editor is notified once and marks it read, a stranger's mention notifies nobody,
 * the editor cannot remove the steward's comment, and the steward removes it, which takes the
 * notification with it. Only the Portal's own records are written; no entity of the space changes.
 */
import { expect, test } from "@playwright/test";
import { EDITOR, STEWARD, csrf, signIn } from "./portal";

const COMMENTS = "/api/v1/projects/helsinki/spaces/helsinki/comments";
const URN = "urn:ngsi-ld:AirQualityObserved:journey-comments";

test("a person comments with a mention, the colleague is notified, and the comment leaves with its notification", async ({ browser }) => {
  const steward = await signIn(browser, STEWARD, "/projects/helsinki/apps?lang=en");
  const editor = await signIn(browser, EDITOR, "/projects/helsinki/apps?lang=en");
  try {
    const asSteward = { "x-csrf-token": await csrf(steward.context), "content-type": "application/json" };
    const asEditor = { "x-csrf-token": await csrf(editor.context), "content-type": "application/json" };

    const notAnUrn = await steward.page.request.post(COMMENTS, { headers: asSteward, data: { urn: "station 7", text: "x" } });
    expect(notAnUrn.status(), "a comment on something that is no NGSI-LD URN is refused").toBe(400);

    const made = await steward.page.request.post(COMMENTS, {
      headers: asSteward,
      data: { urn: URN, text: `@${EDITOR.user} a journey note. cc @nobody@example.org` },
    });
    expect(made.status()).toBe(201);
    const comment = (await made.json()) as { id: number; mentions: string[]; unknownMentions: string[] };
    expect(comment.mentions).toEqual([EDITOR.user]);
    expect(comment.unknownMentions).toEqual(["nobody@example.org"]);

    const inbox = (await (await editor.page.request.get("/api/v1/notifications")).json()) as {
      items: { id: number; commentId: number; read: boolean }[];
    };
    const mine = inbox.items.find((item) => item.commentId === comment.id);
    expect(mine, "the editor is notified of the mention").toBeDefined();
    const read = await editor.page.request.post(`/api/v1/notifications/${mine?.id}/read`, { headers: asEditor });
    expect(read.status()).toBe(204);

    const notTheirs = await editor.page.request.delete(`${COMMENTS}/${comment.id}`, { headers: asEditor });
    expect(notTheirs.status(), "only the author removes a comment").toBe(404);
    const removed = await steward.page.request.delete(`${COMMENTS}/${comment.id}`, { headers: asSteward });
    expect(removed.status()).toBe(204);

    const after = (await (await editor.page.request.get("/api/v1/notifications")).json()) as { items: { commentId: number }[] };
    expect(after.items.some((item) => item.commentId === comment.id), "the notification left with its comment").toBe(false);
  } finally {
    await steward.context.close();
    await editor.context.close();
  }
});
