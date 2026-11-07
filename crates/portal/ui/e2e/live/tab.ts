/**
 * A walker tab that outlives a renderer crash (T-5730). Chromium's renderer can die under host
 * memory pressure ("Target crashed"); every later call on that page throws, so one crash ended the
 * steward's whole walk with an empty report. `run` notices the crash, opens a fresh tab in the same
 * signed-in context (cookies and routes are the context's) and tells the caller, which reports the
 * crash on the page it hit and walks on.
 *
 * `net::ERR_NETWORK_CHANGED` is Chromium dropping its sockets because an address of the machine it
 * runs on moved, which a container starting on the walker's host does. No server can answer it, and
 * it ended the viewer's walk at the first list with an empty report and the steward's halfway
 * (T-6476), so `run` runs the work again on the same tab, `NETWORK_TRIES` times at most.
 */
import type { BrowserContext, Page } from "@playwright/test";

export type Outcome<T> = { crashed: false; value: T } | { crashed: true };

const NETWORK_TRIES = 3;
const networkChanged = (error: unknown) => String(error).includes("net::ERR_NETWORK_CHANGED");

export class Tab {
  page!: Page;
  private crashed = false;

  /** `setup` runs on every page the tab opens: the listeners the walk judges by. */
  constructor(
    private readonly context: BrowserContext,
    page: Page,
    private readonly setup: (page: Page) => void,
    private readonly networkSettleMs = 3_000,
  ) {
    this.adopt(page);
  }

  private adopt(page: Page): void {
    this.page = page;
    this.crashed = false;
    page.on("crash", () => {
      this.crashed = true;
    });
    this.setup(page);
  }

  /**
   * Runs `work` on the tab. A crash under it opens a fresh tab; the machine's network changing under
   * it runs it again, so `work` starts from nothing; any other error is `work`'s own and is thrown.
   */
  async run<T>(work: (page: Page) => Promise<T>): Promise<Outcome<T>> {
    for (let tries = 1; ; tries += 1) {
      try {
        return { crashed: false, value: await work(this.page) };
      } catch (error) {
        if (this.crashed) {
          const dead = this.page;
          this.adopt(await this.context.newPage());
          await dead.close().catch(() => undefined);
          return { crashed: true };
        }
        if (tries >= NETWORK_TRIES || !networkChanged(error)) throw error;
        await new Promise((settled) => setTimeout(settled, this.networkSettleMs));
      }
    }
  }

  /**
   * Runs `work`, and once more on the fresh tab when the renderer crashed under it (`again` true).
   * The host's OOM killer takes a renderer on whatever page is open, so one crash names no page; a
   * page that crashes again on a fresh tab does (T-6009).
   */
  async twice<T>(
    work: (page: Page, again: boolean) => Promise<T>,
  ): Promise<Outcome<T>> {
    const first = await this.run((page) => work(page, false));
    return first.crashed ? this.run((page) => work(page, true)) : first;
  }
}

/**
 * Opens a page in `context` and runs `go` on it, once more on a fresh page when the renderer
 * crashed under it. The OOM killer took the viewer's tab while it signed in, before the walk's own
 * `Tab` existed, and the whole walk wrote no report (T-6011). Two crashes in a row still throw.
 */
export async function openedTwice(
  context: BrowserContext,
  go: (page: Page) => Promise<void>,
  what: string,
): Promise<Page> {
  const tab = new Tab(context, await context.newPage(), () => undefined);
  const outcome = await tab.twice((page) => go(page));
  if (outcome.crashed) throw new Error(`the browser tab crashed twice while ${what}`);
  return tab.page;
}
