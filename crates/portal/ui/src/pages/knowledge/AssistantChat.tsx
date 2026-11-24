import { reasonOf } from "../../api/client";
import { useEffect, useId, useMemo, useRef, useState } from "react";
import type { FormEvent, JSX, KeyboardEvent, ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { CodeBlock, Alert, Button, Checkbox, ExternalLink, Field, Table, Textarea } from "../../components/ui";
import { useBranding } from "../../branding";
import { RecordsGrid } from "../../components/grid/RecordsGrid";
import { askAssistant, cellText, chartSpec, dataChart, dataTable, embedSnippet, launcherSnippet, visualOf } from "./knowledge";
import type { ChatTurn, DataTable, Spec } from "./knowledge";
import { VegaChart, chartTheme } from "../../components/panels/VegaChart";
import { narrowConfig } from "../../components/panels/palette";
import JCRender from "../../generated/assistantRender";
import type { Block, Citation, Inline, TurnError, TurnState } from "../../generated/assistantRender";

type T = (key: string, options?: Record<string, unknown>) => string;

/**
 * What the panel shows of one turn: the person's question, or the assistant's answer as the
 * shared reducer reads its events (T-7790), the same state the public widget shows.
 */
type Shown =
  | { role: "user"; text: string }
  | { role: "assistant"; state: TurnState; started: number; stopped?: boolean; retryAfter?: string | null };

function inlineNodes(nodes: Inline[], ids: string, position: Record<number, number>, t: T): ReactNode[] {
  return nodes.map((node, i) => {
    if (typeof node === "string") return node;
    switch (node.tag) {
      case "br":
        return <br key={i} />;
      case "strong":
        return <strong key={i}>{inlineNodes(node.children, ids, position, t)}</strong>;
      case "em":
        return <em key={i}>{inlineNodes(node.children, ids, position, t)}</em>;
      case "code":
        return (
          <code key={i} className="rounded bg-surface-subtle px-1 font-mono text-caption">
            {inlineNodes(node.children, ids, position, t)}
          </code>
        );
      case "a":
        return (
          <ExternalLink key={i} href={node.href}>
            {inlineNodes(node.children, ids, position, t)}
          </ExternalLink>
        );
      case "cite": {
        const places = [...new Set(node.numbers.map((n) => position[n]).filter(Boolean))];
        return (
          <sup key={i} className="ml-0.5">
            {places.map((place, j) => (
              <span key={place}>
                {j > 0 ? "," : null}
                <a href={`#${ids}-${place}`} aria-label={t("knowledge.chat.sourceNumber", { n: place })} className="text-primary-soft-fg underline-offset-2 hover:underline">
                  {place}
                </a>
              </span>
            ))}
          </sup>
        );
      }
    }
  });
}

function Blocks({ blocks, ids, position }: { blocks: Block[]; ids: string; position: Record<number, number> }): JSX.Element {
  const { t } = useTranslation();
  return (
    <div className="space-y-2" data-testid="assistant-answer">
      {blocks.map((block, i) =>
        block.tag === "p" ? (
          <p key={i}>{inlineNodes(block.children, ids, position, t)}</p>
        ) : block.tag === "pre" ? (
          <CodeBlock label={t("codeBlock.code")} key={i} className="overflow-x-auto rounded bg-surface-subtle p-3 font-mono text-caption">
            <code>{block.children[0].children.join("")}</code>
          </CodeBlock>
        ) : block.tag === "table" ? (
          <Table key={i} caption={t("knowledge.chat.data")} className="text-caption">
            <tbody>
              {block.children.map((row, j) => (
                <tr key={j}>
                  {row.children.map((cell, k) =>
                    cell.tag === "th" ? (
                      <th key={k} scope="col" className="px-2 py-1 text-left">
                        {inlineNodes(cell.children, ids, position, t)}
                      </th>
                    ) : (
                      <td key={k} className="border-t border-border px-2 py-1 align-top">
                        {inlineNodes(cell.children, ids, position, t)}
                      </td>
                    ),
                  )}
                </tr>
              ))}
            </tbody>
          </Table>
        ) : (
          <block.tag key={i} className={block.tag === "ul" ? "list-disc space-y-1 pl-5" : "list-decimal space-y-1 pl-5"}>
            {block.children.map((item, j) => (
              <li key={j}>{inlineNodes(item.children, ids, position, t)}</li>
            ))}
          </block.tag>
        ),
      )}
    </div>
  );
}

/**
 * An answer formatted by the widget's render core (T-3325, T-7790): while it streams, the text so
 * far cut where a marker is still open; once it arrives, the whole text, its markers linked to the
 * sources once they come. Links stay links only to the deployment's hosts (`linkHostsOf`), this
 * Portal's and the ones the answer cites, as in the widget (T-4149); any other is its words.
 */
function Answer({ text, final, citations, ids, linkHosts }: { text: string; final: boolean; citations: Citation[]; ids: string; linkHosts: string[] }): JSX.Element {
  const grouped = JCRender.sources(citations);
  const hosts = [window.location.hostname, ...linkHosts, ...grouped.list.map((source) => source.domain)];
  const blocks = final
    ? JCRender.markdown(text, (n) => grouped.position[n] !== undefined, (url) => JCRender.linkHost(url, hosts))
    : JCRender.markdown(JCRender.streamable(text), null, (url) => JCRender.linkHost(url, hosts));
  return <Blocks blocks={blocks} ids={ids} position={final ? grouped.position : {}} />;
}

/** The sources under an answer, each marker's target, one per address (T-3325). */
function Sources({ citations, ids, label: name }: { citations: Citation[]; ids: string; label: string }): JSX.Element | null {
  const { t } = useTranslation();
  const grouped = JCRender.sources(citations);
  if (grouped.list.length === 0) return null;
  return (
    <ol aria-label={name} className="mt-2 list-decimal border-t border-border pl-6 pt-2 text-caption">
      {grouped.list.map((source, i) => {
        const text = (source.live ? `${t("knowledge.chat.liveData")}: ` : "") + JCRender.label(source);
        // Only an http(s) address a person can open is a link; an uploaded file is its name (T-4869).
        const href = JCRender.safeUrl(source.url);
        return (
          <li key={i} id={`${ids}-${i + 1}`}>
            {href ? <ExternalLink href={href}>{text}</ExternalLink> : text}
            {source.domain && !source.live ? <span className="ml-1 text-fg-muted">{source.domain}</span> : null}
          </li>
        );
      })}
    </ol>
  );
}

/**
 * A data round's chart (T-4198, T-6889), drawn by Vega with the chart theme (T-6886): the table's
 * one numeric column by the widget's rule, or the assistant's checked `visualization`. The table
 * above it holds the same values as text; when Vega cannot draw, a line says so and the table stays.
 */
function Chart({ spec, label }: { spec: Spec; label: string }): JSX.Element {
  const { t } = useTranslation();
  const drawn = useMemo(
    () => ({
      width: "container",
      autosize: { type: "fit", contains: "padding" },
      ...spec,
      config: { ...chartTheme().config, ...narrowConfig(), background: null },
    }),
    [spec],
  );
  const height = typeof spec.height === "number" ? spec.height : 180;
  return (
    <div className="mt-2" data-testid="assistant-chart">
      <VegaChart spec={drawn} label={label} height={height} failure={t("knowledge.chat.chartUnavailable")} />
    </div>
  );
}

/** The rows as a CSV file the browser writes (T-7790): the shared core's CSV, never HTML. */
function downloadCsv(table: DataTable): void {
  const text = JCRender.csv({ columns: table.columns, rows: table.rows });
  if (text === null) return;
  const url = URL.createObjectURL(new Blob([text], { type: "text/csv;charset=utf-8" }));
  const link = document.createElement("a");
  link.href = url;
  link.download = JCRender.csvName(table.caption);
  link.click();
  URL.revokeObjectURL(url);
}

/** A data round's rows as the widget shows them (T-4140): a table, its chart, its links and its CSV. */
function Data({ table, charted }: { table: DataTable; charted: boolean }): JSX.Element {
  const { t } = useTranslation();
  const chart = charted ? null : dataChart(table);
  const spec = useMemo(() => (chart ? chartSpec(chart) : null), [chart]);
  return (
    <figure className="mt-2 text-caption">
      <RecordsGrid
        caption={table.caption || t("knowledge.chat.data")}
        fields={table.columns.map((_, at) => String(at))}
        label={(field) => table.columns[Number(field)] ?? field}
        records={table.rows}
        text={(row, field) => cellText(row[Number(field)])}
        storageKey={`knowledge.chat.data.${table.columns.join(",")}`}
        empty={t("knowledge.chat.noRows")}
      />
      {chart && spec ? <Chart spec={spec} label={t("knowledge.chat.chart", { value: chart.value, by: chart.by })} /> : null}
      <p className="mt-1 flex flex-wrap items-center gap-3">
        {table.rows.length > 0 ? (
          <Button size="sm" onClick={() => downloadCsv(table)}>
            {t("knowledge.chat.csv")}
          </Button>
        ) : null}
        {table.dataset ? <ExternalLink href={table.dataset}>{t("knowledge.chat.dataset")}</ExternalLink> : null}
        {table.download ? <ExternalLink href={table.download}>{t("knowledge.chat.download")}</ExternalLink> : null}
      </p>
    </figure>
  );
}

/**
 * A failed answer in the reader's language (T-7790): the service's words are for its logs and a
 * Slovak visitor, so the Portal says each kind of failure itself, a limit with when it resets.
 */
export function troubleText(error: TurnError, retryAfter: string | null | undefined, lang: string, t: T): string {
  const reset = JCRender.resetAt(retryAfter, lang);
  if (reset && (error.word === "limit" || error.word === "busy")) return t(`knowledge.chat.trouble.${error.word}At`, { time: reset });
  const known = ["full", "unreachable", "unanswered", "slow", "declined", "limit", "busy", "paused", "offline"];
  return t(`knowledge.chat.trouble.${known.includes(error.word) ? error.word : "failed"}`);
}

/** The live line of an answer still under way, and the steps it already took. */
function Progress({ state, lang }: { state: TurnState; lang: string }): JSX.Element {
  const { t } = useTranslation();
  const lines = state.stages.map((stage) => JCRender.live(stage, lang)).filter((line): line is string => Boolean(line));
  const distinct = lines.filter((line, i) => lines.indexOf(line) === i);
  const reading = [...state.tools].reverse().find((tool) => tool.status === "started");
  const now = distinct[distinct.length - 1] ?? (reading ? t("knowledge.chat.reading", { name: reading.endpoint ?? reading.name ?? "" }) : t("knowledge.chat.thinking"));
  return (
    <div className="text-fg-muted">
      <p role="status" className="flex items-center gap-2">
        <span aria-hidden="true" className="inline-block size-2 animate-pulse rounded-full bg-primary" />
        {now}
      </p>
      {distinct.length > 1 ? (
        <ol className="mt-1 space-y-0.5 pl-4 text-caption">
          {distinct.slice(0, -1).map((line) => (
            <li key={line}>✓ {line.replace(/…$/, "")}</li>
          ))}
        </ol>
      ) : null}
    </div>
  );
}

/** One assistant turn: progress, the answer streamed then whole, its data, steps, sources and follow-ups. */
function AssistantTurn({
  turn,
  ids,
  lang,
  last,
  linkHosts,
  onAsk,
}: {
  turn: Extract<Shown, { role: "assistant" }>;
  ids: string;
  lang: string;
  last: boolean;
  linkHosts: string[];
  onAsk: (q: string) => void;
}): JSX.Element {
  const { t } = useTranslation();
  const { state } = turn;
  const citations = state.citations ?? [];
  const tables = state.data.map((raw) => dataTable(raw)).filter((table): table is DataTable => table !== null);
  const visual = state.visuals.length > 0 ? visualOf(state.visuals[state.visuals.length - 1]) : undefined;
  const steps = state.answered || state.done ? JCRender.steps(state.stages, lang, state.timed, state.reads) : null;
  const failed = state.tools.filter((tool) => tool.status === "failed");
  const working = !state.text && !state.error && !state.done && !turn.stopped;
  return (
    <>
      {working ? <Progress state={state} lang={lang} /> : null}
      {tables.map((table, n) => (
        <Data key={n} table={table} charted={!!visual} />
      ))}
      {visual ? <Chart spec={visual.spec} label={t("knowledge.chat.visual")} /> : null}
      {state.text ? <Answer text={state.text} final={state.answered} citations={citations} ids={ids} linkHosts={linkHosts} /> : null}
      {turn.stopped ? <p className="mt-1 text-caption text-fg-muted">{t("knowledge.chat.stopped")}</p> : null}
      {state.error ? (
        <>
          <Alert role="alert" tone="danger">
            {troubleText(state.error, turn.retryAfter, lang, t)}
          </Alert>
          {!state.text ? <Sources citations={citations} ids={ids} label={t("knowledge.chat.closest")} /> : null}
        </>
      ) : state.text ? (
        <Sources citations={citations} ids={ids} label={t("knowledge.chat.sources")} />
      ) : null}
      {failed.length > 0 ? (
        <p className="text-caption text-fg-muted">{t("knowledge.chat.toolFailed", { names: failed.map((tool) => tool.endpoint ?? tool.name).join(", ") })}</p>
      ) : null}
      {steps ? (
        <details className="mt-2 text-caption">
          <summary className="cursor-pointer">{steps.summary}</summary>
          <ol className="mt-1 list-decimal pl-5">
            {steps.lines.map((line, n) => (
              <li key={n}>{line}</li>
            ))}
          </ol>
        </details>
      ) : null}
      {state.scripts.map((script, n) => (
        <details key={n} className="mt-2 text-caption">
          <summary className="cursor-pointer">{t("knowledge.chat.script")}</summary>
          <CodeBlock label={t("knowledge.chat.script")} className="mt-1 overflow-x-auto rounded bg-surface-subtle p-2">{script.code}</CodeBlock>
          <CodeBlock label={t("codeBlock.result")} className="mt-1 overflow-x-auto rounded bg-surface-subtle p-2">{script.output}</CodeBlock>
        </details>
      ))}
      {last && state.followups.length > 0 ? (
        <div className="mt-2">
          <p className="text-caption text-fg-muted">{t("knowledge.chat.followups")}</p>
          <ul className="mt-1 flex flex-wrap gap-2">
            {state.followups.map((question) => (
              <li key={question}>
                <Button size="sm" variant="secondary" onClick={() => onAsk(question)}>
                  {question}
                </Button>
              </li>
            ))}
          </ul>
        </div>
      ) : null}
    </>
  );
}

/** The turns the next question sends back, as API/05 §1.1 bounds them. */
const HISTORY = 6;
const MAX_CHARS = 4_000;

/**
 * The knowledge assistant as the signed-in person asks it (API/05 §1.7, AG-115): the same events
 * the public widget shows, read by the same reducer (T-7790), the answer streamed, the person's own
 * connectors switchable. Stop ends the request, and the Portal's stream with it. Nothing is kept
 * past the dialog: the conversation lives in this panel, as in the widget.
 */
export function AssistantChat({
  project,
  deployment,
  connectors,
  linkHosts = [],
}: {
  project: string;
  deployment: string;
  connectors: string[];
  /** The hosts its links may go to beside the cited ones (`linkHostsOf`). */
  linkHosts?: string[];
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const lang = (i18n.resolvedLanguage ?? i18n.language ?? "en").slice(0, 2);
  const ids = useId();
  const [turns, setTurns] = useState<Shown[]>([]);
  const [question, setQuestion] = useState("");
  const [on, setOn] = useState<string[]>(connectors);
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  const [last, setLast] = useState<string | null>(null);
  const conversation = useRef<string | undefined>(undefined);
  const abort = useRef<AbortController | null>(null);
  const input = useRef<HTMLTextAreaElement | null>(null);
  const log = useRef<HTMLOListElement | null>(null);

  useEffect(() => () => abort.current?.abort(), []);
  // The log follows the newest turn while it grows.
  useEffect(() => {
    const box = log.current;
    if (box) box.scrollTop = box.scrollHeight;
  }, [turns]);

  function update(change: (turn: Extract<Shown, { role: "assistant" }>) => Extract<Shown, { role: "assistant" }>): void {
    setTurns((all) => {
      const at = all[all.length - 1];
      return at && at.role === "assistant" ? [...all.slice(0, -1), change(at)] : all;
    });
  }

  async function send(event?: FormEvent, again?: string): Promise<void> {
    event?.preventDefault();
    const message = (again ?? question).trim();
    if (!message || busy) return;
    const history: ChatTurn[] = turns
      .map((turn, i, all) => {
        if (turn.role === "assistant") return turn.state.text && !turn.state.error && !turn.stopped ? { role: "assistant" as const, text: turn.state.text } : null;
        // A question nothing answered is not history: asked again, it would be there twice.
        const answer = all[i + 1];
        return answer && answer.role === "assistant" && answer.state.text && !answer.state.error ? { role: "user" as const, text: turn.text } : null;
      })
      .filter((turn): turn is ChatTurn => turn !== null)
      .slice(-HISTORY)
      .map((turn) => ({ role: turn.role, text: turn.text.slice(0, MAX_CHARS) }));
    const started = Date.now();
    setTurns((all) => [...all, { role: "user", text: message }, { role: "assistant", state: JCRender.reduce(undefined, "", null), started }]);
    setQuestion("");
    setFailure(null);
    setLast(null);
    // The question comes back into the box when no answer does, to resend or reword (T-4160, finding 122).
    const missed = (again = true) => {
      setQuestion((now) => (now === "" ? message : now));
      if (again) setLast(message);
    };
    setBusy(true);
    const controller = new AbortController();
    abort.current = controller;
    try {
      await askAssistant(
        project,
        deployment,
        {
          conversation: conversation.current,
          message,
          history,
          connectors: connectors.length > 0 ? on : undefined,
        },
        (name, data) => {
          // A limit, a full conversation or a declined question offers no Try again: nothing
          // changes before it resets (T-4671).
          if (name === "error") missed(JCRender.retryable(JCRender.reduce(undefined, name, data).error?.word ?? "failed"));
          update((turn) => {
            const state = JCRender.reduce(turn.state, name, data, Date.now() - turn.started);
            if (state.conversation) conversation.current = state.conversation;
            return { ...turn, state };
          });
        },
        controller.signal,
        (retryAfter) => update((turn) => ({ ...turn, retryAfter })),
      );
    } catch (error) {
      if (controller.signal.aborted) {
        // What arrived stays; the person sees that they stopped it.
        update((turn) => ({ ...turn, stopped: true }));
        return;
      }
      setTurns((all) => all.slice(0, -1));
      missed();
      setFailure(reasonOf(error, t("knowledge.chat.failed")));
    } finally {
      setBusy(false);
      input.current?.focus();
    }
  }

  function onKey(event: KeyboardEvent<HTMLTextAreaElement>): void {
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      void send();
    }
  }

  return (
    <div className="flex h-[min(70vh,48rem)] min-h-80 flex-col gap-3">
      {connectors.length > 0 ? (
        <fieldset className="flex flex-wrap gap-x-4 gap-y-1">
          <legend className="mb-1 text-caption text-fg-muted">{t("knowledge.chat.connectors")}</legend>
          {connectors.map((name) => (
            <Checkbox
              key={name}
              label={name}
              checked={on.includes(name)}
              onChange={(change) =>
                setOn((current) =>
                  change.target.checked ? [...current, name] : current.filter((other) => other !== name),
                )
              }
            />
          ))}
        </fieldset>
      ) : null}
      <ol ref={log} aria-live="polite" aria-label={t("knowledge.chat.conversation")} className="min-h-0 flex-1 space-y-3 overflow-y-auto">
        {turns.length === 0 ? <li className="text-fg-muted">{t("knowledge.chat.empty")}</li> : null}
        {turns.map((turn, index) => (
          <li
            key={index}
            className={turn.role === "user" ? "ml-8 rounded-md bg-surface-subtle p-3" : "mr-8 rounded-md border border-border p-3"}
          >
            <span className="sr-only">{turn.role === "user" ? t("knowledge.chat.you") : t("knowledge.chat.assistant")}: </span>
            {turn.role === "user" ? (
              <p className="whitespace-pre-wrap">{turn.text}</p>
            ) : (
              <AssistantTurn turn={turn} ids={`${ids}-${index}`} lang={lang} last={index === turns.length - 1 && !busy} linkHosts={linkHosts} onAsk={(q) => void send(undefined, q)} />
            )}
          </li>
        ))}
      </ol>
      {failure ? (
        <Alert role="alert" tone="danger">
          {failure}
        </Alert>
      ) : null}
      {last && !busy ? (
        <Button size="sm" onClick={() => void send(undefined, last)}>
          {t("knowledge.chat.again")}
        </Button>
      ) : null}
      <form onSubmit={(event) => void send(event)} className="space-y-2">
        <Field id={`knowledge-chat-${deployment}`} label={t("knowledge.chat.question")} help={t("knowledge.chat.questionHint")}>
          <Textarea
            id={`knowledge-chat-${deployment}`}
            ref={input}
            rows={2}
            maxLength={MAX_CHARS}
            value={question}
            onChange={(change) => setQuestion(change.target.value)}
            onKeyDown={onKey}
          />
        </Field>
        <div className="flex gap-2">
          {busy ? (
            <Button type="button" variant="secondary" onClick={() => abort.current?.abort()}>
              {t("knowledge.chat.stop")}
            </Button>
          ) : (
            <Button type="submit" disabled={question.trim() === ""} disabledReason={t("knowledge.askFirst")}>
              {t("knowledge.chat.send")}
            </Button>
          )}
        </div>
      </form>
    </div>
  );
}

/** One snippet to paste, with the button that copies exactly it. */
function Snippet({ label, code }: { label: string; code: string }): JSX.Element {
  const { t } = useTranslation();
  const [copied, setCopied] = useState(false);
  return (
    <div className="space-y-2">
      <CodeBlock label={label} className="overflow-x-auto whitespace-pre-wrap break-all rounded bg-surface-subtle p-3 text-caption">
        {code}
      </CodeBlock>
      <Button
        size="sm"
        aria-label={t("knowledge.embed.copyOf", { what: label })}
        onClick={() => {
          void navigator.clipboard
            ?.writeText(code)
            .then(() => setCopied(true))
            .catch(() => setCopied(false));
        }}
      >
        {copied ? t("knowledge.embed.copied") : t("knowledge.embed.copy")}
      </Button>
    </div>
  );
}

/**
 * What a city site pastes to place a public assistant (T-4138, T-4196): the one-line launcher,
 * shown live in a sandboxed frame, and the plain frame for a page that takes no script. Both
 * name only the public deployment; the site must be one of its allowed origins.
 */
export function EmbedSnippet({ publicId, title, language }: { publicId: string; title: string; language?: string }): JSX.Element {
  const { t } = useTranslation();
  const branding = useBranding();
  if (!branding.domain) {
    return <p>{t("knowledge.embed.noDomain")}</p>;
  }
  const launcher = launcherSnippet(branding.domain, publicId, language);
  // A blank page holding the launcher alone, served by the Portal under its own policy: a
  // srcdoc frame would inherit the Portal's `script-src 'self'` and refuse the launcher (T-5328).
  // Scripts run, but the frame has no origin of its own, so nothing in it reaches the Portal.
  const preview = `/assistant-preview/${encodeURIComponent(publicId)}${language ? `?lang=${encodeURIComponent(language)}` : ""}`;
  return (
    <div className="space-y-4">
      <p>{t("knowledge.embed.launcherIntro")}</p>
      <Snippet label={t("knowledge.embed.launcher")} code={launcher} />
      <iframe
        title={t("knowledge.embed.preview")}
        sandbox="allow-scripts"
        referrerPolicy="no-referrer"
        src={preview}
        className="h-80 w-full rounded border border-border bg-surface"
      />
      <p className="text-caption text-fg-muted">{t("knowledge.embed.previewHint")}</p>
      {/* The deployment's chat as a page of its own, to link to or share: jc-assistant's `/d/{publicId}` (T-4283). */}
      <p>
        {t("knowledge.embed.fullPageHint")}{" "}
        <a href={`https://assistant.${branding.domain}/d/${encodeURIComponent(publicId)}`} target="_blank" rel="noopener noreferrer" className="break-all text-primary-soft-fg underline">
          {t("knowledge.embed.fullPage")}
        </a>
      </p>
      <p>{t("knowledge.embed.intro")}</p>
      <Snippet label={t("knowledge.embed.snippet")} code={embedSnippet(branding.domain, publicId, title)} />
      <p className="text-caption text-fg-muted">{t("knowledge.embed.origins")}</p>
    </div>
  );
}
