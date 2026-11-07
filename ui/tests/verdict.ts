import { digestOf } from "../src/api/digest";

/** True for the dry run a Check sends, whatever collection it posts to. */
export function isCheck(request: Request, url: URL): boolean {
  return (
    (request.method === "POST" || request.method === "PUT") &&
    url.searchParams.get("dryRun") === "All"
  );
}

/**
 * What a Check gets back: a green verdict fresh for the manifest it judged (AG-62). Strict
 * validation is the default (PF-57), so a form whose proposal names its draft proposes nothing
 * until the check is green — every test that proposes runs the check first (T-0779).
 */
export async function greenVerdict(
  request: Request,
  extra: Record<string, unknown> = {},
): Promise<Record<string, unknown>> {
  const body = (await request.clone().json()) as Record<string, unknown>;
  const manifest = { ...body };
  delete manifest.draft;
  return {
    ...extra,
    verdict: {
      ok: true,
      findings: [],
      checkedAt: new Date().toISOString(),
      inputDigest: digestOf(manifest),
    },
  };
}
