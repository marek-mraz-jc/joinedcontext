/**
 * T-5981 — the user agent: a model plays a persona in a real browser and tries to finish one UI
 * scenario of tasks/connector-scenarios.yaml (segment ui, S-56..S-85) alone.
 *
 * Each step the model reads the page's accessibility tree (role + name, Playwright's aria snapshot,
 * never raw HTML), the goal and its own history, and answers one action (actions.ts). Playwright
 * runs it, every step saves a screenshot, and the run stops at done, stuck, or twice the scenario's
 * step budget. The run's JSON (steps, actions, time, outcome, tokens, cost) and the shots go to
 * `<dir>/`; the live driver is e2e/live/user-agent.spec.ts, the model-free CI proof e2e/agent/stub.spec.ts.
 *
 * Safety: the model never sees cookies, headers or tokens (only the tree, with credential-shaped
 * text masked), navigates only to the run's hosts, and opens no Change: `holdWrites` stops every
 * write to the Portal API except a dry run or a draft, and tells the model the proposal was held.
 */
import { mkdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import type { APIRequestContext, BrowserContext, Page } from "@playwright/test";
import { parse } from "yaml";
import { secretMasks } from "../live/shoot";
import { ACTION_HELP, allowedUrl, nextAction, redact } from "./actions";
import type { Action } from "./actions";

export interface Scenario {
  id: string;
  segment: string;
  as: string;
  says: string;
  expect: string;
  critic?: string;
}

export interface Message {
  role: "system" | "user" | "assistant";
  content: string;
}

export interface Reply {
  text: string;
  inTokens: number;
  outTokens: number;
  cost: number;
}

export interface Model {
  name: string;
  ask(messages: Message[]): Promise<Reply>;
}

export interface Step {
  n: number;
  url: string;
  action: Action;
  result: string;
  retried: boolean;
  ms: number;
  shot: string;
}

export interface RunResult {
  scenario: string;
  persona: string;
  width: number;
  lang: string;
  model: string;
  outcome: "done" | "stuck" | "budget" | "error";
  answer: string;
  budget: number;
  steps: Step[];
  heldWrites: string[];
  secretsHidden: number;
  ms: number;
  tokens: { in: number; out: number };
  costUsd: number;
}

/** Reads one scenario from the catalogue (default: the board's tasks/connector-scenarios.yaml). */
export function loadScenario(
  id: string,
  file = process.env.AGENT_SCENARIOS_FILE ??
    "/workspace/tasks/connector-scenarios.yaml",
): Scenario {
  const all = parse(readFileSync(file, "utf8")) as Scenario[];
  const found = all.find((s) => s.id === id);
  if (!found) throw new Error(`scenario ${id} is not in ${file}`);
  return found;
}

/** The scenario's step budget: the "<= N steps" of its expect line, else 12. */
export function stepBudget(scenario: Pick<Scenario, "expect">): number {
  const m = /<=\s*(\d+)\s*steps?/.exec(scenario.expect);
  return m ? Number(m[1]) : 12;
}

/** Who each persona is, in the words the model plays them by. */
export const PERSONAS: Record<string, string> = {
  citizen:
    "a resident of the region with no account, on their own device, who knows nothing about the platform and reads only what the page shows",
  "steward-first-day":
    "a data steward of a city department who was given access today and has never seen this Portal",
  steward:
    "a data steward of a city department who keeps the department's datasets up to date; comfortable with spreadsheets, not with YAML or APIs",
  analyst:
    "a data analyst who uses Excel and Power BI and wants data without writing code",
  journalist:
    "a journalist checking facts against open data, who needs licence, publisher and update date",
  viewer: "a colleague with read access only",
};

const LANG_NAME: Record<string, string> = { en: "English", sk: "Slovak" };
const MAX_TREE = 14_000;
const ACTION_MS = 60_000;
const EDGE_WAIT_MS = 65_000;
// Two model calls of up to 2 x 90 s each, the tree, the shot and the action.
const STEP_MS = 480_000;

function deadline<T>(work: Promise<T>, ms: number, what: string): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const late = new Promise<never>((_, reject) => {
    timer = setTimeout(
      () => reject(new Error(`${what} took over ${ms / 1000} s`)),
      ms,
    );
  });
  return Promise.race([work, late]).finally(() => clearTimeout(timer));
}
const HISTORY = 15;

