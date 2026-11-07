import { useCallback, useEffect, useRef, useState } from "react";
import { inputOf } from "./QuestionData";
import type { QuestionInput } from "./QuestionData";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api, unwrap } from "../../api/client";
import type { components } from "../../api/schema";

/** The states of Architecture/19 §5, in the order a run walks them (AG-43). */
export const RUN_STATES = [
  "queued",
  "starting",
  "interviewing",
  "building",
  "testing",
  "previewing",
  "awaiting_approval",
  "published",
] as const;

/** A run that ended, whichever way. Nothing more arrives on its stream. */
export const TERMINAL_STATES = ["published", "failed", "cancelled", "expired"];

/**
 * The event kinds the stream carries (API/04 §4). Every frame is a named event, so the hook
 * subscribes to each kind by name: `onmessage` alone would see none of them.
 */
export const EVENT_KINDS = [
  "status",
  "question",
  "answer",
  "message",
  "thought",
  "partial",
  "tool",
  "commit",
  "preview",
  "tests",
  "usage",
  "navigate",
  "ui",
  "lag",
  "endpoints",
  "app-build",
] as const;

export interface AgentRun {
  id: string;
  project: string;
  /** `conversation` for the assistant, an application run otherwise (API/04). */
  kind?: string;
  appName: string;
  /** What the application is called on screen; the name is its id (absent from older runs). */
  title?: string;
  endpointName: string;
  /** The endpoint the preview's reads and writes go through (AP-63). */
  endpointSlug?: string;
  /** The confirmed data needs: which write operations the preview bridge lets through (SDK-18). */
  dataNeeds?: unknown;
  appClass: string;
  visibility: string;
  prompt: string;
  status: string;
  steps: number;
  tokensUsed: number;
  previewUrl?: string;
  /** Started without asking, ending waiting for approval with its preview built (AG-69). */
  unattended?: boolean;
  /** The `chg-…` id of the Change that publishes the application, after Publish (AP-71). */
  changeId?: string;
  /** The forge's web address of the application's source (AP-71). */
  sourceUrl?: string;
  /** The application's copy on GitHub, where the installation keeps one (AP-79). */
  mirrorUrl?: string;
  /** Milliseconds from admission to the first frame and to the first generated version (AG-66). */
  firstFrameMs?: number | null;
  firstVersionMs?: number | null;
  createdBy: string;
  createdAt: string;
  error?: string;
  ticket?: string;
}

export interface RunEvent {
  seq: number;
  kind: string;
  payload: Record<string, unknown>;
}

/** Why the version on screen is not published yet (SDK-38): its tests run or fail. */
export interface TestsHold {
  version: number;
  running: boolean;
  failed: number;
  /** Up to five failing tests, `file › name`. */
  names: string[];
}

/**
 * The newest `tests` result of the version the preview shows, when it holds publication back; the
 * Portal answers a publish of that version 409 with the same reason (API/04 §6). A version that was
 * skipped, could not be tested or passed is not held.
 */
export function testsHold(events: RunEvent[], previewUrl: string | undefined): TestsHold | null {
  const version = Number(new URLSearchParams(previewUrl?.split("?")[1] ?? "").get("v"));
  if (!Number.isInteger(version) || version < 1) {
    return null;
  }
  const result = [...events].reverse().find((event) => event.kind === "tests" && event.payload.version === version);
  const outcome = result?.payload.outcome;
  if (outcome !== "running" && outcome !== "failed") {
    return null;
  }
  const failures = Array.isArray(result?.payload.failures) ? result.payload.failures : [];
  const names = failures
    .slice(0, 5)
    .flatMap((failure: unknown) => {
      const { file, name } = (failure ?? {}) as { file?: unknown; name?: unknown };
      return typeof name === "string" ? [`${typeof file === "string" ? file : ""} › ${name}`] : [];
    });
  const failed = typeof result?.payload.failed === "number" ? result.payload.failed : names.length;
  return { version, running: outcome === "running", failed, names };
}

