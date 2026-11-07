/** identity (ADR-N-045): who is signed in, and whether they may change a type's attribute. */
import { useAccess, useMe } from "@joinedcontext/sdk";

export function WhoMayEdit({ type, attr }: { type: string; attr: string }) {
  const me = useMe();
  const { can } = useAccess();
  const decision = can("updateAttrs", type, attr);
  return (
    <p>
      {me ? `Signed in as ${me.name ?? me.id}` : "Not signed in"}
      {" · "}
      {decision.ok ? `may edit ${attr}` : (decision.reason ?? `may not edit ${attr}`)}
    </p>
  );
}
