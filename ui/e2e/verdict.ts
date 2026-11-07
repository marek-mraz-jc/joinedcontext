import { digestOf } from "../src/api/digest";

/**
 * What a Check gets back: a green verdict fresh for the manifest it judged (PF-57, AG-62).
 * Strict validation is the default, so a form that names its draft proposes nothing until the
 * check is green — every spec that proposes runs the check first.
 */
export function greenVerdict(postData: string | null): unknown {
  const manifest = { ...(JSON.parse(postData ?? "{}") as Record<string, unknown>) };
  delete manifest.draft;
  return {
    verdict: {
      ok: true,
      findings: [],
      checkedAt: new Date().toISOString(),
      inputDigest: digestOf(manifest),
    },
  };
}

/** True for the dry run a Check sends, whatever collection it posts to. */
export function isCheck(method: string, url: string): boolean {
  // Any verb but GET: a form checks with `POST …?dryRun=All`, and Pause, Resume and Publish check
  // the manifest they are about to write with `PUT …?dryRun=All` (PF-57, T-2264). A stub that took
  // only the POST for a check counted the PUT as a write.
  return method !== "GET" && new URL(url).searchParams.has("dryRun");
}
