// T-7706: the job list renders empty on the server, where the browser's storage does not exist.
import { renderToString } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { useJobs } from "../src/jobs";

function Count() {
  return <p>{`${useJobs().length} jobs`}</p>;
}

describe("useJobs on the server", () => {
  it("lists no job", () => {
    window.localStorage.setItem("jc.jobs", JSON.stringify([{ id: "j1", kind: "appBuild", project: "helsinki", name: "x", startedAt: new Date().toISOString() }]));
    expect(renderToString(<Count />)).toContain("0 jobs");
    window.localStorage.clear();
  });
});
