/** How a platform service's refusal reads to a person: which layer switched it off, or which quota is spent. */
import { ServiceRefusedError } from "@joinedcontext/sdk";

export function refusal(err: unknown): string {
  if (!(err instanceof ServiceRefusedError)) return err instanceof Error ? err.message : String(err);
  if (err.layer) return `${err.service} is switched off for this ${err.layer}`;
  if (err.quota) return err.resetAt ? `${err.quota} is used up until ${err.resetAt}` : `${err.quota} is used up`;
  return err.message;
}
