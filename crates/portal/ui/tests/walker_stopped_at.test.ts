import type { Page } from "@playwright/test";
import { describe, expect, it } from "vitest";
import { stoppedAt } from "../e2e/live/portal";

// T-6510: a sign-in that never came ready said only "expect(locator).toBeVisible() failed".
const on = (url: string, text: string, title = "Sign in") =>
  ({
    url: () => url,
    title: async () => title,
    locator: () => ({ innerText: async () => text }),
  }) as unknown as Page;

describe("stoppedAt", () => {
  it("names the page, its words and what was refused, without the query", () => {
    const said = stoppedAt(on("https://idm.dev.joinedcontext.com/realms/dev/login?code=secret&state=s", "We are\n  sorry"), ["429 /assets/a.js", "429 /assets/a.js", "503 /api/v1/me"]);
    return said.then((line) => {
      expect(line).toContain("https://idm.dev.joinedcontext.com/realms/dev/login (title \"Sign in\", text \"We are sorry\"");
      expect(line).toContain("3 answers >= 400: 429 /assets/a.js, 503 /api/v1/me");
      expect(line).not.toContain("secret");
    });
  });

  it("still answers from a page that cannot be read", async () => {
    const dead = {
      url: () => "about:blank",
      title: () => Promise.reject(new Error("Target crashed")),
      locator: () => ({ innerText: () => Promise.reject(new Error("Target crashed")) }),
    } as unknown as Page;
    expect(await stoppedAt(dead, [])).toContain("0 answers >= 400");
  });
});
