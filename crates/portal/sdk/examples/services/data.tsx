/** data (ADR-N-045): read a type through the App's Endpoint and write one attribute back. */
import { useEntities, useSave } from "@joinedcontext/sdk";

export function Notes({ type }: { type: string }) {
  const { rows, loading } = useEntities(type, { attrs: ["name", "note"] });
  const { update, saving, problem } = useSave();
  if (loading) return <p>Loading…</p>;
  return (
    <ul>
      {rows.map((row) => (
        <li key={row.id}>
          {String(row.name ?? row.id)}
          <button disabled={saving} onClick={() => void update(row.id, { note: "checked" })}>
            Mark checked
          </button>
        </li>
      ))}
      {problem ? <li role="alert">{problem.detail ?? problem.title}</li> : null}
    </ul>
  );
}
