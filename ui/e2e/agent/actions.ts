/**
 * T-5981 — the user agent's action language. Each step the model answers ONE JSON object; this file
 * parses it and refuses anything else, so a model never reaches Playwright with a half-formed
 * command. Pure (no Playwright, no network): tests/agentActions.test.ts covers it.
 */

export type Action =
  | { kind: "click"; role: string; name: string }
  | { kind: "fill"; role: string; name: string; value: string }
  | { kind: "select"; role: string; name: string; value: string }
  | { kind: "press"; key: string }
  | { kind: "navigate"; url: string }
  | { kind: "back" }
  | { kind: "done"; answer: string }
  | { kind: "stuck"; reason: string };

/** The ARIA roles a click, fill or select may name: what the accessibility tree shows a person. */
const ROLES = new Set([
  "button",
  "link",
  "tab",
  "menuitem",
  "option",
  "checkbox",
  "radio",
  "switch",
  "combobox",
  "textbox",
  "searchbox",
  "spinbutton",
  "listbox",
  "row",
  "cell",
  "treeitem",
  "heading",
]);
/** Keys a `press` may send: no chords that leave the page (Ctrl+W) or open devtools. */
const KEYS = new Set([
  "Enter",
  "Escape",
  "Tab",
  "Shift+Tab",
  "ArrowDown",
  "ArrowUp",
  "ArrowLeft",
  "ArrowRight",
  "Space",
  "Home",
  "End",
  "PageDown",
  "PageUp",
]);
const MAX_TEXT = 2_000;

/** The action schema as the model reads it in its system prompt. */
export const ACTION_HELP = `Answer with ONE JSON object and nothing else, one of:
{"kind":"click","role":"<aria role>","name":"<accessible name>"}
{"kind":"fill","role":"textbox|searchbox|combobox|spinbutton","name":"<accessible name>","value":"<text>"}
{"kind":"select","role":"combobox|listbox","name":"<accessible name>","value":"<option label>"}
{"kind":"press","key":"Enter|Escape|Tab|Shift+Tab|ArrowDown|ArrowUp|Space|..."}
{"kind":"navigate","url":"<absolute https URL on an allowed host, or a path>"}
{"kind":"back"}  (closes a tab a link opened and returns to the page before it, else the browser's Back)
{"kind":"done","answer":"<what you found or finished, in one or two sentences>"}
{"kind":"stuck","reason":"<why you cannot go on, naming the control or page that stopped you>"}
Role and name must match a line of the accessibility tree exactly.`;

const text = (value: unknown, field: string): string => {
  if (typeof value !== "string" || value.trim() === "")
    throw new Error(`"${field}" must be a non-empty string`);
  if (value.length > MAX_TEXT)
    throw new Error(`"${field}" is longer than ${MAX_TEXT} characters`);
  return value;
};

const role = (value: unknown): string => {
  const r = text(value, "role");
  if (!ROLES.has(r))
    throw new Error(`role "${r}" is not one of ${[...ROLES].join(", ")}`);
  return r;
};

const json = (body: string): unknown => {
  try {
    return JSON.parse(body) as unknown;
  } catch {
    return undefined;
  }
};

/**
 * Parses one model answer into an Action. Accepts the object bare, inside one ```json fence, or as
 * the last `{"kind"...}` object after a few lines of reasoning (models think aloud despite being told
 * not to); anything else throws with a message the model reads on its one retry.
 */