/** One question the agent is waiting on, as the `question` event carries it. */
export interface RunQuestion {
  questionId: string;
  schema: Record<string, unknown>;
  required?: boolean;
  /** The platform filled the options (AG-83): only they are an answer. */
  pick?: boolean;
  /** Options the person cannot take now, by value, each with why (UI-44, T-2694). */
  disabledReasons?: Record<string, string>;
  /** A file or a feed's address the question also takes (T-2694). */
  input?: QuestionInput;
}

export function questionOf(event: RunEvent): RunQuestion | null {
  const id = event.payload.questionId;
  const schema = event.payload.schema;
  if (typeof id !== "string" || typeof schema !== "object" || schema === null) {
    return null;
  }
  const pick = typeof event.payload.pick === "string" && event.payload.pick !== "";
  const disabledReasons: Record<string, string> = {};
  for (const option of Array.isArray(event.payload.options) ? event.payload.options : []) {
    const { value, disabledReason } = (option ?? {}) as { value?: unknown; disabledReason?: unknown };
    if (typeof value === "string" && typeof disabledReason === "string") {
      disabledReasons[value] = disabledReason;
    }
  }
  return {
    questionId: id,
    schema: schema as Record<string, unknown>,
    required: true,
    pick,
    disabledReasons,
    input: inputOf(event.payload.input),
  };
}

/**
 * The questions a person's later message withdrew (T-6104): the run takes a typed message as
 * the next turn and waits on no earlier question, so "cancel this" closes the form, and an agent
 * that still needs the answer asks again.
 */
export function withdrawnQuestions(events: RunEvent[]): Set<string> {
  const answered = new Set(
    events.filter((event) => event.kind === "answer").map((event) => String(event.payload.questionId ?? "")),
  );
  const withdrawn = new Set<string>();
  const asked: string[] = [];
  for (const event of events) {
    if (event.kind === "question") {
      asked.push(String(event.payload.questionId ?? ""));
    } else if (event.kind === "message" && event.payload.sentBy !== "agent") {
      for (const id of asked.splice(0)) {
        if (!answered.has(id)) {
          withdrawn.add(id);
        }
      }
    }
  }
  return withdrawn;
}

/** The questions nobody has answered or withdrawn yet, oldest first. */
export function openQuestions(events: RunEvent[]): RunQuestion[] {
  const closed = withdrawnQuestions(events);
  for (const event of events) {
    if (event.kind === "answer") {
      closed.add(String(event.payload.questionId ?? ""));
    }
  }
  return events
    .filter((event) => event.kind === "question")
    .map(questionOf)
    .filter((question): question is RunQuestion => question !== null)
    .filter((question) => !closed.has(question.questionId));
}

/**
 * One run, live: the record, its event stream, and the three things a person may do to it.
 *
 * The stream is the browser's own `EventSource`, which is what makes the resume free: it
 * reconnects on its own and sends `Last-Event-ID`, and the Portal replays from there (AG-45).
 * The record is re-read whenever a `status` event arrives rather than polled, so a run that
 * says nothing costs nothing.
 */
/** The run a page reopens: `?run=<id>` in the address, kept while the run page is open. */
export function runInUrl(): string | null {
  return new URLSearchParams(window.location.search).get("run");
}

export function setRunInUrl(runId: string | null): void {
  const url = new URL(window.location.href);
  if (runId === null) {
    url.searchParams.delete("run");
  } else {
    url.searchParams.set("run", runId);
  }
  window.history.replaceState(window.history.state, "", url);
}

const NO_EVENTS: RunEvent[] = [];

