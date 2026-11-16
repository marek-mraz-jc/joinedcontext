import { ApiError } from "./client";
import { doorAction } from "./odata";

/**
 * One run of a candidate pipeline over a sample (PL-43, API/01 §7a), through the typed client
 * (UI-07): the route is the API's own and the csrf and session handling are the client's.
 *
 * The document publishes the trace as an object (it is jcctl's `TestTrace`, which carries no
 * schema), so its shape is written here once, beside the one call that reads it.
 */

export type SampleFormat = "csv" | "json" | "text";

export type TestSample = { url: string; format: SampleFormat } | { text: string; format: SampleFormat };

export interface Trace {
  input: { events: number; bytes: number; sample?: unknown };
  mapping: unknown[];
  validation: { index: number; ok: boolean; problems: string[] }[];
  errors: {
    stage: string;
    /** The step of `spec.steps` a `mapping` error failed at (PL-52); absent on lint and runner. */
    step?: number | null;
    line?: number | null;
    message: string;
  }[];
  /**
   * Every step of `spec.steps` as the test saw it (PL-67, API/01 §7a): `reached` messages arrived
   * whole, `sample` is what the step made of the first, cut at 4 KiB, as data or as text. Absent
   * from a runner harness older than jcctl's PL-67; the Studio then shows what it showed before.
   */
  stages?: { step: number; reached: number; sample?: unknown }[];
}

/** The trace, or the status and the reason the route refused the run. */
export type TestAnswer = { trace: Trace } | { status: number; detail?: string };

export async function testPipeline(
  project: string,
  pipeline: unknown,
  sample: TestSample,
): Promise<TestAnswer> {
  const inlined = await inlineEndpointSample(sample);
  if (!("sample" in inlined)) {
    return inlined;
  }
  try {
    return {
      trace: (await doorAction("portal_pipeline_test", { body: { pipeline, sample: inlined.sample } }, project)) as Trace,
    };
  } catch (error) {
    if (!(error instanceof ApiError)) throw error;
    const detail = error.problem?.detail;
    return { status: error.status, detail: typeof detail === "string" ? detail : undefined };
  }
}

/**
 * A sample whose URL is a page of one of this platform's endpoints, read here with the person's
 * own session and sent inline (T-3088). The runner fetches a URL with no credential, and every
 * endpoint wants one, so a test of an endpoint-sourced pipeline answered 401; read by the
 * browser, the sample is also exactly what this person may read, never more. Any other URL is
 * the runner's to fetch, as before. A page the endpoint refuses is the refusal, in words.
 */
export async function inlineEndpointSample(
  sample: TestSample,
): Promise<{ sample: TestSample } | { status: number; detail: string }> {
  if (!("url" in sample) || typeof window === "undefined") {
    return { sample };
  }
  let url: URL;
  try {
    url = new URL(sample.url);
  } catch {
    return { sample };
  }
  if (url.origin !== window.location.origin || !url.pathname.startsWith("/api/endpoint/")) {
    return { sample };
  }
  const response = await globalThis.fetch(
    new Request(url.toString(), { headers: { Accept: "application/ld+json" } }),
  );
  const text = await response.text();
  if (!response.ok) {
    let detail = `${response.status} ${response.statusText}`.trim();
    try {
      const problem = JSON.parse(text) as { detail?: unknown; title?: unknown };
      detail = String(problem.detail ?? problem.title ?? detail);
    } catch {
      // Not a problem document: the status line says it.
    }
    return { status: response.status, detail };
  }
  return { sample: { text, format: sample.format } };
}
