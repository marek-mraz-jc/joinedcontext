import { useEffect, useMemo, useState } from "react";
import { useEntities, useSchema } from "@joinedcontext/sdk";
import type { Summary } from "./summary";
import { numericAttributes, valuesOf, workerSummary } from "./summary";

const format = new Intl.NumberFormat(undefined, { maximumFractionDigits: 2 });

/**
 * The example's one screen: pick a type and one of its number attributes; the SDK fetches the
 * entities, and Rust compiled to WebAssembly summarizes the attribute in a Web Worker.
 */
export default function App() {
  const { schema, error } = useSchema();
  const types = useMemo(() => (schema ? Object.keys(schema).filter((t) => numericAttributes(schema, t).length > 0) : []), [schema]);
  const [type, setType] = useState("");
  const chosenType = type || types[0] || "";
  const attributes = useMemo(() => (schema && chosenType ? numericAttributes(schema, chosenType) : []), [schema, chosenType]);
  const [attribute, setAttribute] = useState("");
  const chosen = attributes.includes(attribute) ? attribute : attributes[0] ?? "";
  const { rows, loading, error: failed } = useEntities(chosenType, undefined, { enabled: Boolean(chosenType) });

  const ask = useMemo(() => workerSummary(new Worker(new URL("./summary.worker.ts", import.meta.url), { type: "module" })), []);
  const [summary, setSummary] = useState<Summary | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  useEffect(() => {
    if (!chosen || loading) return;
    let live = true;
    ask(valuesOf(rows, chosen)).then(
      (answer) => live && setSummary(answer),
      (why: Error) => live && setProblem(why.message),
    );
    return () => {
      live = false;
    };
  }, [ask, rows, chosen, loading]);

  if (error || failed) return <p role="alert">{(error ?? failed)?.message}</p>;
  if (!schema) return <p role="status">Loading…</p>;
  if (types.length === 0) return <p>No type of this endpoint has a number attribute.</p>;
  return (
    <main className="jc-page">
      <h1>Summary in WebAssembly</h1>
      <label>
        Type{" "}
        <select value={chosenType} onChange={(event) => setType(event.target.value)}>
          {types.map((t) => (
            <option key={t}>{t}</option>
          ))}
        </select>
      </label>{" "}
      <label>
        Attribute{" "}
        <select value={chosen} onChange={(event) => setAttribute(event.target.value)}>
          {attributes.map((a) => (
            <option key={a}>{a}</option>
          ))}
        </select>
      </label>
      {problem ? <p role="alert">{problem}</p> : null}
      {summary && !loading ? (
        <dl aria-label="Summary" data-testid="summary">
          <dt>Entities with a value</dt>
          <dd>{summary.count}</dd>
          {summary.count > 0
            ? (["min", "max", "mean", "median"] as const).map((key) => (
                <div key={key}>
                  <dt>{key}</dt>
                  <dd>{format.format(summary[key])}</dd>
                </div>
              ))
            : null}
        </dl>
      ) : (
        <p role="status">Computing…</p>
      )}
    </main>
  );
}
