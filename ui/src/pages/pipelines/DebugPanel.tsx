/**
 * The debug side of the pipeline graph (T-3222, PL-43, PL-67): what the opened node changed in the
 * message, the input that broke a failing node and a rerun of that node alone on it, and the
 * Debug sidebar with what crossed every wire a Debug node taps, message by message. Every run is
 * the dry-run test: nothing is written to a space.
 */
import { useState } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { testPipeline } from "../../api/pipelineTest";
import type { Trace } from "../../api/pipelineTest";
import { RecordView, Alert, Button, Field, Select } from "../../components/ui";
import type { PipelineForm } from "./PipelineEditor";
import { paintOf, samplesOf } from "./PipelineFlow";
import type { FlowEdge, FlowNode, FlowNodeId } from "./PipelineFlow";
import { diffOf, onlyStep, wireKey } from "./pipelineDebug";

const box = "max-h-36";

export interface NodeDebugProps {
  project: string;
  form: PipelineForm;
  trace: Trace;
  nodes: FlowNode[];
  id: FlowNodeId;
  toManifest: (form: PipelineForm) => unknown;
}

/** What one node did to the message, and, when it failed, its input pinned for a rerun of it alone. */
export function NodeDebug({ project, form, trace, nodes, id, toManifest }: NodeDebugProps): JSX.Element {
  const { t } = useTranslation();
  const samples = samplesOf(trace, nodes, id);
  const changes = samples.in !== undefined && samples.out !== undefined ? diffOf(samples.in, samples.out) : [];
  const error = paintOf(trace, nodes)[id]?.error;
  const alone = onlyStep(form, id);
  const [rerun, setRerun] = useState<{ trace?: Trace; failed?: string } | null>(null);
  const [running, setRunning] = useState(false);

  async function runAlone() {
    if (!alone || samples.in === undefined) return;
    setRunning(true);
    try {
      const answered = await testPipeline(
        project,
        toManifest(alone),
        typeof samples.in === "string"
          ? { text: samples.in, format: "text" }
          : { text: JSON.stringify(Array.isArray(samples.in) ? samples.in : [samples.in]), format: "json" },
      );
      setRerun(
        "trace" in answered
          ? { trace: answered.trace }
          : { failed: answered.detail ?? t("pipelines.test.failed", { status: answered.status }) },
      );
    } catch {
      setRerun({ failed: t("pipelines.test.failed", { status: 0 }) });
    } finally {
      setRunning(false);
    }
  }

  return (
    <div className="flex flex-col gap-2" data-testid="flow-node-debug">
      {error ? (
        <Alert tone="danger" title={t("pipelines.debug.failedHere")}>
          <p>{error}</p>
          <p className="mt-1 text-caption">{t("pipelines.debug.brokeIt")}</p>
          {samples.in === undefined ? (
            <p data-testid="flow-breaking-input">{t("pipelines.flow.noSample")}</p>
          ) : (
            <RecordView value={samples.in} label={t("codeBlock.sample")} className={box} data-testid="flow-breaking-input" />
          )}
        </Alert>
      ) : null}
      {samples.in !== undefined && samples.out !== undefined ? (
        <div className="flex flex-col gap-1">
          <span className="text-caption font-semibold text-fg">{t("pipelines.debug.changed")}</span>
          {changes.length === 0 ? (
            <p className="text-caption text-fg-muted">{t("pipelines.debug.unchanged")}</p>
          ) : (
            <ul className="flex flex-col gap-0.5 font-mono text-caption" aria-label={t("pipelines.debug.changed")}>
              {changes.map((change) => (
                <li key={`${change.kind}-${change.path}`}>
                  <span className={change.kind === "removed" ? "text-danger-fg" : change.kind === "added" ? "text-success" : "text-fg"}>
                    {t(`pipelines.debug.kind.${change.kind}`)}
                  </span>{" "}
                  {change.path}
                  {change.kind === "changed" ? `: ${JSON.stringify(change.before)} → ${JSON.stringify(change.after)}` : null}
                  {change.kind === "added" ? `: ${JSON.stringify(change.after)}` : null}
                </li>
              ))}
            </ul>
          )}
        </div>
      ) : null}
      {alone && samples.in !== undefined ? (
        <div className="flex flex-col gap-1">
          <div className="flex flex-wrap items-center gap-2">
            <Button size="sm" variant="secondary" loading={running} onClick={() => void runAlone()}>
              {t("pipelines.debug.rerun")}
            </Button>
            <span className="text-caption text-fg-muted">{t("pipelines.debug.rerunHint")}</span>
          </div>
          <div role="status" data-testid="flow-rerun">
            {rerun?.failed ? (
              <Alert tone="danger">{rerun.failed}</Alert>
            ) : rerun?.trace ? (
              rerun.trace.errors.length > 0 ? (
                <Alert tone="danger" title={t("pipelines.debug.rerunFailed")}>
                  <ul className="text-caption">
                    {rerun.trace.errors.map((one, at) => (
                      <li key={at}>{one.message}</li>
                    ))}
                  </ul>
                </Alert>
              ) : (
                <Alert tone="success" title={t("pipelines.debug.rerunOk")}>
                  <RecordView value={rerun.trace.stages?.[0]?.sample ?? rerun.trace.mapping[0]} label={t("codeBlock.sample")} className={box} />
                </Alert>
              )
            ) : null}
          </div>
        </div>
      ) : null}
    </div>
  );
}

