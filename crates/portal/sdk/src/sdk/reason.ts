/** The longest reason the gateway takes in `JC-Reason` (DM-113): 1 to 500 characters. */
export const MAX_REASON = 500;

/** Whether `reason` is one the gateway takes: 1 to 500 characters once trimmed, as it counts them (DM-113). */
export function reasonOk(reason: unknown): reason is string {
  if (typeof reason !== "string") return false;
  const length = [...reason.trim()].length;
  return length > 0 && length <= MAX_REASON;
}
