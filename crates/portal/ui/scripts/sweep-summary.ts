/**
 * T-2734: the live sweep's Playwright JSON report, as the summary `tasks/file-failures` files
 * tasks from (TS-01, TS-26).
 *
 *     node scripts/sweep-summary.ts <report.json> > summary.json
 *
 * One result per test: its key is `<spec> › <title>`, which is what the board deduplicates on,
 * so a test keeps its task across runs and a new failure gets a new one. The detail is the first
 * error, one paragraph, with every demo password of the environment and anything token-shaped
 * taken out: the board is a shared mount and the summary is written beside the run. The
 * evidence is the failure's screenshot when the test took one, never a trace (a trace records
 * what was typed into the sign-in form).
 */
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

interface Attachment {
  name: string;
  path?: string;
  contentType: string;
}

interface Result {
  status?: string;
  error?: { message?: string };
  errors?: { message: string }[];
  attachments?: Attachment[];
}

interface Suite {
  title: string;
  file: string;
  specs: { title: string; file: string; tests: { status: string; results: Result[] }[] }[];
  suites?: Suite[];
}

export interface Report {
  suites: Suite[];
}

export interface SweepResult {
  key: string;
  verdict: "pass" | "fail" | "error" | "skip";
  title: string;
  detail?: string;
  evidence?: string;
}

export interface Summary {
  check: "live-sweep";
  repo: "joinedcontext-portal";
  run: string;
  requirements: string[];
  results: SweepResult[];
}

/** The environment's demo passwords (e2e/live/portal.ts reads the same names). */
const PASSWORDS = ["PORTAL_PASSWORD", "APPROVER_PASSWORD", "VIEWER_PASSWORD", "EDITOR_PASSWORD", "JANITOR_PASSWORD"];

/** The terminal colours Playwright puts in an error message (ESC [ … m). */
const COLOURS = new RegExp(`${String.fromCharCode(27)}\\[[0-9;]*m`, "g");

/** Text with every known password and anything that looks like a credential taken out. */
export function scrub(text: string, env: Record<string, string | undefined>): string {
  let out = text.replace(COLOURS, "");
  for (const name of PASSWORDS) {
    const value = env[name];
    if (value && value.length >= 4) out = out.split(value).join("[redacted]");
  }
  return out
    .replace(/\beyJ[A-Za-z0-9_-]{5,}\.[A-Za-z0-9_-]{5,}\.[A-Za-z0-9_-]*/g, "[redacted jwt]")
    .replace(/(?:bearer|basic)\s+[A-Za-z0-9._~+/=-]{8,}/gi, "[redacted]")
    .replace(/((?:password|secret|token)["']?\s*[:=,]\s*["']?)[^\s"',)]+/gi, "$1[redacted]");
}

function* specsOf(suites: Suite[]): Generator<Suite["specs"][number]> {
  for (const suite of suites) {
    yield* suite.specs;
    yield* specsOf(suite.suites ?? []);
  }
}

/** The report as file-failures reads it; `run` is the time of the sweep. */
export function summarise(report: Report, run: string, env: Record<string, string | undefined>): Summary {
  const results: SweepResult[] = [];
  for (const spec of specsOf(report.suites)) {
    for (const test of spec.tests) {
      const key = `${spec.file.replace(/^.*e2e\/live\//, "")} › ${spec.title}`;
      const last = test.results.at(-1);
      // `flaky` passed on a retry: green for the board, as the run itself counted it.
      if (test.status === "expected" || test.status === "flaky") {
        results.push({ key, verdict: "pass", title: key });
        continue;
      }
      if (test.status === "skipped") {
        results.push({ key, verdict: "skip", title: key });
        continue;
      }
      const message = last?.error?.message ?? last?.errors?.[0]?.message ?? `ended ${last?.status ?? "without a result"}`;
      // A test that never got going (the sign-in, the instance) is the run's failure, not the page's.
      const verdict = last?.status === "interrupted" ? "error" : "fail";
      const detail = scrub(message, env).replace(/\s+/g, " ").trim().slice(0, 600);
      const screenshot = last?.attachments?.find((attachment) => attachment.contentType === "image/png" && attachment.path);
      results.push({
        key,
        verdict,
        title: `Live sweep: ${spec.title}`.slice(0, 160),
        detail,
        ...(screenshot?.path ? { evidence: screenshot.path } : {}),
      });
    }
  }
  return { check: "live-sweep", repo: "joinedcontext-portal", run, requirements: ["TS-01", "TS-26"], results };
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const [path] = process.argv.slice(2);
  if (path === undefined) {
    console.error("usage: sweep-summary <playwright report.json>");
    process.exit(2);
  }
  const report = JSON.parse(readFileSync(path, "utf8")) as Report;
  process.stdout.write(`${JSON.stringify(summarise(report, new Date().toISOString(), process.env), null, 2)}\n`);
}