function system(
  scenario: Scenario,
  persona: string,
  lang: string,
  given?: string,
): string {
  return [
    `You are ${PERSONAS[persona] ?? persona}. You use a web browser to get one thing done, alone, the way that person would.`,
    `Your goal: ${scenario.says}`,
    ...(given ? [`What you know before you start: ${given}`] : []),
    `The interface language is ${LANG_NAME[lang] ?? lang}; read and type in it.`,
    "Each turn you get the page address, its accessibility tree (role, accessible name, text) and what you did so far.",
    "Pick the single next action a real person would take. Prefer visible links, buttons and search over typing addresses.",
    "Never invent data, never type a password or token, never approve your own change. If a write is held by the runner, the flow ends there: answer done and say what you would have proposed.",
    "Answer done when the goal is reached (say what you found), stuck when you cannot go on (say what stopped you).",
    ACTION_HELP,
  ].join("\n");
}

/**
 * Stops every write to `/api/` on `hosts` and records it, so a run opens no Change, sends no
 * invitation and deletes nothing. What passes: a dry run (`dryRun=All`), a draft, the sign-in and
 * CSRF hops, the read POSTs the pages make (an Endpoint's data preview) and an MCP call that only
 * reads (READ_MCP). An assistant question is held: its tools may propose.
 */
const READ_POSTS =
  /^\/api\/v1\/csrf\b|^\/auth\/|^\/api\/v1\/projects\/[^/]+\/endpoints\/[^/]+\/preview$/;
const READ_MCP =
  /^(initialize|ping|notifications\/.*|tools\/list|resources\/(list|read|templates\/list)|prompts\/list)$/;
const READ_TOOL =
  /^(list_|get_|query_|retrieve_|describe_|search|summarize|area_overview)/;

/** Whether an MCP JSON-RPC body (one call or a batch) only lists or reads. */
export function mcpReadOnly(body: string | null): boolean {
  let calls: unknown;
  try {
    calls = JSON.parse(body ?? "");
  } catch {
    return false;
  }
  return [calls].flat().every((call) => {
    const { method, params } = (call ?? {}) as {
      method?: unknown;
      params?: { name?: unknown };
    };
    if (typeof method !== "string") return false;
    return (
      READ_MCP.test(method) ||
      (method === "tools/call" &&
        typeof params?.name === "string" &&
        READ_TOOL.test(params.name))
    );
  });
}

export async function holdWrites(
  context: BrowserContext,
  hosts: string[],
  held: string[],
  drafts: Set<string> = new Set(),
): Promise<void> {
  await context.route(
    (url) => hosts.includes(url.host) && url.pathname.startsWith("/api/"),
    async (route) => {
      const request = route.request();
      const url = new URL(request.url());
      const write = !["GET", "HEAD", "OPTIONS"].includes(request.method());
      const read =
        url.searchParams.get("dryRun") === "All" ||
        url.pathname.includes("/drafts") ||
        READ_POSTS.test(url.pathname) ||
        (url.pathname.endsWith("/mcp") && mcpReadOnly(request.postData()));
      // A form saves what it holds as a draft; the run deletes each one after it (removeDrafts).
      if (
        write &&
        /\/api\/v1\/projects\/[^/]+\/drafts\/[^/]+\/[^/]+$/.test(
          url.pathname,
        ) &&
        request.method() !== "DELETE"
      )
        drafts.add(url.pathname);
      if (!write || read) return route.fallback();
      held.push(`${request.method()} ${url.pathname}`);
      return route.fulfill({
        status: 409,
        contentType: "application/problem+json",
        body: JSON.stringify({
          type: "about:blank",
          title: "Held by the user-agent runner",
          status: 409,
          detail: "This test run proposes nothing; the flow ends here.",
        }),
      });
    },
  );
}

