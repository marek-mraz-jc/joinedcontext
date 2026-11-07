import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { readCard } from "../../pages/mcp/mcp";
import { ASSISTANT_EVENT } from "../../navigation/shortcuts";
import { CodeBlock, Button } from "../ui";
import { CopyUrlButton, representationUrl } from "./links";

/**
 * The Endpoint's NGSI-LD query card beside its MCP address (T-4760): the text the gateway serves
 * as `card://{slug}` for this person, to paste into any AI client, and "Ask the assistant", which
 * opens the Portal's assistant on this one Endpoint. The card is never shown to a person whose
 * grants do not reach the Endpoint: the gateway refuses it, and the page says so.
 */
export function QueryCard({
  slug,
  name,
  domain,
}: {
  slug: string;
  name: string;
  domain: string | undefined;
}): JSX.Element {
  const { t } = useTranslation();
  const card = useQuery({
    queryKey: ["endpoint-card", slug],
    queryFn: ({ signal }) => readCard(slug, signal),
    retry: false,
  });
  const mcp = representationUrl(slug, "mcp", domain);
  return (
    <section aria-labelledby="query-card" className="flex flex-col gap-3">
      <div>
        <h2 id="query-card" className="text-title font-semibold text-fg">
          {t("endpoints.card.title")}
        </h2>
        <p className="mt-1 text-body text-fg-muted">
          {t("endpoints.card.lead")}
        </p>
      </div>
      <div className="flex flex-wrap items-center gap-2">
        <span className="text-caption text-fg-muted">
          {t("endpoints.card.mcp")}
        </span>
        <code className="break-all font-mono text-caption">{mcp}</code>
        <CopyUrlButton url={mcp} />
      </div>
      {card.isPending ? (
        <p role="status" className="text-caption text-fg-muted">
          {t("endpoints.card.reading")}
        </p>
      ) : card.isError ? (
        // Not an alert: the page works without its card, and its own errors keep the one alert.
        <p role="status" className="text-caption text-fg-muted">
          {t("endpoints.card.failed")}
        </p>
      ) : card.data.state === "refused" ? (
        <p role="status" className="text-caption text-fg-muted">
          {t("endpoints.card.refused")}
        </p>
      ) : (
        <div className="flex flex-col gap-1">
          <CodeBlock
            label={t("endpoints.card.title")}
            className="max-h-96 whitespace-pre-wrap rounded bg-surface-muted p-2 font-mono text-caption"
          >
            {card.data.text}
          </CodeBlock>
          <div>
            <CopyUrlButton
              url={card.data.text}
              label={t("endpoints.card.copy")}
            />
          </div>
        </div>
      )}
      <div>
        <Button
          variant="primary"
          size="sm"
          onClick={() =>
            window.dispatchEvent(
              new CustomEvent(ASSISTANT_EVENT, {
                detail: { endpoints: [name] },
              }),
            )
          }
        >
          {t("endpoints.card.ask")}
        </Button>
      </div>
    </section>
  );
}