export function parseAction(answer: string): Action {
  const fenced = /^\s*```(?:json)?\s*([\s\S]*?)\s*```\s*$/.exec(answer);
  const whole = (fenced ? fenced[1] : answer).trim();
  const last = whole.lastIndexOf('{"kind"');
  const candidates = [
    whole,
    last > 0 ? whole.slice(last).replace(/\s*```\s*$/, "") : "",
  ].filter(Boolean);
  // Some models end the object one brace short (seen with claude-haiku-5.5 on OpenRouter): one
  // missing closing brace is read, anything else is refused.
  const raw = candidates
    .flatMap((c) => [c, `${c}}`])
    .map(json)
    .find((v) => v !== undefined);
  if (raw === undefined) throw new Error("the answer is not one JSON object");
  if (typeof raw !== "object" || raw === null || Array.isArray(raw))
    throw new Error("the answer is not one JSON object");
  const o = raw as Record<string, unknown>;
  switch (o.kind) {
    case "click":
      return { kind: "click", role: role(o.role), name: text(o.name, "name") };
    case "fill":
      return {
        kind: "fill",
        role: role(o.role),
        name: text(o.name, "name"),
        value:
          typeof o.value === "string"
            ? o.value.slice(0, MAX_TEXT)
            : text(o.value, "value"),
      };
    case "select":
      return {
        kind: "select",
        role: role(o.role),
        name: text(o.name, "name"),
        value: text(o.value, "value"),
      };
    case "press": {
      const key = text(o.key, "key");
      if (!KEYS.has(key))
        throw new Error(`key "${key}" is not one of ${[...KEYS].join(", ")}`);
      return { kind: "press", key };
    }
    case "navigate":
      return { kind: "navigate", url: text(o.url, "url") };
    case "back":
      return { kind: "back" };
    case "done":
      return { kind: "done", answer: text(o.answer, "answer") };
    case "stuck":
      return { kind: "stuck", reason: text(o.reason, "reason") };
    default:
      throw new Error(
        `"kind" must be click, fill, select, press, navigate, back, done or stuck`,
      );
  }
}

/**
 * Asks for the next action: a bad answer is sent back once with the parser's message; a second bad
 * answer ends the run as stuck, so a confused model costs at most two calls per step.
 */
export async function nextAction(
  ask: (retryNote?: string) => Promise<string>,
): Promise<{ action: Action; retried: boolean }> {
  const first = await ask();
  try {
    return { action: parseAction(first), retried: false };
  } catch (error) {
    const second = await ask(
      `Your last answer was refused: ${(error as Error).message}. ${ACTION_HELP}`,
    );
    try {
      return { action: parseAction(second), retried: true };
    } catch (again) {
      const said = second.replace(/\s+/g, " ").trim().slice(0, 200);
      return {
        action: {
          kind: "stuck",
          reason: `the model gave no valid action twice: ${(again as Error).message} (it said: ${said || "nothing"})`,
        },
        retried: true,
      };
    }
  }
}

/**
 * Resolves a navigate target against the run's allowed hosts: a path stays on the start origin; an
 * absolute URL must be https on one of `hosts`. Anything else (javascript:, file:, another site) is
 * refused, so the model cannot send the signed-in browser elsewhere.
 */
export function allowedUrl(
  target: string,
  start: string,
  hosts: string[],
): string {
  const url = new URL(target, start);
  const local = url.origin === new URL(start).origin;
  if (!local && (url.protocol !== "https:" || !hosts.includes(url.host))) {
    throw new Error(
      `navigate to ${url.origin} is outside the run's hosts (${hosts.join(", ")})`,
    );
  }
  return url.toString();
}

/** Text shaped like a credential: a JWT, a bearer header, an OpenRouter or GitHub token. */
export const SECRET_SHAPE =
  /\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}(?:\.[A-Za-z0-9_-]+)?|\bBearer\s+[A-Za-z0-9._~+/=-]{16,}|\bsk-or-[A-Za-z0-9-]{16,}|\bgh[pousr]_[A-Za-z0-9]{20,}/g;

/** Masks credential-shaped text before the model or a log sees it; returns the text and how many it hid. */
export function redact(tree: string): { text: string; hidden: number } {
  let hidden = 0;
  const text = tree.replace(SECRET_SHAPE, () => {
    hidden += 1;
    return "[secret hidden]";
  });
  return { text, hidden };
}
