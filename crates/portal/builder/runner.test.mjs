// node --test builder/runner.test.mjs
// The build pod's runner (AP-130): with GITEA_RUNNER_ONCE=1 it registers once, runs one job and
// ends, so the Job completes; the registration token is gone from disk before the job starts and
// reaches `register` on stdin, never on a command line.
import { strict as assert } from "node:assert";
import { chmodSync, existsSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";

const runner = new URL("./runner.sh", import.meta.url).pathname;

function stub(dir, { failRegister = false } = {}) {
  const log = join(dir, "calls");
  const bin = join(dir, "bin");
  spawnSync("mkdir", ["-p", bin]);
  writeFileSync(
    join(bin, "gitea-runner"),
    `#!/bin/sh
case "$*" in
  *register*) printf 'register %s stdin=%s tokenfile=%s\\n' "$*" "$(cat)" "$(test -e "$TOKEN_FILE_SEEN" && echo present || echo gone)" >> "${log}"; ${failRegister ? "exit 1" : "exit 0"} ;;
  *daemon*) echo "daemon" >> "${log}" ;;
esac
`,
  );
  chmodSync(join(bin, "gitea-runner"), 0o755);
  return { bin, log };
}

test("a build pod's runner takes one job and ends, the token read once and never on a command line", () => {
  const dir = mkdtempSync(join(tmpdir(), "runner-"));
  const { bin, log } = stub(dir);
  const token = join(dir, "token");
  writeFileSync(token, "repository-scoped-token");
  const run = spawnSync("sh", [runner], {
    env: {
      PATH: `${bin}:/usr/bin:/bin`,
      GITEA_INSTANCE: "http://forge:3000",
      GITEA_RUNNER_TOKEN_FILE: token,
      GITEA_RUNNER_NAME: "build-bikes-42",
      GITEA_RUNNER_ONCE: "1",
      TOKEN_FILE_SEEN: token,
    },
    timeout: 10_000,
  });
  assert.equal(run.status, 0, run.stderr.toString());
  const calls = readFileSync(log, "utf8").trim().split("\n");
  assert.equal(calls.length, 2, calls.join("\n"));
  assert.match(calls[0], /--ephemeral/);
  assert.match(calls[0], /--name build-bikes-42-1/);
  assert.match(calls[0], /stdin=repository-scoped-token/);
  assert.doesNotMatch(calls[0].split(" stdin=")[0], /repository-scoped-token/);
  assert.match(calls[0], /tokenfile=gone/);
  assert.equal(calls[1], "daemon");
  assert.equal(existsSync(token), false);
});

test("a runner with no token refuses to start", () => {
  const dir = mkdtempSync(join(tmpdir(), "runner-"));
  const { bin } = stub(dir);
  const run = spawnSync("sh", [runner], {
    env: { PATH: `${bin}:/usr/bin:/bin`, GITEA_INSTANCE: "http://forge:3000", GITEA_RUNNER_TOKEN_FILE: join(dir, "none"), GITEA_RUNNER_ONCE: "1" },
    timeout: 10_000,
  });
  assert.notEqual(run.status, 0);
  assert.match(run.stderr.toString(), /no registration token/);
});
