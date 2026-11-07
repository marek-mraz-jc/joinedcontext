import type { Row, Schema } from "@joinedcontext/sdk";

/** What the WebAssembly part answers for one attribute (wasm/src/lib.rs `summarize`). */
export interface Summary {
  count: number;
  min: number;
  max: number;
  mean: number;
  median: number;
}

/** `summarize`'s five numbers by name. */
export function toSummary(out: ArrayLike<number>): Summary {
  return { count: out[0] ?? 0, min: out[1] ?? NaN, max: out[2] ?? NaN, mean: out[3] ?? NaN, median: out[4] ?? NaN };
}

/** The numbers one attribute holds over the rows; a row without a number holds none. */
export function valuesOf(rows: Row[], attribute: string): Float64Array {
  const values: number[] = [];
  for (const row of rows) {
    const cell = row[attribute];
    if (typeof cell === "number" && Number.isFinite(cell)) values.push(cell);
  }
  return Float64Array.from(values);
}

/** The attributes of a type the model says are numbers, in its order. */
export function numericAttributes(schema: Schema, type: string): string[] {
  return Object.entries(schema[type]?.properties ?? {})
    .filter(([, field]) => {
      const kinds = Array.isArray(field.type) ? field.type : [field.type];
      return kinds.includes("number") || kinds.includes("integer");
    })
    .map(([name]) => name);
}

/** The worker's one question and answer, matched by `id`. */
export interface Ask {
  id: number;
  values: Float64Array;
}
export type Answer = { id: number; summary: number[] } | { id: number; error: string };

/** The side of a Worker the summary talks to; a `Worker` is one. */
export interface Port {
  postMessage(ask: Ask): void;
  addEventListener(type: "message", listener: (event: MessageEvent<Answer>) => void): void;
  removeEventListener(type: "message", listener: (event: MessageEvent<Answer>) => void): void;
}

/**
 * Asks the worker, which runs the WebAssembly module off the page's thread, so the screen never
 * waits on it. Each answer finds its own question by `id`.
 */
export function workerSummary(worker: Port) {
  let next = 0;
  return (values: Float64Array): Promise<Summary> =>
    new Promise((resolve, reject) => {
      const id = ++next;
      const onMessage = (event: MessageEvent<Answer>) => {
        if (event.data.id !== id) return;
        worker.removeEventListener("message", onMessage);
        if ("error" in event.data) reject(new Error(event.data.error));
        else resolve(toSummary(event.data.summary));
      };
      worker.addEventListener("message", onMessage);
      worker.postMessage({ id, values } satisfies Ask);
    });
}
