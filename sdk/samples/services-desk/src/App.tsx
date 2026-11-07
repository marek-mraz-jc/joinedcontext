import { useEffect, useState } from "react";
import { Card, displayName, format, Header, Page, Split, useClient, useEntities } from "@joinedcontext/sdk";
import type { FileInfo, JobStatus, Row } from "@joinedcontext/sdk";
import { Empty, Loading, Problem } from "./components/states";
import { refusal } from "./refusal";

const TYPE = "DefectReport";
const SEVERITY = { type: "object", required: ["severity"], properties: { severity: { type: "integer", minimum: 1, maximum: 5 } } };

/** The report's own folder in the App's files: the last segment of its id, nothing a person typed. */
export function folderOf(row: Row): string {
  return `reports/${(row.id.split(":").pop() ?? "").replace(/[^A-Za-z0-9._-]/g, "-")}/`;
}

/** A file name as a key: letters, digits, dot, dash and underscore, at most 100 of them. */
export function keyOf(name: string): string {
  return name.replace(/[^A-Za-z0-9._-]/g, "-").slice(-100) || "photo";
}

type Said = { ok: boolean; text: string } | null;

/** One action's last answer: a status when it went through, an alert naming the refusal when not. */
function Said({ said }: { said: Said }): React.JSX.Element | null {
  if (!said) return null;
  return said.ok ? <p role="status">{said.text}</p> : <p role="alert" className="app-refused">{said.text}</p>;
}

/** Runs one action and keeps what it said; a second click while it runs does nothing. */
function useAction(): [Said, boolean, (work: () => Promise<string>) => Promise<void>] {
  const [said, setSaid] = useState<Said>(null);
  const [busy, setBusy] = useState(false);
  const run = async (work: () => Promise<string>) => {
    if (busy) return;
    setBusy(true);
    try {
      setSaid({ ok: true, text: await work() });
    } catch (err) {
      setSaid({ ok: false, text: refusal(err) });
    } finally {
      setBusy(false);
    }
  };
  return [said, busy, run];
}

