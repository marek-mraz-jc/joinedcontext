/** files (ADR-N-045): store a photo under the App's own prefix and list what is there. */
import { useState } from "react";
import { jc, type FileInfo } from "@joinedcontext/sdk";
import { refusal } from "./refused";

export function PhotoShelf() {
  const [files, setFiles] = useState<FileInfo[]>([]);
  const [problem, setProblem] = useState<string | null>(null);
  async function upload(file: File) {
    try {
      await jc().files.put(`photos/${file.name}`, file, { contentType: file.type });
      setFiles(await jc().files.list("photos/"));
    } catch (err) {
      setProblem(refusal(err));
    }
  }
  return (
    <section>
      <input type="file" accept="image/*" onChange={(e) => e.target.files?.[0] && upload(e.target.files[0])} />
      {problem && <p role="alert">{problem}</p>}
      <ul>{files.map((f) => <li key={f.path}>{f.path} · {f.size} bytes</li>)}</ul>
    </section>
  );
}
