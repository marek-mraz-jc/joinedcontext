import { useCallback, useEffect, useState } from "react";
import type { FormEvent } from "react";
import type { Note, NotesApi } from "./notes";

/** The notes, a form for a new one, and per note: delete, attach a file, open it. */
export default function App({ api }: { api: NotesApi }) {
  const [notes, setNotes] = useState<Note[] | null>(null);
  const [draft, setDraft] = useState("");
  const [problem, setProblem] = useState<string | null>(null);

  const reload = useCallback(() => {
    api.list().then(setNotes, (error: Error) => setProblem(error.message));
  }, [api]);
  useEffect(reload, [reload]);

  const act = (work: Promise<unknown>) => {
    setProblem(null);
    work.then(reload, (error: Error) => setProblem(error.message));
  };

  const add = (event: FormEvent) => {
    event.preventDefault();
    if (!draft.trim()) return;
    act(api.add(draft).then(() => setDraft("")));
  };

  return (
    <main className="notes">
      <h1>Notes</h1>
      <form onSubmit={add}>
        <label htmlFor="new-note">New note</label>
        <textarea id="new-note" value={draft} maxLength={2000} onChange={(event) => setDraft(event.target.value)} />
        <button type="submit" disabled={!draft.trim()}>
          Add
        </button>
      </form>
      {problem ? <p role="alert">{problem}</p> : null}
      {notes === null ? (
        <p role="status">Loading…</p>
      ) : notes.length === 0 ? (
        <p>No notes yet.</p>
      ) : (
        <ul aria-label="Notes">
          {notes.map((note) => (
            <li key={note.id}>
              <p>{note.body}</p>
              <button type="button" onClick={() => act(api.remove(note.id))} aria-label={`Delete note ${note.id}`}>
                Delete
              </button>
              <label>
                {note.file ? "Replace the file" : "Attach a file"}
                <input type="file" onChange={(event) => event.target.files?.[0] && act(api.upload(note.id, event.target.files[0]))} />
              </label>
              {note.file ? (
                <button type="button" onClick={() => api.downloadUrl(note.id).then((url) => window.open(url, "_blank", "noopener"), (e: Error) => setProblem(e.message))}>
                  Open the file
                </button>
              ) : null}
            </li>
          ))}
        </ul>
      )}
    </main>
  );
}