function Photos({ row }: { row: Row }): React.JSX.Element {
  const client = useClient();
  const folder = folderOf(row);
  const [photos, setPhotos] = useState<FileInfo[] | null>(null);
  const [said, busy, run] = useAction();
  useEffect(() => {
    void run(async () => {
      const found = await client.files.list(folder);
      setPhotos(found);
      return found.length === 1 ? "1 photo." : `${found.length} photos.`;
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [folder]);
  const upload = (file: File) =>
    run(async () => {
      await client.files.put(folder + keyOf(file.name), file, { contentType: file.type || undefined });
      const found = await client.files.list(folder);
      setPhotos(found);
      return `${file.name} is stored.`;
    });
  return (
    <section aria-label="Photos">
      <h3>Photos</h3>
      {photos && photos.length > 0 && (
        <ul className="app-files">
          {photos.map((photo) => (
            <li key={photo.path}>
              {photo.path.slice(folder.length)} · {Math.ceil(photo.size / 1024)} KiB
            </li>
          ))}
        </ul>
      )}
      <label className="app-upload">
        Add a photo
        <input type="file" accept="image/*" disabled={busy} onChange={(e) => e.target.files?.[0] && void upload(e.target.files[0])} />
      </label>
      <Said said={said} />
    </section>
  );
}

function Report({ row, onClosed }: { row: Row; onClosed: () => void }): React.JSX.Element {
  const client = useClient();
  const [rated, rating, rate] = useAction();
  const [mailed, mailing, mail] = useAction();
  const [closed, closing, close] = useAction();
  const summary = `${displayName(row)} (${String(row.district ?? "no district")}): ${String(row.description ?? "")}`;
  return (
    <Card title={displayName(row)} level={2}>
      <p>{String(row.description ?? "")}</p>
      <p className="app-meta">
        {String(row.district ?? "—")} · reported {format(row.dateReported as string, "date") || "—"} · {row.status === "closed" ? "Closed" : "Open"}
      </p>
      <Photos row={row} />
      <section aria-label="Actions" className="app-actions">
        <h3>Actions</h3>
        <div>
          <button
            type="button"
            disabled={rating}
            onClick={() =>
              void rate(async () => {
                const done = await client.ai.complete({
                  messages: [
                    { role: "system", content: "Rate a street defect from 1 (cosmetic) to 5 (dangerous). Answer only the rating." },
                    { role: "user", content: summary },
                  ],
                  maxTokens: 20,
                  schema: SEVERITY,
                });
                const severity = (done.json as { severity?: unknown } | undefined)?.severity;
                return typeof severity === "number" ? `Severity ${severity} of 5, as the model rates it.` : "The model gave no rating.";
              })
            }
          >
            Rate severity
          </button>
          <Said said={rated} />
        </div>
        <div>
          <button
            type="button"
            disabled={mailing}
            onClick={() =>
              void mail(async () => {
                await client.email.send({ to: "me", subject: `Defect report: ${displayName(row)}`, text: summary });
                return "Sent to your address.";
              })
            }
          >
            Email me this report
          </button>
          <Said said={mailed} />
        </div>
        <div>
          <button
            type="button"
            disabled={closing || row.status === "closed"}
            title={row.status === "closed" ? "This report is closed." : undefined}
            onClick={() =>
              void close(async () => {
                await client.functions.call("close-report", { id: row.id });
                onClosed();
                return "Closed.";
              })
            }
          >
            Close report
          </button>
          <Said said={closed} />
        </div>
      </section>
    </Card>
  );
}

function Jobs(): React.JSX.Element {
  const client = useClient();
  const [jobs, setJobs] = useState<JobStatus[] | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  useEffect(() => {
    client.jobs.list().then(setJobs, (err: unknown) => setProblem(refusal(err)));
  }, [client]);
  return (
    <Card title="Scheduled jobs" level={2}>
      {problem ? (
        <p role="alert" className="app-refused">
          {problem}
        </p>
      ) : !jobs ? (
        <Loading />
      ) : jobs.length === 0 ? (
        <Empty>This App has no scheduled job.</Empty>
      ) : (
        <ul className="app-jobs">
          {jobs.map((job) => (
            <li key={job.name}>
              <strong>{job.name}</strong> <code>{job.schedule}</code>
              <span>next {job.nextRun ? format(job.nextRun, "date") : "none within a year"}</span>
              {job.lastRun && (
                <span className={job.lastRun.ok ? "app-ok" : "app-failed"}>
                  last {job.lastRun.ok ? "ran" : "failed"} {format(job.lastRun.at, "date")}
                  {job.lastRun.message ? `: ${job.lastRun.message}` : ""}
                </span>
              )}
            </li>
          ))}
        </ul>
      )}
    </Card>
  );
}

/** The reports beside the chosen one, its photos and actions, and the App's scheduled jobs. */
export default function App(): React.JSX.Element {
  const { rows, loading, error, reload } = useEntities(TYPE);
  const [openId, setOpenId] = useState<string | null>(null);
  const open = rows.find((row) => row.id === openId) ?? rows[0] ?? null;
  return (
    <div className="jc-shell">
      <main className="jc-main">
        <Page label="Defect desk">
          <Header level={1} title="Defect desk" subtitle="Reports with their photos, a rating, a mail and the App's jobs" />
          <Split ratio="1:2">
            <Card title="Reports" level={2}>
              {loading ? (
                <Loading />
              ) : error ? (
                <Problem error={error} onRetry={reload} />
              ) : rows.length === 0 ? (
                <Empty>No report to work on.</Empty>
              ) : (
                <ul className="app-reports">
                  {rows.map((row) => (
                    <li key={row.id}>
                      <button type="button" aria-pressed={row.id === open?.id} onClick={() => setOpenId(row.id)}>
                        {displayName(row)}
                        <span>{row.status === "closed" ? "Closed" : "Open"}</span>
                      </button>
                    </li>
                  ))}
                </ul>
              )}
            </Card>
            {open ? <Report key={open.id} row={open} onClosed={reload} /> : <Empty>Choose a report.</Empty>}
          </Split>
          <Jobs />
        </Page>
      </main>
    </div>
  );
}
