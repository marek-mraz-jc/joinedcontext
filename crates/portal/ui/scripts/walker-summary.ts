/**
 * T-4950: the live walker's reports (`walker-steward.json`, `walker-viewer.json`) as the summary
 * `tasks/file-failures` files tasks from, after each portal rollout to dev (UI-01, UI-16).
 *
 *     node scripts/walker-summary.ts <walk dir> <run> [--previous <summary.json>] [--merged T-1,T-2]
 *
 * One result per finding: its key is `<rule> <route> [<label>]`, which the board deduplicates on.
 * The same field without a hint at 1440 and 400 px, for the steward and the viewer, is one key and
 * so one task, and so is every unhinted field of one form (`ruleAndLabel`). The route keeps the section and tab a page belongs to and drops the item a walk
 * happened to open, so a finding keeps its task when the first item of a list changes. A key that failed in the previous run, on a
 * route this run walked, and is gone now is reported as a pass, which closes its task. The tasks
 * merged since the last clean walk are named in each detail: one of them most likely brought it.
 */
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

export interface WalkEntry {
  route: string;
  address: string;
  width: number;
  lang: string;
  findings: string[];
  screenshot: string;
}

export interface WalkerResult {
  key: string;
  verdict: "pass" | "fail" | "error";
  title: string;
  route?: string;
  detail?: string;
  evidence?: string;
}

export interface WalkerSummary {
  check: "walker";
  repo: "joinedcontext-portal";
  run: string;
  requirements: string[];
  admitted: string;
  results: WalkerResult[];
}

/** The owner order that has each finding filed as a task, so the board's freeze lets it be claimed. */
export const ADMITTED = "owner 2026-10-21T05:54Z (T-4950: each walker finding on dev becomes a task)";

export const PEOPLE = ["steward", "viewer"] as const;

