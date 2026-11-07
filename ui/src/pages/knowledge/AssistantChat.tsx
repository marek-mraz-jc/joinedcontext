import { reasonOf } from "../../api/client";
import { useEffect, useId, useRef, useState } from "react";
import type { FormEvent, JSX, KeyboardEvent, ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { CodeBlock, Alert, Button, Checkbox, ExternalLink, Field, Table, Textarea } from "../../components/ui";
import { useBranding } from "../../branding";
import { askAssistant, cellText, dataChart, embedSnippet, launcherSnippet } from "./knowledge";
import type { ChatEvent, ChatTurn, Citation, DataChart, DataTable } from "./knowledge";
import { label, markdown, sources } from "./answerFormat";
import type { Block, Inline } from "./answerFormat";

/** What the panel shows of one turn: the person's question or the assistant's answer as it grows. */
interface Shown {
  role: "user" | "assistant";
  text: string;
  reading?: string;
  tools: { tool: string; endpoint?: string; status: string }[];
  scripts: { code: string; result: string }[];
  citations: Citation[];
  data: DataTable[];
  error?: string;
}

function inlineNodes(nodes: Inline[], ids: string, position: Record<number, number>, t: (key: string, options?: Record<string, unknown>) => string): ReactNode[] {
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

/** An answer formatted, its markers linked to the sources listed under it (T-3325, AG-117). */
function Answer({ text, citations, ids }: { text: string; citations: Citation[]; ids: string }): JSX.Element {
  const { t } = useTranslation();
  const grouped = sources(citations);
  const blocks: Block[] = markdown(text, (n) => grouped.position[n] !== undefined);
  return (
    <>
      <div className="space-y-2" data-testid="assistant-answer">
        {blocks.map((block, i) =>
          block.tag === "p" ? (
            <p key={i}>{inlineNodes(block.children, ids, grouped.position, t)}</p>
          ) : block.tag === "pre" ? (
            <CodeBlock label={t("codeBlock.code")} key={i} className="overflow-x-auto rounded bg-surface-subtle p-3 font-mono text-caption">
              <code>{block.text}</code>
            </CodeBlock>
          ) : block.tag === "table" ? (
            <Table key={i} caption={t("knowledge.chat.data")} className="text-caption">
              <tbody>
                {block.children.map((row, j) => (
                  <tr key={j}>
                    {row.children.map((cell, k) =>
                      cell.tag === "th" ? (
                        <th key={k} scope="col" className="px-2 py-1 text-left">
                          {inlineNodes(cell.children, ids, grouped.position, t)}
                        </th>
                      ) : (
                        <td key={k} className="border-t border-border px-2 py-1 align-top">
                          {inlineNodes(cell.children, ids, grouped.position, t)}
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
                <li key={j}>{inlineNodes(item.children, ids, grouped.position, t)}</li>
              ))}
            </block.tag>
          ),
        )}
      </div>
      <Sources citations={citations} ids={ids} label={t("knowledge.chat.sources")} />
    </>
  );
}

/** The sources under an answer, each marker's target, one per address (T-3325). */
function Sources({ citations, ids, label: name }: { citations: Citation[]; ids: string; label: string }): JSX.Element | null {
  const { t } = useTranslation();
  const grouped = sources(citations);
  return (
    <>
      {grouped.list.length > 0 ? (
        <ol aria-label={name} className="mt-2 list-decimal border-t border-border pl-6 pt-2 text-caption">
          {grouped.list.map((source, i) => {
            const name = (source.live ? `${t("knowledge.chat.liveData")}: ` : "") + label(source);
            return (
              <li key={i} id={`${ids}-${i + 1}`}>
                {source.url ? <ExternalLink href={source.url}>{name}</ExternalLink> : name}
                {source.domain && !source.live ? <span className="ml-1 text-fg-muted">{source.domain}</span> : null}
              </li>
            );
          })}
        </ol>
      ) : null}
    </>
  );
}

/**
 * A data round's one numeric column as a small line or bar chart (T-4198), the widget's drawing.
 * The table above it is the text alternative, so the drawing is hidden from screen readers.
 */
function Chart({ chart }: { chart: DataChart }): JSX.Element {
  const W = 300;
  const H = 90;
  const top = 14;
  const base = top + H;
  const values = chart.points.map((p) => p.value);
  const low = Math.min(0, ...values);
  const high = Math.max(0, ...values);
  const span = high - low || 1;
  const y = (v: number) => top + H - ((v - low) / span) * H;
  const step = W / chart.points.length;
  return (
    <svg viewBox={`0 0 ${W} ${base + 14}`} aria-hidden="true" focusable="false" className="mt-2 block h-auto w-full" data-testid="assistant-chart">
      <line x1={0} x2={W} y1={y(0)} y2={y(0)} className="stroke-border" />
      {chart.kind === "line" ? (
        <polyline
          points={chart.points.map((p, i) => `${(i * step + step / 2).toFixed(1)},${y(p.value).toFixed(1)}`).join(" ")}
          className="fill-none stroke-primary-soft-fg"
          strokeWidth={2}
        />
      ) : (
        chart.points.map((p, i) => {
          const a = y(Math.max(p.value, 0));
          const b = y(Math.min(p.value, 0));
          return <rect key={i} x={i * step + step * 0.1} width={step * 0.8} y={a} height={Math.max(b - a, 1)} className="fill-primary-soft-fg" />;
        })
      )}
      <text x={0} y={10} fontSize={10} className="fill-fg-muted">{`${chart.value} ${Number(high.toPrecision(4))}`}</text>
      <text x={0} y={base + 12} fontSize={10} className="fill-fg-muted">{chart.points[0].label}</text>
      <text x={W} y={base + 12} textAnchor="end" fontSize={10} className="fill-fg-muted">{chart.points[chart.points.length - 1].label}</text>
    </svg>
  );
}

/** A data round's rows as the widget shows them (T-4140): a table, its chart and its links. */
function Data({ table }: { table: DataTable }): JSX.Element {
  const { t } = useTranslation();
  const chart = dataChart(table);
  return (
    <figure className="mt-2 text-caption">
      <Table caption={table.caption || t("knowledge.chat.data")}>
        <thead>
          <tr>
            {table.columns.map((c, i) => (
              <th key={i} scope="col" className="whitespace-nowrap px-2 py-1 text-left">
                {c}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {table.rows.map((row, r) => (
            <tr key={r}>
              {table.columns.map((_, c) => (
                <td key={c} className="whitespace-nowrap border-t border-border px-2 py-1">
                  {cellText(row[c])}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </Table>
      {chart ? <Chart chart={chart} /> : null}
      {table.dataset || table.download ? (
        <p className="mt-1 flex flex-wrap gap-3">
          {table.dataset ? <ExternalLink href={table.dataset}>{t("knowledge.chat.dataset")}</ExternalLink> : null}
          {table.download ? <ExternalLink href={table.download}>{t("knowledge.chat.download")}</ExternalLink> : null}
        </p>
      ) : null}
    </figure>
  );
}

/** The turns the next question sends back, as API/05 §1.1 bounds them. */
const HISTORY = 6;
const MAX_CHARS = 4_000;

function apply(turn: Shown, event: ChatEvent): Shown {
  switch (event.name) {
    case "tool":
      return {
        ...turn,
        reading: event.status === "started" ? (event.endpoint ?? event.tool) : turn.reading,
        tools: [...turn.tools, { tool: event.tool, endpoint: event.endpoint, status: event.status }],
      };
    case "script":
      return { ...turn, scripts: [...turn.scripts, { code: event.code, result: event.output ?? event.error ?? "" }] };
    case "answer":
      return { ...turn, text: event.text, reading: undefined };
    case "citations":
      return { ...turn, citations: event.citations };
    case "data":
      return { ...turn, data: [...turn.data, event.data] };
    case "error":
      return { ...turn, error: event.detail, reading: undefined };
    default:
      return turn;
  }
}

/**
 * The knowledge assistant as the signed-in person asks it (API/05 §1.7, AG-115): the same events
 * the public widget shows, the answer streamed, the person's own connectors switchable. Nothing
 * is kept past the dialog: the conversation lives in this panel, as in the widget.
 */
export function AssistantChat({
  project,
  deployment,
  connectors,
}: {
  project: string;
  deployment: string;
  connectors: string[];
}): JSX.Element {
  const { t } = useTranslation();
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

  useEffect(() => () => abort.current?.abort(), []);

  async function send(event?: FormEvent, again?: string): Promise<void> {
    event?.preventDefault();
    const message = (again ?? question).trim();
    if (!message || busy) return;
    const history: ChatTurn[] = turns
      // A question nothing answered is not history: asked again, it would be there twice.
      .filter((turn, i, all) => turn.text && !turn.error && (turn.role !== "user" || (all[i + 1] !== undefined && !all[i + 1].error)))
      .slice(-HISTORY)
      .map((turn) => ({ role: turn.role, text: turn.text.slice(0, MAX_CHARS) }));
    setTurns((all) => [
      ...all,
      { role: "user", text: message, tools: [], scripts: [], citations: [], data: [] },
      { role: "assistant", text: "", tools: [], scripts: [], citations: [], data: [] },
    ]);
    setQuestion("");
    setFailure(null);
    setLast(null);
    // The question comes back into the box when no answer does, to resend or reword (T-4160, finding 122).
    const missed = () => {
      setQuestion((now) => (now === "" ? message : now));
      setLast(message);
    };
    setBusy(true);
    abort.current = new AbortController();
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
        (streamed) => {
          if (streamed.name === "conversation") conversation.current = streamed.id;
          if (streamed.name === "error") missed();
          // The assistant words a failed model call for a public visitor, in Slovak; the Portal
          // says it in the person's language (T-4869).
          const event =
            streamed.name === "error" && streamed.title === "Model Unavailable"
              ? { ...streamed, detail: t("knowledge.chat.modelUnavailable") }
              : streamed;
          setTurns((all) => [...all.slice(0, -1), apply(all[all.length - 1], event)]);
        },
        abort.current.signal,
      );
    } catch (error) {
      if ((error as { name?: string }).name === "AbortError") return;
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
    <div className="space-y-3">
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
      <ol aria-live="polite" aria-label={t("knowledge.chat.conversation")} className="max-h-96 space-y-3 overflow-y-auto">
        {turns.map((turn, index) => (
          <li
            key={index}
            className={turn.role === "user" ? "ml-8 rounded-md bg-surface-subtle p-3" : "mr-8 rounded-md border border-border p-3"}
          >
            <span className="sr-only">{turn.role === "user" ? t("knowledge.chat.you") : t("knowledge.chat.assistant")}: </span>
            {turn.role === "assistant" && !turn.text && !turn.error ? (
              <p role="status" className="text-fg-muted">
                {turn.reading ? t("knowledge.chat.reading", { name: turn.reading }) : t("knowledge.chat.thinking")}
              </p>
            ) : null}
            {turn.text && turn.role === "user" ? <p className="whitespace-pre-wrap">{turn.text}</p> : null}
            {turn.data.map((table, n) => (
              <Data key={n} table={table} />
            ))}
            {turn.text && turn.role === "assistant" ? <Answer text={turn.text} citations={turn.citations} ids={`${ids}-${index}`} /> : null}
            {turn.error ? (
              <>
                <Alert role="alert" tone="danger">
                  {turn.error}
                </Alert>
                {!turn.text ? (
                  <Sources citations={turn.citations} ids={`${ids}-${index}`} label={t("knowledge.chat.closest")} />
                ) : null}
              </>
            ) : null}
            {turn.tools.some((tool) => tool.status === "failed") ? (
              <p className="text-caption text-fg-muted">
                {t("knowledge.chat.toolFailed", {
                  names: turn.tools
                    .filter((tool) => tool.status === "failed")
                    .map((tool) => tool.endpoint ?? tool.tool)
                    .join(", "),
                })}
              </p>
            ) : null}
            {turn.scripts.map((script, n) => (
              <details key={n} className="mt-2 text-caption">
                <summary className="cursor-pointer">{t("knowledge.chat.script")}</summary>
                <CodeBlock label={t("knowledge.chat.script")} className="mt-1 overflow-x-auto rounded bg-surface-subtle p-2">{script.code}</CodeBlock>
                <CodeBlock label={t("codeBlock.result")} className="mt-1 overflow-x-auto rounded bg-surface-subtle p-2">{script.result}</CodeBlock>
              </details>
            ))}
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
        <Field id={`knowledge-chat-${deployment}`} label={t("knowledge.chat.question")}>
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
        <Button
          type="submit"
          disabled={busy || question.trim() === ""}
          disabledReason={busy ? t("app.busyReason") : t("knowledge.askFirst")}
          loading={busy}
        >
          {t("knowledge.chat.send")}
        </Button>
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
