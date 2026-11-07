/**
 * The person's own copy of real data into a preview (PF-83, API/01 §22): read through the
 * origin's Endpoint, write through the preview's, both on the person's edge session, so each
 * Policy decides as it would for any other call. Bounded per type and in bytes.
 *
 * Every entity is written as it was read, ids and relationships included: an entity is its space
 * and its URN (ADR-N-041), so the copy in the preview's space is another entity under the same
 * URN, and a relationship between two copied entities still points within the preview.
 */

export const PER_TYPE = 1000;
export const MAX_BYTES = 10 * 1024 * 1024;
const PAGE = 100;

export interface CopyResult {
  /** Entities written, by type. */
  copied: Record<string, number>;
  /** Why the copy stopped early: a refusal, or the byte bound. */
  stopped?: string;
}

type Fetch = (input: string, init?: RequestInit) => Promise<Response>;

async function reason(response: Response): Promise<string> {
  const text = await response.text().catch(() => "");
  try {
    const problem = JSON.parse(text) as { detail?: string; title?: string };
    return problem.detail ?? problem.title ?? `HTTP ${response.status}`;
  } catch {
    return text || `HTTP ${response.status}`;
  }
}

export async function copyIntoPreview(
  originSlug: string,
  previewSlug: string,
  fetcher: Fetch = (input, init) => fetch(input, { credentials: "same-origin", ...init }),
): Promise<CopyResult> {
  const origin = `/api/endpoint/${originSlug}/ngsi-ld/v1`;
  const preview = `/api/endpoint/${previewSlug}/ngsi-ld/v1`;
  const json = { Accept: "application/json" };
  const result: CopyResult = { copied: {} };

  const types = await fetcher(`${origin}/types`, { headers: json });
  if (!types.ok) return { ...result, stopped: await reason(types) };
  const typeList = ((await types.json()) as { typeList?: string[] }).typeList ?? [];

  let bytes = 0;
  for (const type of typeList) {
    for (let offset = 0; offset < PER_TYPE; offset += PAGE) {
      const limit = Math.min(PAGE, PER_TYPE - offset);
      const query = new URLSearchParams({ type, limit: String(limit), offset: String(offset) });
      const page = await fetcher(`${origin}/entities?${query}`, { headers: json });
      if (!page.ok) return { ...result, stopped: await reason(page) };
      const entities = (await page.json()) as { id: string }[];
      if (entities.length === 0) break;
      const body = JSON.stringify(entities);
      bytes += body.length;
      if (bytes > MAX_BYTES) return { ...result, stopped: `the copy stops at ${MAX_BYTES / 1024 / 1024} MiB` };
      const written = await fetcher(`${preview}/entityOperations/upsert`, {
        method: "POST",
        // The copy is the person's write through the Portal, so it names why (DM-113).
        headers: { "Content-Type": "application/json", "JC-Reason": encodeURIComponent(`copied from ${originSlug} into its preview`) },
        body,
      });
      if (!written.ok) return { ...result, stopped: await reason(written) };
      if (written.status === 207) {
        const report = (await written.json().catch(() => ({}))) as { errors?: { error?: { detail?: string } }[] };
        const refused = report.errors ?? [];
        result.copied[type] = (result.copied[type] ?? 0) + entities.length - refused.length;
        if (refused.length > 0) return { ...result, stopped: refused[0].error?.detail ?? "some entities were refused" };
      } else {
        result.copied[type] = (result.copied[type] ?? 0) + entities.length;
      }
      if (entities.length < limit) break;
    }
  }
  return result;
}
