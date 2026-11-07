/** ai (ADR-N-045): classify a report with the platform's model, as one value of a JSON Schema. */
import { useState } from "react";
import { jc } from "@joinedcontext/sdk";
import { refusal } from "./refused";

const SEVERITY = { type: "object", required: ["severity"], properties: { severity: { type: "integer", minimum: 1, maximum: 5 } } };

export function Severity({ report }: { report: string }) {
  const [answer, setAnswer] = useState<string>("");
  async function classify() {
    try {
      const done = await jc().ai.complete({
        messages: [{ role: "system", content: "Rate a road defect from 1 (cosmetic) to 5 (dangerous)." }, { role: "user", content: report }],
        maxTokens: 50,
        schema: SEVERITY,
      });
      setAnswer(`Severity ${(done.json as { severity: number }).severity}`);
    } catch (err) {
      setAnswer(refusal(err));
    }
  }
  return (
    <p>
      <button type="button" onClick={classify}>Rate it</button> {answer}
    </p>
  );
}
