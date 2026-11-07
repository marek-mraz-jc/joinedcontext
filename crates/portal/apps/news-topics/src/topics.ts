import { useEffect, useMemo, useRef, useState } from "react";
import { format } from "@joinedcontext/sdk";
import type { Row } from "@joinedcontext/sdk";

export interface ArticleInput {
  id: string;
  title: string;
  summary?: string;
  published?: string;
}

export interface AnalysisInput {
  articles: ArticleInput[];
  k: number;
  seed: number;
}

export interface Keyword {
  term: string;
  weight: number;
}

export interface Topic {
  id: number;
  keywords: Keyword[];
  articles: string[];
  share: number;
}

export interface WeekShare {
  week: string;
  shares: number[];
}

export interface AnalysisOutput {
  topics: Topic[];
  weeks: WeekShare[];
  unassigned: string[];
}

export type TopicStatus = "loading" | "ready" | "error";

export interface TopicsHookResult {
  status: TopicStatus;
  result: AnalysisOutput | null;
  error: Error | null;
}

/** A cell as text; the SDK has already resolved a language map to the reader's language. */
function textOf(cell: Row[string]): string {
  return format(cell).trim();
}

/** Formats SDK rows into the typed input expected by the WASM topic clusterer; the module clamps k. */
export function toInput(rows: Row[], k = 5, seed = 1): AnalysisInput {
  const articles: ArticleInput[] = rows.map((row) => {
    const title = textOf(row.name) || row.id;
    const summary = textOf(row.description);
    const published = textOf(row.datePublished);
    const article: ArticleInput = {
      id: row.id,
      title,
    };
    if (summary) article.summary = summary;
    if (published) article.published = published;
    return article;
  });

  return { articles, k, seed };
}

/**
 * Runs topic clustering in a dedicated Web Worker, dropping stale requests
 * and managing loading and error states cleanly without crashing.
 */
export function useTopics(rows: Row[], k = 5, seed = 1): TopicsHookResult {
  const [status, setStatus] = useState<TopicStatus>("loading");
  const [result, setResult] = useState<AnalysisOutput | null>(null);
  const [error, setError] = useState<Error | null>(null);

  const workerRef = useRef<Worker | null>(null);
  const nextIdRef = useRef(1);
  const activeIdRef = useRef<number | null>(null);

  useEffect(() => {
    let worker: Worker;
    try {
      worker = new Worker(new URL("./worker.ts", import.meta.url), { type: "module" });
    } catch (err) {
      setStatus("error");
      setError(err instanceof Error ? err : new Error(String(err)));
      return;
    }

    workerRef.current = worker;

    worker.onmessage = (
      event: MessageEvent<{ id: number; output?: AnalysisOutput | { error: string }; error?: string }>,
    ) => {
      const msg = event.data;
      if (!msg || msg.id !== activeIdRef.current) {
        return;
      }

      if (msg.error) {
        setStatus("error");
        setError(new Error(msg.error));
        setResult(null);
      } else if (msg.output) {
        if ("error" in msg.output && typeof msg.output.error === "string") {
          setStatus("error");
          setError(new Error(msg.output.error));
          setResult(null);
        } else {
          setStatus("ready");
          setResult(msg.output as AnalysisOutput);
          setError(null);
        }
      }
    };

    worker.onerror = (err) => {
      setStatus("error");
      setError(new Error(err.message || "Worker error"));
    };

    return () => {
      worker.terminate();
      workerRef.current = null;
    };
  }, []);

  const input = useMemo(() => toInput(rows, k, seed), [rows, k, seed]);
  const inputJson = useMemo(() => JSON.stringify(input), [input]);

  useEffect(() => {
    if (rows.length === 0) {
      setStatus("ready");
      setResult({ topics: [], weeks: [], unassigned: [] });
      setError(null);
      activeIdRef.current = null;
      return;
    }

    const worker = workerRef.current;
    if (!worker) {
      return;
    }

    const id = nextIdRef.current++;
    activeIdRef.current = id;
    setStatus("loading");

    worker.postMessage({ id, input });
  }, [inputJson, rows.length]);

  return { status, result, error };
}

/** A topic's top keyword: what a follow and a steward's name hold on to as topics are fitted again. */
export const topTerm = (topic: Topic): string => topic.keywords[0]?.term ?? "";

/**
 * Topics a steward gave one name, as one (T-3828): the first topic's id kept, the shares summed,
 * keywords and articles joined. A topic without a name stays as it is.
 */
export function mergeNamed(result: AnalysisOutput, nameOf: (term: string) => string | undefined): AnalysisOutput {
  const groups = new Map<string, Topic[]>();
  for (const topic of result.topics) {
    const key = nameOf(topTerm(topic)) ?? `#${topic.id}`;
    groups.set(key, [...(groups.get(key) ?? []), topic]);
  }
  const topics = [...groups.values()].map(([first, ...rest]) =>
    rest.length === 0
      ? first
      : {
          id: first.id,
          keywords: [...first.keywords, ...rest.flatMap((topic) => topic.keywords)].filter(
            (keyword, index, all) => all.findIndex((other) => other.term === keyword.term) === index,
          ),
          articles: [...first.articles, ...rest.flatMap((topic) => topic.articles)],
          share: rest.reduce((sum, topic) => sum + topic.share, first.share),
        },
  );
  const weeks = result.weeks.map((week) => {
    const shares = [...week.shares];
    for (const [first, ...rest] of groups.values()) {
      if (rest.length === 0) continue;
      shares[first.id] = rest.reduce((sum, topic) => sum + (week.shares[topic.id] ?? 0), week.shares[first.id] ?? 0);
    }
    return { week: week.week, shares };
  });
  return { ...result, topics, weeks };
}
