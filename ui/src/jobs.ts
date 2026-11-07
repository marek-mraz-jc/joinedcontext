/**
 * The long-running actions this person started in this browser (T-3245): an App's build, a
 * knowledge source's crawl. The page that started one may be left; the Shell watches every job
 * here and says when it finishes, linking back. Kept in this browser only: a job is a
 * convenience of the person who pressed the button, and the platform keeps the truth (the run, the
 * crawl), so a job lost with the browser's storage loses nothing but the notice.
 */
import { useSyncExternalStore } from "react";

export type JobKind = "appBuild" | "knowledgeCrawl";
export type JobOutcome = "succeeded" | "failed" | "cancelled";

export interface Job {
  id: string;
  kind: JobKind;
  project: string;
  /** The App or the knowledge source. */
  name: string;
  /** For a build: the run number that was newest when the person pressed Rebuild. */
  after?: number;
  /** RFC 3339. */
  startedAt: string;
  finishedAt?: string;
  outcome?: JobOutcome;
  /** Whether the person has seen it finish. */
  seen?: boolean;
}

const KEY = "jc.jobs";
/** A finished job stays in the list a day; a running one a day too, so a lost one ends. */
const KEPT_MS = 24 * 60 * 60 * 1000;

let cache: Job[] | null = null;
const listeners = new Set<() => void>();

function read(): Job[] {
  if (cache) return cache;
  let stored: unknown = [];
  try {
    stored = JSON.parse(localStorage.getItem(KEY) ?? "[]");
  } catch {
    stored = [];
  }
  const now = Date.now();
  cache = (Array.isArray(stored) ? stored : [])
    .filter((job): job is Job => typeof job?.id === "string" && typeof job?.kind === "string")
    .filter((job) => now - Date.parse(job.finishedAt ?? job.startedAt) < KEPT_MS);
  return cache;
}

function write(jobs: Job[]): void {
  cache = jobs;
  try {
    localStorage.setItem(KEY, JSON.stringify(jobs));
  } catch {
    // A browser that keeps nothing still shows the jobs of this visit.
  }
  listeners.forEach((listener) => listener());
}

export function startJob(job: Omit<Job, "id" | "startedAt">): void {
  const id = `${job.kind}:${job.project}:${job.name}`;
  // One job per thing: starting it again replaces the one before.
  write([{ ...job, id, startedAt: new Date().toISOString() }, ...read().filter((other) => other.id !== id)]);
}

export function finishJob(id: string, outcome: JobOutcome): void {
  write(read().map((job) => (job.id === id && !job.outcome ? { ...job, outcome, finishedAt: new Date().toISOString() } : job)));
}

export function markJobsSeen(): void {
  if (read().some((job) => job.outcome && !job.seen)) {
    write(read().map((job) => (job.outcome ? { ...job, seen: true } : job)));
  }
}

export function dismissJob(id: string): void {
  write(read().filter((job) => job.id !== id));
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  // Another tab of the same Portal started or finished one.
  const fromOtherTab = (event: StorageEvent) => {
    if (event.key === KEY) {
      cache = null;
      listener();
    }
  };
  window.addEventListener("storage", fromOtherTab);
  return () => {
    listeners.delete(listener);
    window.removeEventListener("storage", fromOtherTab);
  };
}

export function useJobs(): Job[] {
  return useSyncExternalStore(subscribe, read, () => []);
}

/** Forgets the cache; for tests that change the storage under it. */
export function resetJobs(): void {
  cache = null;
}

/** How a running job stands against its estimate, in whole minutes: `left` once there is one. */
export function progressOf(startedAt: string, typicalSeconds: number | null | undefined, now = Date.now()) {
  const elapsed = Math.max(0, Math.round((now - Date.parse(startedAt)) / 1000));
  if (!typicalSeconds) return { elapsedMinutes: Math.floor(elapsed / 60), leftMinutes: undefined };
  const left = Math.max(0, typicalSeconds - elapsed);
  return { elapsedMinutes: Math.floor(elapsed / 60), leftMinutes: Math.ceil(left / 60) };
}
