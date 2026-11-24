// Hand-written types of the generated assistantRender.js (scripts/assistant-render.mjs, T-7790):
// the parts of the widget's render.js the Portal chat uses. render.test.js tests the code itself.

export type Inline =
  | string
  | { tag: "strong" | "em" | "code"; children: Inline[] }
  | { tag: "a"; href: string; children: Inline[] }
  | { tag: "cite"; numbers: number[] }
  | { tag: "br" };

export type Cell = { tag: "th" | "td"; children: Inline[] };

export type Block =
  | { tag: "p"; children: Inline[] }
  | { tag: "ul" | "ol"; children: { tag: "li"; children: Inline[] }[] }
  | { tag: "table"; children: { tag: "tr"; children: Cell[] }[] }
  | { tag: "pre"; children: [{ tag: "code"; children: string[] }] };

export interface Citation {
  n: number;
  url?: string;
  tool?: string;
  endpoint?: string;
  title?: string;
}

export interface Source {
  url: string | null;
  title: string | null;
  live: boolean;
  endpoint: string | null;
  domain: string;
  numbers: number[];
}

/** A failed answer: the widget's word for it (`limit`, `busy`, `unreachable`, …) and the service's own fields. */
export interface TurnError {
  word: string;
  status: number | null;
  title: string | null;
  detail: string | null;
}

/** One answer's state after its events so far (`reduce`). */
export interface TurnState {
  conversation: string | null;
  at: string | null;
  resumable: string | null;
  stages: Record<string, unknown>[];
  tools: { name: string | null; endpoint: string | null; status: "started" | "done" | "failed" }[];
  scripts: { code: string; output: string }[];
  data: Record<string, unknown>[];
  visuals: Record<string, unknown>[];
  reads: Record<string, unknown>[];
  text: string;
  answered: boolean;
  citations: Citation[] | null;
  timed: Record<string, number> | null;
  followups: string[];
  error: TurnError | null;
  done: boolean;
}

export interface Steps {
  summary: string;
  lines: string[];
  links?: (string | undefined)[];
}

declare const JCRender: {
  events(buffer: string): { events: { name: string; data: unknown }[]; rest: string };
  reduce(turn: TurnState | undefined, name: string, data: unknown, ms?: number): TurnState;
  markdown(text: string, known?: ((n: number) => boolean) | null, linkable?: (url: string) => boolean): Block[];
  streamable(text: string): string;
  sources(citations: Citation[] | null | undefined): { list: Source[]; position: Record<number, number> };
  label(source: Pick<Source, "url" | "title" | "endpoint">): string;
  linkHost(url: string, hosts: string[]): boolean;
  safeUrl(url: unknown): string | null;
  steps(stages: unknown[], lang: string, timed: Record<string, number> | null, reads: unknown[]): Steps | null;
  live(data: unknown, lang: string): string | null;
  csv(table: { columns: string[]; rows: unknown[][] } | null): string | null;
  csvName(caption: string | null | undefined): string;
  retryable(word: string): boolean;
  resetAt(seconds: unknown, lang: string, now?: number): string | null;
};
export default JCRender;
