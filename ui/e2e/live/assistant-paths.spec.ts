/**
 * T-2697, AG-91: Integrate a pipeline, walked on dev with a small CSV, within its budgets. The
 * Portal takes every step of this walk itself, so no model call stands in the way: the first
 * question under 300 ms and on screen under 2 s, the file profiled under 3 s, the drafted space
 * open under 2 min. The times are the run's own (`elapsedMs`, `durationMs` on its events), read
 * from the stream the dock reads, never typed here.
 */
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { STEWARD, signIn, sweepDrafts } from "./portal";

const PROJECT = "helsinki";

interface Event {
  kind: string;
  payload: Record<string, unknown>;
}

/** Every event of the run the dock follows, replayed from its stream. */
async function runEvents(page: Page): Promise<Event[]> {
  return page.evaluate(async (project) => {
    const raw = window.sessionStorage.getItem("jc.assistant.run");
    const run = raw === null ? null : (JSON.parse(raw) as { runId?: string } | string | null);
    const id = typeof run === "object" && run !== null ? run.runId : null;
    if (typeof id !== "string") {
      return [];
    }
    const kinds = ["path", "question", "answer", "tool", "navigate", "thought", "status"];
    return new Promise<Event[]>((resolve) => {
      const seen: Event[] = [];
      const source = new EventSource(`/api/v1/projects/${project}/agent-runs/${id}/events`, { withCredentials: true });
      for (const kind of kinds) {
        source.addEventListener(kind, (message) => {
          seen.push({ kind, payload: JSON.parse((message as MessageEvent<string>).data) as Record<string, unknown> });
        });
      }
      setTimeout(() => {
        source.close();
        resolve(seen);
      }, 2000);
    });
  }, PROJECT);
}

test("Integrate a pipeline from a CSV and its feed stays within its budgets (T-2695, T-2697)", async ({ browser }) => {
  test.setTimeout(180_000);
  const { page, context } = await signIn(browser, STEWARD, `/projects/${PROJECT}/spaces?lang=en`);
  const name = `journey-paths-${Date.now().toString(36)}`;
  try {
    await page.getByRole("button", { name: "Open the assistant" }).click();
    const clicked = Date.now();
    await page.getByRole("button", { name: /^Integrate a pipeline/ }).click();
    const drop = page.getByLabel("Choose a file of your data");
    await expect(drop).toBeAttached();
    const onScreen = Date.now() - clicked;

    await drop.setInputFiles({
      name: `${name}.csv`,
      mimeType: "text/csv",
      buffer: Buffer.from("station,name,bikes,lat,lon\n1,Kamppi,4,60.169,24.931\n2,Kallio,0,60.184,24.950\n"),
    });
    await page.getByRole("radio", { name: /^A new context space/ }).click();
    // The file is a sample of a feed (T-2695): the pipeline reads the address and is tested on the
    // file, so the address is never fetched while drafting.
    await page.getByLabel("Or the address of a feed").last().fill(`https://feed.example.org/${name}.json`);
    await page.getByRole("button", { name: "Use this address" }).last().click();
    await expect(page).toHaveURL(new RegExp(`/projects/${PROJECT}/spaces/complete\\?space=${name}`), { timeout: 120_000 });

    const events = await runEvents(page);
    const question = events.find((event) => event.kind === "question");
    const profiled = events.find((event) => event.kind === "tool" && event.payload.tool === "profile_sample");
    const opened = events.find((event) => event.kind === "navigate");
    const drafted = events.find((event) => event.kind === "tool" && event.payload.tool === "space_complete");
    const kinds = ((drafted?.payload.output as { drafts?: { kind: string }[] } | undefined)?.drafts ?? []).map((draft) => draft.kind);
    expect(kinds, "the drafts the person proposes").toEqual(expect.arrayContaining(["DataSource", "Pipeline"]));
    const ms = (value: unknown) => (typeof value === "number" ? value : Number.NaN);
    const times = {
      firstStep: ms(question?.payload.elapsedMs),
      onScreen,
      profiling: ms(profiled?.payload.durationMs),
      whole: ms(opened?.payload.elapsedMs),
    };
    console.log(`budgets ${JSON.stringify(times)}`);
    expect(times.firstStep, "the first question, server side").toBeLessThan(300);
    expect(times.onScreen, "the first question on screen").toBeLessThan(2000);
    expect(times.profiling, "the file profiled").toBeLessThan(3000);
    expect(times.whole, "the drafted space open").toBeLessThan(120_000);
    // Every step and every page opened says when it happened (API/04 §4).
    for (const event of events.filter((one) => one.kind === "tool" || one.kind === "navigate")) {
      expect(typeof event.payload.elapsedMs, `${event.kind} ${JSON.stringify(event.payload).slice(0, 120)}`).toBe("number");
    }
  } finally {
    await page.getByRole("button", { name: "Start a new conversation" }).click().catch(() => undefined);
    // The space and its model are drafts of this journey alone: nothing is proposed, nothing stays.
    await sweepDrafts(context, page, PROJECT, new RegExp(`^${name}`));
    await context.close();
  }
});
