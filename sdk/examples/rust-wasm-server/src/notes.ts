/**
 * The example's server, `/apps/{name}/api` (T-3341): its notes and their files. A file goes from
 * the browser to the store and back through the presigned URL the server hands out, never through
 * the server itself.
 */
export interface Note {
  id: number;
  body: string;
  file: string | null;
  created_at: string;
}

/** A problem+json answer as an error whose message is its `detail`. */
export class Problem extends Error {
  constructor(
    readonly status: number,
    detail: string,
  ) {
    super(detail);
  }
}

/** Where the server answers: `/apps/{appName}/api`, the App's name from `#jc-config`. */
export function apiBase(doc: Document = document): string {
  const text = doc.getElementById("jc-config")?.textContent ?? "";
  let name = "";
  try {
    name = String((JSON.parse(text || "{}") as { appName?: unknown }).appName ?? "");
  } catch {
    name = "";
  }
  return name && /^[a-z0-9-]+$/.test(name) ? `/apps/${name}/api` : "/api";
}

async function call<T>(base: string, method: string, path: string, body?: unknown): Promise<T> {
  const answer = await fetch(`${base}${path}`, {
    method,
    credentials: "same-origin",
    headers: body === undefined ? { Accept: "application/json" } : { Accept: "application/json", "Content-Type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (!answer.ok) {
    const problem = (await answer.json().catch(() => ({}))) as { detail?: string };
    throw new Problem(answer.status, problem.detail ?? `The server answered ${answer.status}.`);
  }
  return (answer.status === 204 ? undefined : await answer.json()) as T;
}

export function notesApi(base: string) {
  return {
    list: () => call<Note[]>(base, "GET", "/notes"),
    add: (body: string) => call<Note>(base, "POST", "/notes", { body }),
    change: (id: number, body: string) => call<void>(base, "PUT", `/notes/${id}`, { body }),
    remove: (id: number) => call<void>(base, "DELETE", `/notes/${id}`),
    /** Asks the server for an upload URL, then sends the file there itself. */
    upload: async (id: number, file: File) => {
      const { url } = await call<{ url: string }>(base, "POST", `/notes/${id}/file`, { name: file.name, contentType: file.type || undefined });
      const put = await fetch(url, { method: "PUT", body: file, headers: file.type ? { "Content-Type": file.type } : {} });
      if (!put.ok) throw new Problem(put.status, "The file could not be stored.");
    },
    downloadUrl: async (id: number) => (await call<{ url: string }>(base, "GET", `/notes/${id}/file`)).url,
  };
}

export type NotesApi = ReturnType<typeof notesApi>;
