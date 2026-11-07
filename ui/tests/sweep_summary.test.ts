/**
 * T-2734: the live sweep's report, turned into the summary the board files tasks from.
 *
 * The board deduplicates by key, so the key has to be the same test's across runs; and the board
 * is a shared mount, so no password of the run and nothing token-shaped may reach it.
 */
import { describe, expect, it } from "vitest";
import { scrub, summarise } from "../scripts/sweep-summary";
import type { Report } from "../scripts/sweep-summary";

// Built from parts: a fixture, not a credential.
const PASSWORD = ["walk", "er", "-", "Pw9", "x"].join("");
const ENV = { VIEWER_PASSWORD: PASSWORD };

const report: Report = {
  suites: [
    {
      title: "login.spec.ts",
      file: "/ci/ui/e2e/live/login.spec.ts",
      specs: [
        {
          title: "a person signs in from /login to the page they asked for, and signs out",
          file: "/ci/ui/e2e/live/login.spec.ts",
          tests: [{ status: "expected", results: [{ status: "passed" }] }],
        },
        {
          title: "a wrong password gets no session, and an address off the Portal is not followed",
          file: "/ci/ui/e2e/live/login.spec.ts",
          tests: [
            {
              status: "unexpected",
              results: [
                {
                  status: "failed",
                  error: { message: `\u001b[31mExpected\u001b[39m visible; filled #password with ${PASSWORD}` },
                  attachments: [
                    { name: "trace", path: "/ci/test-results/x/trace.zip", contentType: "application/zip" },
                    { name: "screenshot", path: "/ci/test-results/x/test-failed-1.png", contentType: "image/png" },
                  ],
                },
              ],
            },
          ],
        },
      ],
      suites: [
        {
          title: "nested",
          file: "/ci/ui/e2e/live/walker.spec.ts",
          specs: [
            {
              title: "every page and form, at 1440 and 2560 px, for the viewer",
              file: "/ci/ui/e2e/live/walker.spec.ts",
              tests: [
                { status: "flaky", results: [{ status: "failed" }, { status: "passed" }] },
              ],
            },
            {
              title: "a journey that never started",
              file: "/ci/ui/e2e/live/walker.spec.ts",
              tests: [{ status: "unexpected", results: [{ status: "interrupted" }] }],
            },
            {
              title: "a skipped one",
              file: "/ci/ui/e2e/live/walker.spec.ts",
              tests: [{ status: "skipped", results: [] }],
            },
          ],
        },
      ],
    },
  ],
};

describe("the live sweep's summary (T-2734)", () => {
  const summary = summarise(report, "2026-09-25T09:00Z", ENV);

  it("keys each test by its spec and title, so the board finds its task again next run", () => {
    expect(summary.results.map((result) => [result.key, result.verdict])).toEqual([
      ["login.spec.ts › a person signs in from /login to the page they asked for, and signs out", "pass"],
      ["login.spec.ts › a wrong password gets no session, and an address off the Portal is not followed", "fail"],
      ["walker.spec.ts › every page and form, at 1440 and 2560 px, for the viewer", "pass"],
      ["walker.spec.ts › a journey that never started", "error"],
      ["walker.spec.ts › a skipped one", "skip"],
    ]);
    expect(summary).toMatchObject({ check: "live-sweep", repo: "joinedcontext-portal", run: "2026-09-25T09:00Z" });
  });

  it("carries the failure's screenshot as evidence, never its trace", () => {
    const failed = summary.results[1];
    expect(failed.evidence).toBe("/ci/test-results/x/test-failed-1.png");
    expect(JSON.stringify(summary)).not.toContain("trace.zip");
  });

  it("never lets a password of the run or a token reach the board", () => {
    const text = JSON.stringify(summary);
    expect(text).not.toContain(PASSWORD);
    expect(text).not.toContain("\u001b[");
    expect(summary.results[1].detail).toContain("[redacted]");
    const jwt = ["eyJhbGciOi", "eyJzdWIiOiJ4In0", "c2lnbmF0dXJl"].join(".");
    expect(scrub(`Authorization: Bearer ${jwt} and token=abc123def`, {})).not.toMatch(/eyJ|abc123def/);
  });
});