/**
 * Deletes the drafts a run's forms saved (holdWrites records them): left on dev they pile up and
 * slow the drafts route for everyone (T-2248). Returns the paths a delete did not remove.
 */
export async function removeDrafts(
  request: APIRequestContext,
  drafts: Set<string>,
  csrfToken: string,
): Promise<string[]> {
  const left: string[] = [];
  for (const path of drafts) {
    const answer = await request.delete(path, {
      headers: { "x-csrf-token": csrfToken },
    });
    if (!answer.ok() && answer.status() !== 404)
      left.push(`${path} (${answer.status()})`);
  }
  return left;
}

async function tree(page: Page): Promise<string> {
  const snapshot = await page
    .locator("body")
    .ariaSnapshot({ timeout: 15_000 })
    .catch(
      (e: Error) => `(no accessibility tree: ${e.message.split("\n")[0]})`,
    );
  return snapshot.length > MAX_TREE
    ? `${snapshot.slice(0, MAX_TREE)}\n… (tree cut at ${MAX_TREE} characters; scroll or search to see more)`
    : snapshot;
}

async function settle(page: Page): Promise<void> {
  await page
    .waitForLoadState("load", { timeout: 20_000 })
    .catch(() => undefined);
  await page.waitForTimeout(800);
}

/** Runs one action; the string is what the model reads back as its result. */
async function act(
  page: Page,
  action: Action,
  start: string,
  hosts: string[],
): Promise<string> {
  switch (action.kind) {
    case "navigate": {
      const url = allowedUrl(action.url, start, hosts);
      try {
        const answer = await page.goto(url, {
          waitUntil: "load",
          timeout: 45_000,
        });
        return `opened ${url} (${answer?.status() ?? "no answer"})`;
      } catch (error) {
        // A file address starts a download instead of a page; the download watcher reports it.
        if (/Download is starting/.test((error as Error).message))
          return `opened ${url}`;
        throw error;
      }
    }
    case "back":
      await page.goBack({ waitUntil: "load", timeout: 30_000 });
      return `went back to ${page.url()}`;
    case "press":
      await page.keyboard.press(action.key === "Space" ? " " : action.key);
      return `pressed ${action.key}`;
    case "done":
    case "stuck":
      return action.kind;
    default: {
      const role = action.role as Parameters<Page["getByRole"]>[0];
      let target = page.getByRole(role, { name: action.name, exact: true });
      if ((await target.count()) === 0)
        target = page.getByRole(role, { name: action.name });
      const count = await target.count();
      if (count === 0)
        return `no ${action.role} named "${action.name}" is on the page`;
      const one = target.first();
      if (action.kind === "click") await one.click({ timeout: 10_000 });
      else if (action.kind === "fill")
        await one.fill(action.value, { timeout: 10_000 });
      else await one.selectOption({ label: action.value }, { timeout: 10_000 });
      return `${action.kind} ${action.role} "${action.name}" ok${count > 1 ? ` (first of ${count})` : ""}`;
    }
  }
}

