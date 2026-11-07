// Its own module: a public form checks a reason without loading the explorer's filters (T-5165).
/** The longest `JC-Reason` the gateway takes (DM-113): 1 to 500 characters once trimmed. */
export const MAX_REASON = 500;

/** What is wrong with a reason a person typed, or null when the gateway takes it (DM-113). */
export function reasonProblem(reason: string): "required" | "tooLong" | null {
  const trimmed = reason.trim();
  if (trimmed === "") return "required";
  return [...trimmed].length > MAX_REASON ? "tooLong" : null;
}
