import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import type { components } from "../api/schema";
import { Badge } from "./ui";

type ProposedVia = components["schemas"]["ProposedVia"];

/** The product names of the CIMD clients the server names by a short id (T-6390). */
const CLIENT_NAMES: Record<string, string> = {
  "claude-code": "Claude Code",
  "claude-ai": "claude.ai",
};

/** The client as a person reads it: its product name when known, else the id the server kept. */
export function clientName(client: string): string {
  return CLIENT_NAMES[client] ?? client;
}

/**
 * The AI client a change came through, read from its merge request's trailer (AG-137, T-5973):
 * "via Claude Code connector doprava-in-claude" when a connector opened it, else the client alone.
 */
export function ProposedViaBadge({ via }: { via: ProposedVia }): JSX.Element {
  const { t } = useTranslation();
  return (
    <Badge tone="accent" data-testid="change-proposed-via">
      {via.connectorName
        ? t("approvals.proposedVia.badgeConnector", {
            client: clientName(via.client),
            connector: via.connectorName,
          })
        : t("approvals.proposedVia.badge", { client: clientName(via.client) })}
    </Badge>
  );
}

/** The sentence under the author: who confirmed, through which connector, after which plan. */
export function ProposedViaHint({
  via,
  author,
}: {
  via: ProposedVia;
  author: string;
}): JSX.Element {
  const { t } = useTranslation();
  return (
    <>
      <dd className="mt-1 text-xs text-fg-muted">
        {via.connectorName
          ? t("approvals.proposedVia.hintConnector", {
              client: clientName(via.client),
              connector: via.connectorName,
              author,
            })
          : t("approvals.proposedVia.hint", { client: clientName(via.client), author })}
      </dd>
      {via.plan ? (
        <dd className="mt-1 text-xs text-fg-muted" data-testid="change-proposed-plan">
          {t("approvals.proposedVia.plan", { plan: via.plan, author })}
        </dd>
      ) : null}
    </>
  );
}
