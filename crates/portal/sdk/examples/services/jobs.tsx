/** jobs (ADR-N-045): show the App's schedules and how each last ran. */
import { useEffect, useState } from "react";
import { jc, type JobStatus } from "@joinedcontext/sdk";
import { refusal } from "./refused";

export function JobList() {
  const [jobs, setJobs] = useState<JobStatus[]>([]);
  const [problem, setProblem] = useState<string | null>(null);
  useEffect(() => {
    jc().jobs.list().then(setJobs, (err: unknown) => setProblem(refusal(err)));
  }, []);
  if (problem) return <p role="alert">{problem}</p>;
  return (
    <ul>
      {jobs.map((j) => (
        <li key={j.name}>
          {j.name} ({j.schedule}): next {j.nextRun ?? "none within a year"}
          {j.lastRun && `, last ${j.lastRun.ok ? "ok" : "failed"} at ${j.lastRun.at}`}
        </li>
      ))}
    </ul>
  );
}
