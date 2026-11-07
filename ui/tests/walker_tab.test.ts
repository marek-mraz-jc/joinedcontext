import type { BrowserContext, Page } from "@playwright/test";
import { describe, expect, it } from "vitest";
import { openedTwice, Tab } from "../e2e/live/tab";

// T-5730: a renderer crash ("Target crashed") in one page ended the steward's walk with no report.
// The tab opens a fresh page in the same context and the walk goes on.
function fakePage() {
  const handlers: Record<string, (() => void)[]> = {};
  return {
    closed: false,
    on(event: string, handler: () => void) {
      (handlers[event] ??= []).push(handler);
    },
    crash() {
      for (const handler of handlers.crash ?? []) handler();
    },
    async close() {
      this.closed = true;
    },
  };
}

function tabOver() {
  const pages = [fakePage()];
  const set: unknown[] = [];
  const context = {
    async newPage() {
      const page = fakePage();
      pages.push(page);
      return page;
    },
  } as unknown as BrowserContext;
  const tab = new Tab(context, pages[0] as unknown as Page, (page) =>
    set.push(page),
  );
  return { tab, pages, set };
}

describe("walker Tab", () => {
  it("answers the work's value on a live tab", async () => {
    const { tab, pages } = tabOver();
    expect(await tab.run(async () => 7)).toEqual({ crashed: false, value: 7 });
    expect(tab.page).toBe(pages[0]);
  });

  it("opens a fresh tab, set up again, after the renderer crashed", async () => {
    const { tab, pages, set } = tabOver();
    const outcome = await tab.run(async () => {
      pages[0].crash();
      throw new Error("locator.evaluateAll: Target crashed");
    });
    expect(outcome).toEqual({ crashed: true });
    expect(pages).toHaveLength(2);
    expect(tab.page).toBe(pages[1]);
    expect(set).toEqual([pages[0], pages[1]]);
    expect(pages[0].closed).toBe(true);
    expect(await tab.run(async (page) => page)).toEqual({
      crashed: false,
      value: pages[1],
    });
  });

  it("throws any other error", async () => {
    const { tab, pages } = tabOver();
    await expect(
      tab.run(async () => {
        throw new Error("Timeout 30000ms exceeded");
      }),
    ).rejects.toThrow("Timeout");
    expect(pages).toHaveLength(1);
  });

  // T-6009: the host's OOM killer took a renderer on /endpoints and the walker filed the page.
  it("runs the work again on a fresh tab after one crash", async () => {
    const { tab, pages } = tabOver();
    const seen: boolean[] = [];
    const outcome = await tab.twice(async (page, again) => {
      seen.push(again);
      if (!again) {
        pages[0].crash();
        throw new Error("page.goto: Page crashed");
      }
      return page;
    });
    expect(outcome).toEqual({ crashed: false, value: pages[1] });
    expect(seen).toEqual([false, true]);
  });

  it("reports a crash only when the fresh tab crashes too", async () => {
    const { tab, pages } = tabOver();
    const outcome = await tab.twice(async () => {
      pages[pages.length - 1].crash();
      throw new Error("page.goto: Target crashed");
    });
    expect(outcome).toEqual({ crashed: true });
    expect(pages).toHaveLength(3);
  });

  it("runs the work once on a live tab", async () => {
    const { tab } = tabOver();
    let runs = 0;
    expect(await tab.twice(async () => ++runs)).toEqual({
      crashed: false,
      value: 1,
    });
    expect(runs).toBe(1);
  });
});

// T-6011: the OOM killer took the viewer's tab while it signed in, before the walk's Tab existed.
describe("walker sign-in tab", () => {
  function contextOver() {
    const pages: ReturnType<typeof fakePage>[] = [];
    const context = {
      async newPage() {
        const page = fakePage();
        pages.push(page);
        return page;
      },
    } as unknown as BrowserContext;
    return { context, pages };
  }

  it("signs in again on a fresh page after the renderer crashed", async () => {
    const { context, pages } = contextOver();
    const signedIn: unknown[] = [];
    const page = await openedTwice(
      context,
      async (page) => {
        if (pages.length === 1) {
          pages[0].crash();
          throw new Error("page.waitForTimeout: Page crashed");
        }
        signedIn.push(page);
      },
      "viewer signed in",
    );
    expect(page).toBe(pages[1]);
    expect(signedIn).toEqual([pages[1]]);
    expect(pages[0].closed).toBe(true);
  });

  it("throws, naming the step, when the fresh page crashes too", async () => {
    const { context, pages } = contextOver();
    await expect(
      openedTwice(
        context,
        async () => {
          pages[pages.length - 1].crash();
          throw new Error("page.goto: Page crashed");
        },
        "viewer signed in",
      ),
    ).rejects.toThrow("the browser tab crashed twice while viewer signed in");
  });

  it("passes a sign-in error that is no crash through", async () => {
    const { context } = contextOver();
    await expect(
      openedTwice(
        context,
        async () => {
          throw new Error("no password");
        },
        "viewer signed in",
      ),
    ).rejects.toThrow("no password");
  });
});

describe("walker Tab when the machine's network changes (T-6476)", () => {
  const change = () => new Error("page.goto: net::ERR_NETWORK_CHANGED at https://portal.dev/catalogue?lang=en");
  const over = () => {
    const { pages } = tabOver();
    return new Tab({} as BrowserContext, pages[0] as unknown as Page, () => undefined, 0);
  };

  it("runs the work again on the same tab, and the walk goes on", async () => {
    const tab = over();
    const page = tab.page;
    let calls = 0;
    const outcome = await tab.twice(async () => {
      calls += 1;
      if (calls < 3) throw change();
      return "walked";
    });
    expect(outcome).toEqual({ crashed: false, value: "walked" });
    expect(calls).toBe(3);
    expect(tab.page).toBe(page);
  });

  it("throws when it keeps changing: no walk is reported from a network that never held", async () => {
    let calls = 0;
    await expect(
      over().run(async () => {
        calls += 1;
        throw change();
      }),
    ).rejects.toThrow("ERR_NETWORK_CHANGED");
    expect(calls).toBe(3);
  });

  it("runs no other failure twice: a refused or unanswered load stays the walk's error", async () => {
    let calls = 0;
    await expect(
      over().run(async () => {
        calls += 1;
        throw new Error("page.goto: net::ERR_CONNECTION_REFUSED at https://portal.dev/");
      }),
    ).rejects.toThrow("ERR_CONNECTION_REFUSED");
    expect(calls).toBe(1);
  });

  it("signs in again when it changes under the sign-in", async () => {
    const { pages } = tabOver();
    const context = { newPage: async () => pages[0] } as unknown as BrowserContext;
    let calls = 0;
    const page = await openedTwice(
      context,
      async () => {
        calls += 1;
        if (calls === 1) throw change();
      },
      "signed in",
    );
    expect(page).toBe(pages[0]);
    expect(calls).toBe(2);
  });
});