/** Plays one scenario on an already opened page; writes `<dir>/run.json` and `<dir>/step-NN.png`. */
export async function runScenario(options: {
  page: Page;
  scenario: Scenario;
  given?: string;
  persona: string;
  width: number;
  lang: string;
  model: Model;
  dir: string;
  hosts: string[];
  held?: string[];
}): Promise<RunResult> {
  const { scenario, persona, width, lang, model, dir, hosts } = options;
  mkdirSync(dir, { recursive: true });
  const budget = stepBudget(scenario);
  const start = options.page.url();
  const began = Date.now();
  const steps: Step[] = [];
  const tokens = { in: 0, out: 0 };
  let cost = 0;
  let hidden = 0;
  let outcome: RunResult["outcome"] = "budget";
  let answer = "";
  const held = options.held ?? [];
  let page = options.page;
  // What a person sees beside the page: a file saved and a tab opened (a "Download" link opens one).
  const events: string[] = [];
  const saving: Promise<void>[] = [];
  // The edge limits requests per address (the swarm shares one): a page it refused with 429 is
  // waited out once and loaded again before the model sees it, as walk-citizen does.
  let refused = false;
  const watch = (p: Page) =>
    p
      .on("response", (response) => {
        if (
          response.status() === 429 &&
          response.request().isNavigationRequest()
        )
          refused = true;
      })
      .on(
        "filechooser",
        () =>
          void events.push(
            "a file picker opened; this test run has no file to give it",
          ),
      )
      .on("download", (download) => {
        saving.push(
          download
            .path()
            .then(
              (path) =>
                events.push(
                  `downloaded ${download.suggestedFilename()} (${path ? statSync(path).size : 0} bytes)`,
                ),
              (e: Error) =>
                void events.push(
                  `download of ${download.suggestedFilename()} failed: ${e.message}`,
                ),
            )
            .then(() => undefined),
        );
      });
  watch(page);
  page.context().on("page", (opened) => {
    watch(opened);
    events.push(`a new tab opened`);
    page = opened;
  });

  /** One step; true when the model answered done or stuck. */
  const once = async (n: number): Promise<boolean> => {
    const t0 = Date.now();
    const shot = `step-${String(n).padStart(2, "0")}.png`;
    if (refused) {
      refused = false;
      await page.waitForTimeout(EDGE_WAIT_MS);
      await page.reload({ waitUntil: "load" });
      events.push(
        `the edge answered 429; the runner waited ${EDGE_WAIT_MS / 1000} s and loaded the page again`,
      );
    }
    await page.screenshot({
      path: join(dir, shot),
      fullPage: false,
      mask: secretMasks(page),
    });
    const masked = redact(await tree(page));
    hidden += masked.hidden;
    const done =
      steps
        .slice(-HISTORY)
        .map((s) => `${s.n}. ${JSON.stringify(s.action)} -> ${s.result}`)
        .join("\n") || "(nothing yet)";
    const prompt = `Step ${n} of at most ${budget * 2} (target ${budget}).\nAddress: ${page.url()}\nTitle: ${await page.title()}\n\nDone so far:\n${done}\n\nAccessibility tree:\n${masked.text}`;
    const { action, retried } = await nextAction(async (note) => {
      const messages: Message[] = [
        {
          role: "system",
          content: system(scenario, persona, lang, options.given),
        },
        { role: "user", content: prompt },
      ];
      if (note) messages.push({ role: "user", content: note });
      const reply = await model.ask(messages);
      tokens.in += reply.inTokens;
      tokens.out += reply.outTokens;
      cost += reply.cost;
      return reply.text;
    });
    const heldBefore = held.length;
    let result: string;
    try {
      const opener = action.kind === "back" ? await page.opener() : null;
      if (opener) {
        await page.close();
        page = opener;
        result = `closed the tab, back on ${page.url()}`;
      } else
        result = await deadline(
          act(page, action, start, hosts),
          ACTION_MS,
          "the action",
        );
      if (action.kind !== "done" && action.kind !== "stuck") await settle(page);
    } catch (error) {
      result = `failed: ${(error as Error).message.split("\n")[0]}`;
    }
    await deadline(
      Promise.all(saving.splice(0)),
      ACTION_MS,
      "the download",
    ).catch((e: Error) => events.push(e.message));
    if (events.length)
      result += `; ${events.splice(0).join("; ")}${page.url() !== "about:blank" ? `, now on ${page.url()}` : ""}`;
    if (held.length > heldBefore)
      result += `; the runner held ${held.slice(heldBefore).join(", ")} (no Change is opened in a test run)`;
    steps.push({
      n,
      url: page.url(),
      action,
      result: redact(result).text,
      retried,
      ms: Date.now() - t0,
      shot,
    });
    if (action.kind !== "done" && action.kind !== "stuck") return false;
    outcome = action.kind;
    answer = action.kind === "done" ? action.answer : action.reason;
    return true;
  };
  // A step that hangs (a page that never settles, a model that never answers) ends the run as an
  // error with what was recorded so far, rather than holding the whole test budget.
  try {
    for (let n = 1; n <= budget * 2; n += 1)
      if (await deadline(once(n), STEP_MS, `step ${n}`)) break;
  } catch (error) {
    outcome = "error";
    answer = (error as Error).message.split("\n")[0];
  }
  const run: RunResult = {
    scenario: scenario.id,
    persona,
    width,
    lang,
    model: model.name,
    outcome,
    answer,
    budget,
    steps,
    heldWrites: held,
    secretsHidden: hidden,
    ms: Date.now() - began,
    tokens,
    costUsd: Number(cost.toFixed(5)),
  };
  writeFileSync(join(dir, "run.json"), `${JSON.stringify(run, null, 2)}\n`);
  return run;
}

