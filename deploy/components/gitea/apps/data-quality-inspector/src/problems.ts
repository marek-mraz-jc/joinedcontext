import { t } from "./i18n";
import type { Lang, Word } from "./i18n";
import { Problem } from "./server";

/** What the server's refusal means to the reader, in their language. */
export function problemText(lang: Lang, error: unknown): string {
  if (!(error instanceof Problem)) return t(lang, "serverDown");
  const words: Partial<Record<number, Word>> = { 0: "serverOffline", 401: "signInAgain", 403: "noRight", 404: "gone" };
  const word = words[error.status];
  if (word) return t(lang, word);
  if (error.status === 400 || error.status === 409) return t(lang, "serverRefused", { why: error.message });
  return t(lang, "serverDown");
}