export function useAgentRun(project: string, runId: string | null) {
  // The events are the run's they came on: a switch to another run shows none of the last one's
  // and drops none of its own as a replay (F-UI2-011).
  const [stream, setStream] = useState<{ runId: string | null; events: RunEvent[] }>({ runId, events: [] });
  const events = stream.runId === runId ? stream.events : NO_EVENTS;
  const [streaming, setStreaming] = useState(false);
  const queryClient = useQueryClient();
  const key = ["agent-run", project, runId];
  const seen = useRef(new Set<number>());

  const run = useQuery({
    queryKey: key,
    enabled: runId !== null,
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/agent-runs/{id}", {
          params: { path: { project, id: runId ?? "" } },
        }),
      ) as unknown as AgentRun,
  });

  useEffect(() => {
    if (runId === null) {
      return;
    }
    seen.current = new Set<number>();
    // Resolved at call time: jsdom has no EventSource, and a test substitutes one.
    const Source = (globalThis as { EventSource?: typeof EventSource }).EventSource;
    if (!Source) {
      return;
    }
    const source = new Source(
      `/api/v1/projects/${encodeURIComponent(project)}/agent-runs/${encodeURIComponent(runId)}/events`,
      { withCredentials: true },
    );
    const onOpen = () => {
      setStreaming(true);
    };
    const onError = () => {
      setStreaming(false);
    };
    const handlers = EVENT_KINDS.map((kind) => {
      const handler = (message: MessageEvent<string>) => {
        let payload: Record<string, unknown> = {};
        try {
          payload = JSON.parse(message.data) as Record<string, unknown>;
        } catch {
          // A frame the Portal did not write. Dropping it beats rendering half of it.
          return;
        }
        const seq = Number(payload.seq ?? message.lastEventId ?? 0);
        // The replay a reconnect brings overlaps what the stream already showed.
        if (seen.current.has(seq)) {
          return;
        }
        seen.current.add(seq);
        setStream((current) => ({
          runId,
          events: [...(current.runId === runId ? current.events : []), { seq, kind, payload }].sort((a, b) => a.seq - b.seq),
        }));
        if (kind === "status" || kind === "preview" || kind === "usage") {
          void queryClient.invalidateQueries({ queryKey: key });
        }
      };
      source.addEventListener(kind, handler as EventListener);
      return { kind, handler };
    });
    source.addEventListener("open", onOpen);
    source.addEventListener("error", onError);

    return () => {
      for (const { kind, handler } of handlers) {
        source.removeEventListener(kind, handler as EventListener);
      }
      source.removeEventListener("open", onOpen);
      source.removeEventListener("error", onError);
      source.close();
      setStreaming(false);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [project, runId]);

  const answer = useMutation({
    mutationFn: async (body: { questionId: string; answers: unknown }) =>
      unwrap(
        await api.POST("/api/v1/projects/{project}/agent-runs/{id}/answers", {
          params: { path: { project, id: runId ?? "" } },
          body: body,
        }),
      ),
  });

  const send = useMutation({
    // A message may change the endpoints a conversation queries (AG-75): they travel with it.
    mutationFn: async (
      message:
        | string
        | {
            text: string;
            endpointNames?: string[];
            pageContext?: { route: string };
            access?: components["schemas"]["Capabilities"];
            scope?: string;
          },
    ) =>
      unwrap(
        await api.POST("/api/v1/projects/{project}/agent-runs/{id}/messages", {
          params: { path: { project, id: runId ?? "" } },
          // `endpointNames` is AG-75; the generated body type catches up with the next API render.
          body: (typeof message === "string" ? { text: message } : message) as { text: string },
        }),
      ),
  });

  const cancel = useMutation({
    // A caller that moves on from the run in the same click names it: the mutation runs with the
    // newest render's `runId`, which by then is the next conversation's or none (T-2463).
    mutationFn: async (id?: string) =>
      unwrap(
        await api.POST("/api/v1/projects/{project}/agent-runs/{id}/cancel", {
          params: { path: { project, id: id ?? runId ?? "" } },
        }),
      ),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: key }),
  });

  const publish = useMutation({
    mutationFn: async () =>
      unwrap(
        await api.POST("/api/v1/projects/{project}/agent-runs/{id}/publish", {
          params: { path: { project, id: runId ?? "" } },
        }),
      ),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: key }),
  });

  const reset = useCallback(() => {
    seen.current = new Set<number>();
    setStream({ runId, events: [] });
  }, [runId]);

  return { run, events, streaming, answer, send, cancel, publish, reset };
}