/** Replays recorded answers in order: the model of the CI stub run, no network. */
export function recordedModel(answers: string[]): Model {
  let i = 0;
  return {
    name: "recorded",
    ask: async () => ({
      text: answers[i++] ?? '{"kind":"stuck","reason":"the recording ended"}',
      inTokens: 0,
      outTokens: 0,
      cost: 0,
    }),
  };
}

/**
 * A model on OpenRouter. The key comes from OPENROUTER_API_KEY or the secrets mount and goes only
 * into the Authorization header of this one request; it is never logged or written to a run.
 */
export function openRouterModel(id: string): Model {
  const key = () =>
    process.env.OPENROUTER_API_KEY?.trim() ||
    readFileSync("/workspace/.secrets/openrouter-api-key", "utf8").trim();
  return {
    name: id,
    ask: async (messages) => {
      for (let attempt = 1; ; attempt += 1) {
        const answer = await fetch(
          "https://openrouter.ai/api/v1/chat/completions",
          {
            method: "POST",
            headers: {
              authorization: `Bearer ${key()}`,
              "content-type": "application/json",
            },
            body: JSON.stringify({
              model: id,
              messages,
              temperature: 0,
              max_tokens: 1_500,
              usage: { include: true },
            }),
            signal: AbortSignal.timeout(90_000),
          },
        );
        if (answer.ok) {
          const body = (await answer.json()) as {
            choices?: {
              finish_reason?: string;
              message?: { content?: string };
            }[];
            usage?: {
              prompt_tokens?: number;
              completion_tokens?: number;
              cost?: number;
              cost_details?: { upstream_inference_cost?: number };
            };
          };
          return {
            text:
              body.choices?.[0]?.message?.content ||
              `(empty answer, finish_reason ${body.choices?.[0]?.finish_reason ?? "none"})`,
            inTokens: body.usage?.prompt_tokens ?? 0,
            outTokens: body.usage?.completion_tokens ?? 0,
            // A BYOK key bills upstream: `cost` is 0 there and the price is in cost_details.
            cost:
              body.usage?.cost ||
              body.usage?.cost_details?.upstream_inference_cost ||
              0,
          };
        }
        // OpenRouter's error body names the cause (no credit, unknown model); the key is not in it.
        const why = `${answer.status} ${(await answer.text()).slice(0, 300)}`;
        if (attempt >= 2 || ![429, 502, 503].includes(answer.status))
          throw new Error(`OpenRouter refused ${id}: ${why}`);
        await new Promise((r) => setTimeout(r, 5_000));
      }
    },
  };
}