/** The route a finding belongs to: the pattern, with its section and tab filled in from the address. */
export function routeKey(route: string, address: string): string {
  const pattern = route.split("/");
  const path = address.split(/[?#]/)[0].split("/");
  if (pattern.length !== path.length) return route;
  return pattern
    .map((segment, index) => {
      if (segment === "$plural" || segment === "$tab") return path[index];
      if (segment === "$") return path[index] === "new" ? "new" : "$";
      return segment;
    })
    .join("/");
}

/**
 * The finding's rule and the label that tells it apart on its route. A page-wording rule counts once
 * per route (five fields without a hint on one form are one fix, one task); a request, a console
 * error, an axe rule and a viewer's write control are told apart by what they name, with data
 * taken out.
 */
export function ruleAndLabel(finding: string): [string, string] {
  const at = finding.indexOf(":");
  const rule = at < 0 ? finding : finding.slice(0, at);
  const detail = at < 0 ? "" : finding.slice(at + 1).trim();
  // A path segment with a digit or a colon (but the API version) is an item, not the place: `403 GET /api/v1/…/*`.
  // The project too: the inbox asks every project the person is in, and one fault is one task.
  if (rule === "http") return [rule, detail.replace(/\/projects\/[^/\s]+/, "/projects/*").replace(/\/[^/\s]*[0-9:][^/\s]*/g, (segment) => (/^\/v\d+$/.test(segment) ? segment : "/*"))];
  if (rule === "console") return [rule, detail.replace(/\d+/g, "#").slice(0, 80)];
  if (rule === "axe") return [rule, detail.split(/\s/)[0]];
  if (rule === "viewer-write") return [rule, detail.slice(0, 120)];
  return [rule, ""];
}

/** A finding's key and title, as the board deduplicates on them. */
function keyOf(rule: string, route: string, label: string): string {
  return [rule, route, label].filter(Boolean).join(" ").replace(/\s+/g, " ").slice(0, 200);
}

export function summarise(
  walks: Partial<Record<(typeof PEOPLE)[number], WalkEntry[]>>,
  run: string,
  options: { previous?: WalkerSummary; merged?: string[]; logs?: Partial<Record<(typeof PEOPLE)[number], string>> } = {},
): WalkerSummary {
  const found = new Map<string, { rule: string; route: string; label: string; seen: string[]; evidence: string; samples: string[] }>();
  const walked = new Set<string>();
  const results: WalkerResult[] = [];
  for (const who of PEOPLE) {
    const entries = walks[who];
    // No report at all: the walk itself did not run (sign-in, the Portal down, the time cap).
    if (entries === undefined || entries.length === 0) {
      results.push({ key: `run ${who}`, verdict: "error", title: `Walker: the ${who}'s walk of dev wrote no report`, detail: walkError(options.logs?.[who]) });
      continue;
    }
    results.push({ key: `run ${who}`, verdict: "pass", title: `Walker: the ${who}'s walk` });
    for (const entry of entries) {
      const route = routeKey(entry.route, entry.address);
      walked.add(route);
      for (const finding of entry.findings) {
        const [rule, label] = ruleAndLabel(finding);
        const key = keyOf(rule, route, label);
        const at = `${who} ${entry.width}px ${entry.lang} ${entry.address}`;
        const known = found.get(key);
        if (known) {
          if (!known.seen.includes(at)) known.seen.push(at);
          if (known.samples.length < 3 && !known.samples.includes(finding)) known.samples.push(finding);
        } else found.set(key, { rule, route, label, seen: [at], evidence: entry.screenshot, samples: [finding] });
      }
    }
  }
  const merged = options.merged ?? [];
  const since = merged.length > 0 ? `Merged since the last clean walk: ${merged.join(", ")}.` : "No portal task merged since the last clean walk is known.";
  for (const [key, finding] of found) {
    results.push({
      key,
      verdict: "fail",
      route: finding.route,
      title: `Walker: ${finding.rule} on ${finding.route}${finding.label ? `: ${finding.label}` : ""}`.slice(0, 160),
      // Token-shaped text is redacted by tasks/file-failures before it reaches the board.
      detail: [
          `Found as: ${finding.samples.join(" | ")}`,
          `Seen by: ${finding.seen.slice(0, 8).join("; ")}${finding.seen.length > 8 ? ` and ${finding.seen.length - 8} more` : ""}`,
          since,
          "Rule: tests/pageChecks.ts (the walker's checks); excuse only with an open task in e2e/live/walker.allow.json.",
      ].join("\n"),
      evidence: finding.evidence,
    });
  }
  for (const result of options.previous?.results ?? []) {
    if (result.verdict !== "fail" || found.has(result.key) || result.route === undefined || !walked.has(result.route)) continue;
    results.push({ key: result.key, verdict: "pass", route: result.route, title: result.title });
  }
  return { check: "walker", repo: "joinedcontext-portal", run, requirements: ["UI-01", "UI-16"], admitted: ADMITTED, results };
}

/**
 * Why a walk wrote no report, from its log's first Playwright error (T-5993: the filed task said only
 * "See the run log", and the log was gone by the time a worker read it). A crashed tab is Chromium's
 * renderer killed by the host's OOM killer, not a Portal page: the hint says where to look.
 */
export function walkError(log: string | undefined): string {
  const error = log?.split("\n").find((line) => /^\s*Error: /.test(line))?.trim().slice(0, 300);
  if (!error) return "See the run log beside the evidence.";
  const crashed = /crashed/i.test(error) ? " A crashed tab is Chromium's renderer killed under host memory pressure (oom_kill in memory.events): compare with the host's free memory at the run's time before judging a page." : "";
  return `${error}${crashed}`;
}

function read(path: string): WalkEntry[] | undefined {
  if (!existsSync(path)) return undefined;
  return JSON.parse(readFileSync(path, "utf8")) as WalkEntry[];
}

export function main(argv: string[]): number {
  const flag = (name: string) => {
    const at = argv.indexOf(name);
    return at < 0 ? undefined : argv[at + 1];
  };
  const positional = argv.filter((arg, index) => !arg.startsWith("--") && !argv[index - 1]?.startsWith("--"));
  const [dir, run] = positional;
  if (dir === undefined || run === undefined) {
    console.error("usage: walker-summary <walk dir> <run> [--previous <summary.json>] [--merged T-1,T-2]");
    return 2;
  }
  const previousPath = flag("--previous");
  const previous = previousPath && existsSync(previousPath) ? (JSON.parse(readFileSync(previousPath, "utf8")) as WalkerSummary) : undefined;
  const merged = (flag("--merged") ?? "").split(",").filter((id) => /^T-\d{4,5}$/.test(id));
  const walks = Object.fromEntries(PEOPLE.map((who) => [who, read(join(dir, `walker-${who}.json`))]));
  const logs = Object.fromEntries(PEOPLE.map((who) => [who, existsSync(join(dir, `walk-${who}.log`)) ? readFileSync(join(dir, `walk-${who}.log`), "utf8") : undefined]));
  process.stdout.write(`${JSON.stringify(summarise(walks, run, { previous, merged, logs }), null, 2)}\n`);
  return 0;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) process.exit(main(process.argv.slice(2)));
