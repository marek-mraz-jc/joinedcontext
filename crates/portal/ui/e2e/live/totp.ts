/**
 * The second factor of a demo person who must sign in with TOTP (deployment T-5120: holders of the
 * realm role `mfa-required`, so demo.steward and demo.approver). The seed is the `totp` key of
 * Secret keycloak-user-<name>, handed in by environment (PORTAL_TOTP, APPROVER_TOTP) and never
 * logged.
 */
import { createHmac } from "node:crypto";
import { closeSync, openSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { Page } from "@playwright/test";

const STEP_S = 30;

/** RFC 6238 over the seed's UTF-8 bytes with HMAC-SHA256, 6 digits: the realm's OTP policy. */
export function totp(seed: string, step: number): string {
  const counter = Buffer.alloc(8);
  counter.writeBigUInt64BE(BigInt(step));
  const mac = createHmac("sha256", Buffer.from(seed, "utf8")).update(counter).digest();
  const offset = mac[mac.length - 1] & 0x0f;
  return String((mac.readUInt32BE(offset) & 0x7fffffff) % 1_000_000).padStart(6, "0");
}

/**
 * A code Keycloak has not seen yet. It refuses a code used once (otpPolicyCodeReusable false) and
 * accepts the step before and after the current one (look-ahead 1), so three sign-ins of one person
 * fit in a step; a fourth waits for the next. The claim is a file created exclusively, so parallel
 * workers on one host never hand out the same code.
 */
export async function unusedCode(user: string, seed: string, now: () => number = Date.now, dir = tmpdir()): Promise<string> {
  for (;;) {
    const step = Math.floor(now() / 1000 / STEP_S);
    for (const candidate of [step, step + 1, step - 1]) {
      const code = totp(seed, candidate);
      try {
        closeSync(openSync(join(dir, `jc-totp-${user.replace(/[^a-z0-9.]/gi, "_")}-${candidate}`), "wx"));
        return code;
      } catch (error) {
        if ((error as NodeJS.ErrnoException).code !== "EEXIST") throw error;
      }
    }
    await new Promise((resolve) => setTimeout(resolve, (step + 1) * STEP_S * 1000 - now() + 250));
  }
}

/** Answers Keycloak's TOTP form (`#otp`, `#kc-login`) when it is the page shown; false when it is not. */
export async function answerOtp(page: Page, who: { user: string; totp?: string }): Promise<boolean> {
  if (!(await page.locator("#otp").count())) return false;
  if (!who.totp) {
    throw new Error(
      `Keycloak asks ${who.user} for a TOTP code and no seed is in the environment ` +
        "(PORTAL_TOTP / APPROVER_TOTP: Secret keycloak-user-<name>, key totp)",
    );
  }
  await page.fill("#otp", await unusedCode(who.user, who.totp));
  await submitted(page, () => page.click("#kc-login"));
  return true;
}

/**
 * Clicks and waits for the next document's load. `waitForURL(() => true)` matched the page being
 * left and returned at once, so a sign-in looked at the old form while Keycloak was still answering
 * and stopped before the TOTP form came (T-5176: the steward's walk of dev wrote no report).
 */
export async function submitted(page: Page, click: () => Promise<unknown>): Promise<void> {
  await Promise.all([page.waitForEvent("load"), click()]);
}
