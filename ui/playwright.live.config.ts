import { defineConfig, devices } from "@playwright/test";
import { projects } from "./e2e/live/stateGroups";

// The journeys of ui/e2e/live run against a live Portal (dev), signed in through Keycloak as
// the demo people, with no stubbed API: what they prove is the whole path (T-0630, TS-12).
//   PORTAL_URL=https://portal.… PORTAL_PASSWORD=… APPROVER_PASSWORD=… VIEWER_PASSWORD=… \
//     EDITOR_PASSWORD=… npx playwright test --config playwright.live.config.ts
// Passwords come from the environment only, read from the cluster Secret at run time: each demo
// person's is Secret `keycloak-user-demo-<name>` (key `password`), so `demo.editor` is
// `keycloak-user-demo-editor` (T-2231). The journeys of a workload's own token (T-1595, T-1596)
// read PROPOSER_CLIENT_SECRET, the `client-secret` of Secret
// `keycloak-client-helsinki-pipeline-proposer` (T-2245). The forge journeys (T-2647) reach the forge
// at JC_FORGE_URL, by default the Portal URL's host without `portal.` plus `/git` (e2e/live/forge.ts).
// LIVE_WORKERS (default 1) runs that many specs at once; the sweep sets it (T-6404). The tests of one
// spec still run in order on one worker, and e2e/live/stateGroups.ts keeps specs that share state apart.
export default defineConfig({
  testDir: "./e2e/live",
  fullyParallel: false,
  workers: Number(process.env.LIVE_WORKERS ?? "1") || 1,
  retries: 0,
  reporter: "list",
  timeout: 420_000,
  use: {
    baseURL: process.env.PORTAL_URL ?? "https://portal.dev.joinedcontext.com",
    // Without this a click on a locator that matches nothing waits for the whole test timeout: one
    // wrong locator cost a ten-minute run that said only "Test timeout exceeded".
    actionTimeout: 30_000,
    trace: "retain-on-failure",
    viewport: { width: 1600, height: 1000 },
    // Every run a journey starts is a test run (AG-93, T-2816): the Portal records it as
    // `origin: journey` and the Assistant page leaves it out of the history people read. The
    // Portal accepts it only from the demo people it names in JC_PORTAL_JOURNEY_USERS, and never
    // beside a bearer token; anyone else who sends it is refused with 403.
    extraHTTPHeaders: { "X-JC-Run-Origin": "journey" },
  },
  projects: projects({
    ...devices["Desktop Chrome"],
    // A container's /dev/shm is 64 MB by default: Chromium's shared memory there runs out on the
    // walker's large pages and the tab dies ("Target crashed", T-5992). /tmp has the room.
    launchOptions: { args: ["--disable-dev-shm-usage"] },
  }),
});
