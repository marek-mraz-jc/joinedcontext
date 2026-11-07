/**
 * The assistant evals on dev (T-2733; AG-87, AG-91, TS-26): the conversations of
 * `tests/assistant_evals/<workflow>.yaml` and of `<workflow>--<case>.yaml` (T-2742), each started with the real model as demo.steward, its
 * questions answered from the conversation's later lines, and the calls it made checked against
 * the conversation's `expect`. A passing run is written to
 * `tests/assistant_evals/recordings/<file>.json`, named as the conversation's file — the run's events with the tools' outputs
 * left out — which `tests/assistant_evals_tests.rs` replays on every push with no spend.
 *
 * Spend: at most 10 conversations a pass (`EVAL_LIMIT`, capped at 10), those never recorded
 * first, then the oldest recordings. Every title names the assistant, so the hourly sweep leaves
 * the file to the nightly batch, which runs it once and announces it in AI_shared_folder.md
 * before it starts. `EVAL_WORKFLOWS=space,datamodel--one-to-many` picks conversations by workflow
 * (every file of it) or by file.
 *
 * Nothing is proposed: a change opens a draft for the person, and the drafts of this pass
 * (named `eval-…`) are removed at the end; the run is cancelled once read. The viewer's refusal
 * is checked for every conversation, and costs no model call. A person-only conversation is
 * recorded like any other; the replay fails it if the run took the person's step.
 */
import { existsSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { expect, test, type BrowserContext, type Page } from "@playwright/test";
import { parse } from "yaml";
import { STEWARD, VIEWER, csrf, signIn, sweepDrafts } from "./portal";

const PROJECT = "helsinki";
const HERE = dirname(fileURLToPath(import.meta.url));
const EVALS = join(HERE, "../../../tests/assistant_evals");
const RECORDINGS = join(EVALS, "recordings");
const LIMIT = Math.min(10, Number(process.env.EVAL_LIMIT ?? 10) || 10);
/** How long a run may stay silent after it has said something before it counts as done. */
const QUIET_MS = 20_000;
const RUN_MS = 300_000;

type Call = { tool: string; input?: Record<string, unknown> };
type Conversation = {
  /** The file's name without `.yaml`: the workflow, or `<workflow>--<case>`. */
  name: string;
  workflow: string;
  as: string;
  says: string[];
  expect: { calls: Call[]; outcome: string; why?: string };
  refusal: { as: string; says: string };
};
type Recorded = { kind: string; payload: Record<string, unknown> };

const conversations: Conversation[] = readdirSync(EVALS)
  .filter((file) => file.endsWith(".yaml"))
  .sort()
  .map((file) => ({ ...(parse(readFileSync(join(EVALS, file), "utf8")) as Omit<Conversation, "name">), name: file.replace(/\.yaml$/, "") }));

function recordedAt(name: string): string {
  const file = join(RECORDINGS, `${name}.json`);
  return existsSync(file) ? ((JSON.parse(readFileSync(file, "utf8")) as { recordedAt: string }).recordedAt ?? "") : "";
}

/** This pass's conversations: the ones asked for, else the never recorded and then the oldest. */
function chosen(): Conversation[] {
  const named = (process.env.EVAL_WORKFLOWS ?? "").split(",").map((w) => w.trim()).filter(Boolean);
  const pool = named.length ? conversations.filter((c) => named.includes(c.workflow) || named.includes(c.name)) : conversations;
  return [...pool]
    .sort((a, b) => recordedAt(a.name).localeCompare(recordedAt(b.name)))
    .slice(0, LIMIT);
}

/** `want`'s fields, and every one of them equal, in `got` (the Rust replay's `carries`). */
function carries(got: unknown, want: unknown): boolean {
  if (want && typeof want === "object" && !Array.isArray(want)) {
    return (
      !!got &&
      typeof got === "object" &&
      Object.entries(want).every(([key, value]) => carries((got as Record<string, unknown>)[key], value))
    );
  }
  return JSON.stringify(got) === JSON.stringify(want);
}

/** One SSE frame's kind and payload, without what the tool answered or when. */
function kept(kind: string, data: string): Recorded {
  const payload = JSON.parse(data) as Record<string, unknown>;
  for (const key of ["output", "seq", "timestamp"]) {
    delete payload[key];
  }
  return { kind, payload };
}

/**
 * Follows a run's event stream with the page's own session, answers its questions from `answers`
 * (or the first option), and returns its events once it has said something and then stayed quiet.
 */
async function follow(context: BrowserContext, page: Page, run: string, answers: string[]): Promise<Recorded[]> {
  const base = new URL(page.url()).origin;
  const cookie = (await context.cookies(base)).map((c) => `${c.name}=${c.value}`).join("; ");
  const token = await csrf(context);
  const events: Recorded[] = [];
  const stop = new AbortController();
  const deadline = setTimeout(() => stop.abort(), RUN_MS);
  let quiet: ReturnType<typeof setTimeout> | undefined;
  const settle = () => {
    clearTimeout(quiet);
    quiet = setTimeout(() => stop.abort(), QUIET_MS);
  };
  try {
    const response = await fetch(`${base}/api/v1/projects/${PROJECT}/agent-runs/${run}/events`, {
      headers: { cookie, accept: "text/event-stream", "x-jc-run-origin": "journey" },
      signal: stop.signal,
    });
    expect(response.ok, `the run's events: ${response.status}`).toBe(true);
    const reader = response.body!.pipeThrough(new TextDecoderStream()).getReader();
    let buffer = "";
    for (;;) {
      const { value, done } = await reader.read();
      if (done) {
        break;
      }
      buffer += value;
      let end: number;
      while ((end = buffer.indexOf("\n\n")) >= 0) {
        const frame = buffer.slice(0, end);
        buffer = buffer.slice(end + 2);
        const kind = /^event: ?(.*)$/m.exec(frame)?.[1];
        const data = frame
          .split("\n")
          .filter((line) => line.startsWith("data:"))
          .map((line) => line.replace(/^data: ?/, ""))
          .join("\n");
        if (!kind || !data || kind === "lag") {
          continue;
        }
        const event = kept(kind, data);
        events.push(event);
        if (kind === "question") {
          clearTimeout(quiet);
          const options = (event.payload.options as { value: unknown }[] | undefined) ?? [];
          const given = answers.shift() ?? options[0]?.value;
          if (given === undefined) {
            return events;
          }
          const answered = await page.request.post(`/api/v1/projects/${PROJECT}/agent-runs/${run}/answers`, {
            headers: { "x-csrf-token": token },
            data: { questionId: event.payload.questionId, answers: { answer: given } },
          });
          expect(answered.ok(), await answered.text()).toBe(true);
        } else if (events.some((e) => e.kind === "thought")) {
          settle();
        }
      }
    }
  } catch (err) {
    if (!stop.signal.aborted) {
      throw err;
    }
  } finally {
    clearTimeout(deadline);
    clearTimeout(quiet);
  }
  return events;
}

/** The calls of a run, without the search the conversation runs on the person's words itself. */
function calls(events: Recorded[], message: string): Recorded[] {
  const tools = events.filter((e) => e.kind === "tool");
  const first = tools.findIndex(
    (e) => e.payload.tool === "search_catalog" && (e.payload.input as { q?: string } | undefined)?.q === message,
  );
  return tools.filter((_, at) => at !== first);
}

test.describe.configure({ mode: "serial" });
test.setTimeout(3_600_000);

test("the assistant evals: each conversation calls what it expects, and is recorded", async ({ browser }) => {
  const picked = chosen();
  expect(picked.length, "no conversation to run").toBeGreaterThan(0);
  const { page, context } = await signIn(browser, STEWARD, `/projects/${PROJECT}/assistant?lang=en`);
  const failed: string[] = [];
  try {
    const profile = await page.request.get(`/api/v1/projects/org/agentprofiles/app-builder`);
    const model = profile.ok()
      ? `app-builder: ${((await profile.json()) as { spec?: { model?: { name?: string } } }).spec?.model?.name ?? "unknown"}`
      : "app-builder";
    mkdirSync(RECORDINGS, { recursive: true });
    for (const conversation of picked) {
      const [message, ...answers] = conversation.says;
      const started = await page.request.post(`/api/v1/projects/${PROJECT}/assistant/conversations`, {
        headers: { "x-csrf-token": await csrf(context) },
        data: { message },
      });
      expect(started.status(), await started.text()).toBe(202);
      const run = ((await started.json()) as { id: string }).id;
      try {
        const events = await follow(context, page, run, [...answers]);
        const made = calls(events, message);
        const ok = made.filter((e) => e.payload.status === "ok");
        const missing = conversation.expect.calls.filter(
          (call) => !ok.some((e) => e.payload.tool === call.tool && (!call.input || carries(e.payload.input, call.input))),
        );
        if (missing.length) {
          failed.push(
            `${conversation.name} (run ${run}): expected ${JSON.stringify(missing)}, made ${JSON.stringify(
              made.map((e) => [e.payload.tool, e.payload.status, e.payload.error]),
            )}`,
          );
          continue;
        }
        writeFileSync(
          join(RECORDINGS, `${conversation.name}.json`),
          `${JSON.stringify(
            { workflow: conversation.workflow, run, recordedAt: new Date().toISOString(), model, events },
            null,
            2,
          )}\n`,
        );
      } finally {
        await page.request.post(`/api/v1/projects/${PROJECT}/agent-runs/${run}/cancel`, {
          headers: { "x-csrf-token": await csrf(context) },
        });
      }
    }
  } finally {
    await sweepDrafts(context, page, PROJECT, /^eval-/);
    await context.close();
  }
  expect(failed, "conversations that did not do what they expect").toEqual([]);
});

test("a viewer is refused every assistant eval conversation", async ({ browser }) => {
  const { page, context } = await signIn(browser, VIEWER, `/projects/${PROJECT}/assistant?lang=en`);
  try {
    for (const conversation of conversations) {
      const answer = await page.request.post(`/api/v1/projects/${PROJECT}/assistant/conversations`, {
        headers: { "x-csrf-token": await csrf(context) },
        data: { message: conversation.refusal.says },
      });
      expect(answer.status(), `${conversation.name}: ${await answer.text()}`).toBe(403);
      expect(await answer.text()).toMatch(/propose/i);
    }
  } finally {
    await context.close();
  }
});
