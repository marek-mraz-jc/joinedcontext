import { execFileSync, spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

// T-5161: the host cron kills rollout-walk.sh at the time `--budget` prints; it must cover the
// steward walk, then the viewer walk, and the journeys, each with its own cap and kill grace.
const budget = (env: Record<string, string>) =>
  Number(
    execFileSync("bash", ["e2e/live/rollout-walk.sh", "--budget"], {
      env: { ...process.env, WALK_EVIDENCE: "/nonexistent/never-created", ...env },
      encoding: "utf8",
    }).trim(),
  );

describe("rollout-walk --budget", () => {
  it("covers two tries of two walks and the journeys at the defaults", () => {
    expect(budget({})).toBeGreaterThanOrEqual(4 * 900 + 2 * 90 + 3600);
  });

  it("follows WALK_CAP and SWEEP_CAP", () => {
    expect(budget({ WALK_CAP: "1200", SWEEP_CAP: "60" })).toBeGreaterThanOrEqual(4 * 1200 + 60);
  });
});

// T-5993: a host short of memory OOM-kills Chromium's renderer mid-walk; the walk waits for a later
// tick instead of spending its three tries, and walks anyway after WALK_DEFER_MAX deferrals.
describe("rollout-walk memory gate", () => {
  const sha = "943ebc79cdc0eb95db7793f4a5c4535d5fba0864";
  const setup = (availableKb: number) => {
    const root = mkdtempSync(join(tmpdir(), "rollout-walk-"));
    mkdirSync(join(root, "portal/api/v1"), { recursive: true });
    writeFileSync(join(root, "portal/api/v1/version"), JSON.stringify({ portal: { commit: sha } }));
    writeFileSync(join(root, "meminfo"), `MemTotal: 31457280 kB\nMemAvailable: ${availableKb} kB\n`);
    return root;
  };
  // No users file: a walk that gets past the gate stops at "no steward or viewer password" (exit 2).
  const walk = (root: string, env: Record<string, string> = {}) =>
    spawnSync("bash", ["e2e/live/rollout-walk.sh"], {
      env: { ...process.env, PORTAL_URL: `file://${root}/portal`, WALK_EVIDENCE: join(root, "evidence"), WALK_MEMINFO: join(root, "meminfo"), DEMO_USERS: join(root, "none"), WALK_KUBECONFIG: join(root, "none"), WALK_SETTLE_S: "0", ...env },
      encoding: "utf8",
    });

  it("defers the walk while the host has less memory available than the walk needs", () => {
    const root = setup(2_000_000);
    const first = walk(root);
    expect(first.status).toBe(1);
    expect(first.stderr).toContain(`walk of ${sha} deferred (1 of 12)`);
    expect(readFileSync(join(root, "evidence/deferred"), "utf8").trim()).toBe("1");
    expect(existsSync(join(root, "evidence", sha, "tries"))).toBe(false);
  });

  it("walks anyway after WALK_DEFER_MAX deferrals, and at once with enough memory", () => {
    const short = setup(2_000_000);
    expect(walk(short, { WALK_DEFER_MAX: "1" }).status).toBe(1);
    const late = walk(short, { WALK_DEFER_MAX: "1" });
    expect(late.status).toBe(2);
    expect(late.stderr).toContain("no steward or viewer password");
    expect(existsSync(join(short, "evidence/deferred"))).toBe(false);
    const ample = walk(setup(12_000_000));
    expect(ample.status).toBe(2);
    expect(ample.stderr).toContain("no steward or viewer password");
  });
});

// T-6076: the walk that started a minute after a rollout met the edge's 502 on its first loads
// (13:17Z /api/v1/branding, 16:31Z /ai-connectors) while the dev sync still ran. A commit is walked
// only after it has been served for WALK_SETTLE_S (300 s), so the walk judges the settled Portal.
describe("rollout-walk settle gate", () => {
  const sha = "9be7dc6e721463d317c2c862c5b7dbea1a928e5d";
  const setup = () => {
    const root = mkdtempSync(join(tmpdir(), "rollout-walk-"));
    mkdirSync(join(root, "portal/api/v1"), { recursive: true });
    mkdirSync(join(root, "evidence"));
    writeFileSync(join(root, "portal/api/v1/version"), JSON.stringify({ portal: { commit: sha } }));
    writeFileSync(join(root, "meminfo"), "MemAvailable: 12000000 kB\n");
    return root;
  };
  const walk = (root: string) =>
    spawnSync("bash", ["e2e/live/rollout-walk.sh"], {
      env: { ...process.env, PORTAL_URL: `file://${root}/portal`, WALK_EVIDENCE: join(root, "evidence"), WALK_MEMINFO: join(root, "meminfo"), DEMO_USERS: join(root, "none"), WALK_KUBECONFIG: join(root, "none") },
      encoding: "utf8",
    });

  it("defers a commit dev has served for less than the settle time", () => {
    const root = setup();
    const first = walk(root);
    expect(first.status).toBe(1);
    expect(first.stderr).toContain(`walk of ${sha} deferred until the rollout settles`);
    expect(readFileSync(join(root, "evidence/first-seen"), "utf8")).toMatch(new RegExp(`^${sha} [0-9]+\n$`));
    expect(walk(root).status).toBe(1);
    expect(existsSync(join(root, "evidence", sha))).toBe(false);
  });

  it("walks a commit served for longer, and restarts the clock for a new commit", () => {
    const root = setup();
    const old = Math.floor(Date.now() / 1000) - 301;
    writeFileSync(join(root, "evidence/first-seen"), `${sha} ${old}\n`);
    const settled = walk(root);
    expect(settled.status).toBe(2);
    expect(settled.stderr).toContain("no steward or viewer password");
    writeFileSync(join(root, "evidence/first-seen"), `943ebc79cdc0eb95db7793f4a5c4535d5fba0864 ${old}\n`);
    expect(walk(root).status).toBe(1);
  });
});

// T-6174, T-6179: an apply during the walk (the next Portal commit, or a deployment apply under the
// same Portal commit: 8 /api reads answered 502 on /projects/helsinki/policies at 07:31:48Z while
// deployment 6215c731 was applied) judged dev mid-apply: its findings are not filed.
describe("rollout-walk apply during the walk", () => {
  const sha = "d8f3d2456906c85d4270b8ef0471ff7d1e4f1542";
  const next = "ab44055a8d9a8f7c1b1e8f0e1d6b0c4a2f3e5d71";
  const tool = (dir: string, name: string, body: string) => {
    writeFileSync(join(dir, name), `#!/usr/bin/env bash\n${body}\n`, { mode: 0o755 });
  };
  // git, pnpm, npx and kubectl stand in for the worktree, the install, Playwright and the cluster:
  // npx writes an empty walker report and, with ROLL_TO or APPLY_TO set, moves the Portal commit or
  // the jc-release record from then on; kubectl prints the jc-release record.
  const setup = () => {
    const root = mkdtempSync(join(tmpdir(), "rollout-walk-"));
    const bin = join(root, "bin");
    mkdirSync(join(root, "portal/api/v1"), { recursive: true });
    mkdirSync(join(root, "evidence"));
    mkdirSync(bin);
    writeFileSync(join(root, "portal/api/v1/version"), JSON.stringify({ portal: { commit: sha } }));
    writeFileSync(join(root, "release"), "6215c73196995e4acd6fc1f32ba1d0409e7a62c0 2026-10-30T07:05:52Z");
    writeFileSync(join(root, "kubeconfig"), "");
    writeFileSync(join(root, "meminfo"), "MemAvailable: 12000000 kB\n");
    writeFileSync(join(root, "users"), "demo.steward@hel.fi s-pass 123456\ndemo.viewer@hel.fi v-pass\n");
    tool(bin, "git", `[ "$3" = worktree ] && [ "$4" = add ] && mkdir -p "$7/ui/e2e/live" && echo WALK_REPORT_DIR >"$7/ui/e2e/live/walker.spec.ts"; exit 0`);
    tool(bin, "pnpm", "exit 0");
    tool(bin, "kubectl", 'case "$*" in *applyStartedAt*) cat "$RELEASE_FILE" ;; *) cut -d" " -f1,2 "$RELEASE_FILE" ;; esac');
    tool(bin, "npx", [
      'person=$(printf "%s\\n" "$@" | sed -n "s/^for the \\(.*\\)\\$$/\\1/p")',
      'echo "$*" >>"$WALK_REPORT_DIR/npx-calls"',
      '[ -n "${NO_REPORT:-}" ] && exit 1',
      'grep -q "^$person$" "$WALK_REPORT_DIR/npx-calls" || { [ "$person" = "${FIRST_TRY_FAILS:-}" ] && echo "$person" >>"$WALK_REPORT_DIR/npx-calls" && echo "Error: first" && exit 1; }',
      'echo "${REPORT:-[]}" >"$WALK_REPORT_DIR/walker-$person.json"',
      '[ -n "${ROLL_TO:-}" ] && printf \'{"portal":{"commit":"%s"}}\' "$ROLL_TO" >"$VERSION_FILE"',
      '[ -n "${APPLY_TO:-}" ] && printf "%s" "$APPLY_TO" >"$RELEASE_FILE"',
      "exit 0",
    ].join("\n"));
    return root;
  };
  const walk = (root: string, env: Record<string, string> = {}) =>
    spawnSync("bash", ["e2e/live/rollout-walk.sh"], {
      env: {
        ...process.env, PATH: `${join(root, "bin")}:${process.env.PATH}`, PORTAL_URL: `file://${root}/portal`,
        VERSION_FILE: join(root, "portal/api/v1/version"), RELEASE_FILE: join(root, "release"),
        WALK_EVIDENCE: join(root, "evidence"), WALK_MEMINFO: join(root, "meminfo"), DEMO_USERS: join(root, "users"),
        WALK_SETTLE_S: "0", WALK_KUBECONFIG: join(root, "kubeconfig"), WALK_JOURNEYS: "0", WALK_RETRY_WAIT: "0", TASKS_DIR: "", ...env,
      },
      encoding: "utf8",
    });
  const notFiled = (root: string) => {
    expect(existsSync(join(root, "evidence", sha, "walker-viewer.json"))).toBe(true);
    expect(existsSync(join(root, "evidence", sha, "summary.json"))).toBe(false);
    expect(existsSync(join(root, "evidence/last-summary.json"))).toBe(false);
    expect(readFileSync(join(root, "evidence/first-seen"), "utf8")).toMatch(new RegExp(`^${sha} [0-9]+\n$`));
  };

  // T-6510: Playwright runs a teardown project whatever --grep says, so the read-only walk ran the
  // sweep's exclusive specs after each person, and the viewer's sign-in lost the whole walk once.
  const report = '[{"route":"/","address":"/","width":1440,"lang":"en","findings":[],"screenshot":""}]';
  const calls = (root: string) => readFileSync(join(root, "evidence", sha, "npx-calls"), "utf8").split("\n").filter((line) => line.includes("playwright"));

  it("walks each person once, without the sweep's exclusive tail", () => {
    const root = setup();
    walk(root, { REPORT: report });
    expect(calls(root)).toHaveLength(2);
    for (const call of calls(root)) expect(call).toContain("--no-deps");
  });

  it("walks a person who wrote no report once more, keeping the first log", () => {
    const root = setup();
    const again = walk(root, { REPORT: report, FIRST_TRY_FAILS: "viewer" });
    expect(again.stderr).toContain("the viewer's walk wrote no report, once more in 0 s");
    expect(calls(root).filter((call) => call.includes("for the viewer"))).toHaveLength(2);
    expect(readFileSync(join(root, "evidence", sha, "walk-viewer.first.log"), "utf8")).toContain("Error: first");
    expect(readFileSync(join(root, "evidence", sha, "summary.json"), "utf8")).not.toContain("wrote no report");
  });

  it("still reports a walk that wrote nothing on both tries", () => {
    const root = setup();
    walk(root, { NO_REPORT: "1", REPORT: report });
    expect(calls(root)).toHaveLength(4);
  });

  it("files nothing when dev serves another Portal commit after the walk", () => {
    const root = setup();
    const moved = walk(root, { ROLL_TO: next });
    expect(moved.stderr).toContain(`dev moved from ${sha} to ${next} during the walk`);
    expect(moved.status).toBe(1);
    notFiled(root);
  });

  it("files nothing when dev was applied during the walk under the same Portal commit", () => {
    const root = setup();
    const applied = walk(root, { APPLY_TO: "6215c73196995e4acd6fc1f32ba1d0409e7a62c0 2026-10-30T07:29:59Z" });
    expect(applied.stderr).toContain("dev was applied during the walk");
    expect(applied.status).toBe(1);
    notFiled(root);
  });

  // T-6263: the 21:15Z walk of 5fb4aa4 began 1-3 min after a full sync under the same Portal commit
  // and met one 502 document load; the walk waits until the last apply is WALK_SETTLE_S old too.
  it("defers the walk while dev's last apply is younger than the settle time", () => {
    const root = setup();
    const now = Math.floor(Date.now() / 1000);
    const iso = (s: number) => new Date(s * 1000).toISOString().replace(/\.\d+Z$/, "Z");
    writeFileSync(join(root, "evidence/first-seen"), `${sha} ${now - 3600}\n`);
    writeFileSync(join(root, "release"), `dfa8139a94d0c5b1e2f3a4b5c6d7e8f9a0b1c2d3 ${iso(now - 60)}`);
    const young = walk(root, { WALK_SETTLE_S: "300" });
    expect(young.status).toBe(1);
    expect(young.stderr).toContain(`walk of ${sha} deferred until the apply settles`);
    expect(existsSync(join(root, "evidence", sha))).toBe(false);
    writeFileSync(join(root, "release"), `dfa8139a94d0c5b1e2f3a4b5c6d7e8f9a0b1c2d3 ${iso(now - 301)}`);
    const settled = walk(root, { WALK_SETTLE_S: "300" });
    expect(settled.stderr).not.toContain("deferred");
    expect(settled.stdout).toContain("journeys skipped");
  });

  // T-6425: the 07:15Z walk of e5b27f1 began inside the full sync of abcbe12f, which recorded only
  // after the walk ended, and filed a 503 met on the node that sync had at 94-99 % memory.
  describe("an apply still running", () => {
    const now = Math.floor(Date.now() / 1000);
    const iso = (s: number) => new Date(s * 1000).toISOString().replace(/\.\d+Z$/, "Z");
    const record = (root: string, appliedAgo: number, startedAgo: number) => {
      writeFileSync(join(root, "evidence/first-seen"), `${sha} ${now - 3600}\n`);
      writeFileSync(join(root, "release"), `abcbe12f94d0c5b1e2f3a4b5c6d7e8f9a0b1c2d3 ${iso(now - appliedAgo)} ${iso(now - startedAgo)}`);
    };

    it("defers the walk while applyStartedAt is newer than appliedAt", () => {
      const root = setup();
      record(root, 3600, 600);
      const running = walk(root, { WALK_SETTLE_S: "300" });
      expect(running.status).toBe(1);
      expect(running.stderr).toContain(`walk of ${sha} deferred until it ends`);
      expect(existsSync(join(root, "evidence", sha))).toBe(false);
    });

    it("walks once the apply recorded its end", () => {
      const root = setup();
      record(root, 301, 2000);
      const ended = walk(root, { WALK_SETTLE_S: "300" });
      expect(ended.stderr).not.toContain("deferred");
      expect(ended.stdout).toContain("journeys skipped");
    });

    it("stops waiting on the stamp of an apply that died", () => {
      const root = setup();
      record(root, 9000, 7201);
      const dead = walk(root, { WALK_SETTLE_S: "300" });
      expect(dead.stderr).not.toContain("deferred");
      expect(dead.stdout).toContain("journeys skipped");
    });

    it("files nothing when an apply began during the walk and had not ended after it", () => {
      const root = setup();
      const began = walk(root, { APPLY_TO: `6215c73196995e4acd6fc1f32ba1d0409e7a62c0 2026-10-30T07:05:52Z ${iso(now)}` });
      expect(began.stderr).toContain("dev was applied during the walk");
      expect(began.status).toBe(1);
      notFiled(root);
    });
  });

  // T-6334: the 11:10Z walk of 6250cbc was set aside (dev applied during it), the 11:35Z retry met
  // Chromium's launch failure and wrote no report, and the summary judged the set-aside walk's
  // reports left in the same directory: a 503 met mid-apply was filed. Each try starts clean.
  it("never judges the reports of a set-aside walk when the retry writes none", () => {
    const root = setup();
    expect(walk(root, { APPLY_TO: "6215c73196995e4acd6fc1f32ba1d0409e7a62c0 2026-10-30T07:29:59Z" }).status).toBe(1);
    const retry = walk(root, { NO_REPORT: "1" });
    expect(retry.status).toBe(1);
    expect(retry.stderr).toContain("no walker report (try 1 of 3)");
    expect(existsSync(join(root, "evidence", sha, "walker-steward.json"))).toBe(false);
    expect(existsSync(join(root, "evidence", sha, "summary.json"))).toBe(false);
  });

  it("judges a walk when dev stayed as it was, or its apply record cannot be read", () => {
    const kept = walk(setup());
    expect(kept.stderr).not.toContain("during the walk");
    expect(kept.stdout).toContain("journeys skipped");
    const unread = setup();
    const blind = walk(unread, { WALK_KUBECONFIG: join(unread, "none"), APPLY_TO: "x y" });
    expect(blind.stderr).not.toContain("during the walk");
    expect(blind.stdout).toContain("journeys skipped");
  });
});
