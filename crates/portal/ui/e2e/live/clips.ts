/**
 * T-6454: the camera of the clip journeys (clips_*.spec.ts). A clip is one recorded journey at one
 * size. Each step holds its result on screen long enough to be read and leaves a line in the
 * caption file, so a person reads the video and a judge reads a still of each state.
 *
 * Every clip runs at both sizes in ONE Playwright invocation, since Playwright empties test-results
 * per run (T-3308). `CLIP_DIR` receives <journey>-<width>.webm, .vtt and .json (step timings and the
 * Portal commit); scripts/clips.sh turns them into mp4 and stills. Whoever calls `step` keeps every
 * secret off the screen: the context of a clip records all it shows.
 */
import { mkdirSync, renameSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { expect } from "@playwright/test";
import type { Browser, BrowserContext, BrowserContextOptions, Page } from "@playwright/test";
import { portalReady, signIn, withLocalUi } from "./portal";
import { shareEdge } from "./edgeShare";

export const CLIP_DIR = process.env.CLIP_DIR ?? "test-results/clips";
/** Enough for a still every 2 s to catch each state twice. */
const HOLD_MS = 4_000;

export const SIZES = [
  { width: 1440, height: 900 },
  { width: 400, height: 844 },
];
type Size = (typeof SIZES)[number];

interface Step {
  caption: string;
  startS: number;
  endS: number;
  /** How long the person waited for the step, the hold left out. */
  tookS: number;
}

const stamp = (seconds: number) => {
  const date = new Date(Math.round(seconds * 1000)).toISOString();
  return date.slice(11, 23);
};

/** The WebVTT text of a clip's steps. */
export const captions = (steps: Step[]): string =>
  `WEBVTT\n\n${steps.map((one) => `${stamp(one.startS)} --> ${stamp(one.endS)}\n${one.caption}\n`).join("\n")}`;

export class Clip {
  private steps: Step[] = [];
  private started = Date.now();

  constructor(
    readonly page: Page,
    readonly context: BrowserContext,
    private name: string,
  ) {}

  /** One thing the person does: the caption, the action, then the result held on screen. */
  async step(caption: string, action: () => Promise<void>, holdMs = HOLD_MS): Promise<void> {
    const start = Date.now();
    await action();
    const done = Date.now();
    await this.page.waitForTimeout(holdMs);
    const seconds = (at: number) => (at - this.started) / 1000;
    this.steps.push({ caption, startS: seconds(start), endS: seconds(Date.now()), tookS: (done - start) / 1000 });
  }

  /** Saves the video beside its captions; a failed journey keeps what it recorded. */
  async finish(): Promise<void> {
    const video = this.page.video();
    await this.context.close();
    mkdirSync(CLIP_DIR, { recursive: true });
    if (video) renameSync(await video.path(), join(CLIP_DIR, `${this.name}.webm`));
    writeFileSync(join(CLIP_DIR, `${this.name}.vtt`), captions(this.steps));
    writeFileSync(join(CLIP_DIR, `${this.name}.json`), JSON.stringify({ name: this.name, steps: this.steps }, null, 1));
  }
}

const recorded = (size: Size): BrowserContextOptions => ({
  viewport: size,
  recordVideo: { dir: join(CLIP_DIR, "raw"), size },
});

/**
 * A clip of a signed-in person. The phone cannot sign in on camera (goSignedIn wants the desktop
 * landmarks, KNOWLEDGE T-5981), so every clip signs in off camera and starts on `path`; the one
 * journey that shows the sign-in passes `onCamera` and runs at 1440.
 */
export async function clipOf(
  browser: Browser,
  who: { user: string; password: string },
  journey: string,
  size: Size,
  path: string,
  onCamera = false,
): Promise<Clip> {
  const name = `${journey}-${size.width}`;
  if (onCamera) {
    const { context, page } = await signIn(browser, who, path, portalReady, recorded(size));
    return new Clip(page, context, name);
  }
  const off = await signIn(browser, who, "/?lang=en");
  const storageState = await off.context.storageState();
  await off.context.close();
  const context = await browser.newContext({ ...recorded(size), storageState });
  await withLocalUi(context);
  await shareEdge(context);
  const page = await context.newPage();
  await page.goto(path, { waitUntil: "load" });
  return new Clip(page, context, name);
}

/** A clip of a visitor with no session. */
export async function clipOfVisitor(browser: Browser, journey: string, size: Size, path: string): Promise<Clip> {
  const context = await browser.newContext(recorded(size));
  await withLocalUi(context);
  await shareEdge(context);
  const page = await context.newPage();
  await page.goto(path, { waitUntil: "load" });
  return new Clip(page, context, `${journey}-${size.width}`);
}

/** No bearer token, JWT or client secret in what the page says. */
export async function noSecretOnScreen(page: Page): Promise<void> {
  await expect(page.locator("body")).not.toContainText(/eyJ[A-Za-z0-9_-]{8}|Bearer [A-Za-z0-9]|client.secret\s*[:=]/i);
}
