/** email (ADR-N-045): send the signed-in person a summary; never a typed address. */
import { useState } from "react";
import { jc } from "@joinedcontext/sdk";
import { refusal } from "./refused";

export function MailMeTheSummary({ summary }: { summary: string }) {
  const [state, setState] = useState<string>("");
  async function send() {
    try {
      await jc().email.send({ to: "me", subject: "Today's summary", text: summary });
      setState("Sent.");
    } catch (err) {
      setState(refusal(err));
    }
  }
  return (
    <p>
      <button type="button" onClick={send}>Email me the summary</button> {state}
    </p>
  );
}