/** One run of the test, and the message of the sample it ran on in step mode (or all of them). */
export interface DebugEntry {
  message: number | null;
  trace: Trace;
}

export interface DebugSidebarProps {
  log: DebugEntry[];
  nodes: FlowNode[];
  edges: FlowEdge[];
  taps: string[];
  nameOf: (id: FlowNodeId) => string;
  onClear: () => void;
}

/** What crossed each tapped wire in each run, newest first, filterable by node (PL-67). */
export function DebugSidebar({ log, nodes, edges, taps, nameOf, onClear }: DebugSidebarProps): JSX.Element {
  const { t } = useTranslation();
  const [node, setNode] = useState<string>("");
  const tapped = edges.filter((edge) => taps.includes(wireKey(edge)));
  const watched = tapped.filter((edge) => node === "" || edge.from === node || edge.to === node);
  const rows = [...log].reverse().flatMap((entry, at) =>
    watched.map((edge) => ({ key: `${log.length - at}-${wireKey(edge)}`, entry, edge })),
  );

  return (
    <section aria-labelledby="flow-debug-title" className="flex flex-col gap-2 rounded-md border border-border bg-surface p-3" data-testid="flow-debug">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h3 id="flow-debug-title" className="text-body font-semibold text-fg">
          {t("pipelines.debug.title")}
        </h3>
        <Button size="sm" variant="ghost" disabled={log.length === 0} disabledReason={t("pipelines.debug.nothingToClear")} onClick={onClear}>
          {t("pipelines.debug.clear")}
        </Button>
      </div>
      {tapped.length === 0 ? (
        <p className="text-caption text-fg-muted">{t("pipelines.debug.noTaps")}</p>
      ) : (
        <>
          <Field id="flow-debug-node" label={t("pipelines.debug.filter")}>
            <Select id="flow-debug-node" value={node} onChange={(e) => setNode(e.target.value)}>
              <option value="">{t("pipelines.debug.everyNode")}</option>
              {nodes.map((one) => (
                <option key={one.id} value={one.id}>
                  {nameOf(one.id)} · {one.kind}
                </option>
              ))}
            </Select>
          </Field>
          {rows.length === 0 ? (
            <p className="text-caption text-fg-muted">{t("pipelines.debug.nothingYet")}</p>
          ) : (
            <ol className="flex flex-col gap-2" aria-label={t("pipelines.debug.messages")}>
              {rows.map(({ key, entry, edge }) => {
                const sample = samplesOf(entry.trace, nodes, edge.to).in;
                return (
                  <li key={key} className="flex flex-col gap-1 rounded-md border border-border p-2">
                    <span className="text-caption text-fg-muted">
                      {entry.message === null
                        ? t("pipelines.debug.wholeSample")
                        : t("pipelines.debug.message", { number: entry.message + 1 })}{" "}
                      · {t("pipelines.debug.wire", { from: nameOf(edge.from), to: nameOf(edge.to) })}
                    </span>
                    {sample === undefined ? (
                      <p className="text-caption text-fg-muted">{t("pipelines.debug.notReached")}</p>
                    ) : (
                      <RecordView value={sample} label={t("codeBlock.sample")} className={box} />
                    )}
                  </li>
                );
              })}
            </ol>
          )}
        </>
      )}
    </section>
  );
}
