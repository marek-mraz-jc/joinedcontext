/**
 * A `grid` widget of a Dashboard (T-1440; UI-18, UI-71, SDK-30).
 *
 * The same component the data explorer and a generated application render, configured by the same
 * object: the manifest's `grid`, with the source and the type supplied from the widget's own
 * `endpointRef` and `entityType` so a dashboard can never point it at another endpoint's data. The
 * read travels with the person's session, so the Endpoint's Policy decides what the grid shows and
 * whether a correction lands.
 */
import { useMemo } from "react";
import type { JSX, ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { parseGridConfig } from "@joinedcontext/sdk";
import { Card, CardHeader, InlineError } from "../ui";
import { PortalEntityGrid } from "../entities/PortalEntityGrid";

export function GridWidget({
  project,
  slug,
  type,
  config,
  title,
}: {
  project: string;
  slug: string;
  type: string;
  config?: Record<string, unknown>;
  title: ReactNode;
}): JSX.Element {
  const { t } = useTranslation();
  const parsed = useMemo(
    () => parseGridConfig({ ...(config ?? {}), source: { kind: "endpoint", slug }, type }),
    [config, slug, type],
  );

  return (
    <Card>
      {/* `Card` takes no `title`: passed to it, the widget's title landed on the div as the HTML
          `title` attribute — a tooltip nobody sees on a touch screen and nothing a screen reader
          lists. `CardHeader` at `h2` puts it under the dashboard's `h1` (T-2137, UI-16). */}
      <CardHeader as="h2" title={title} className="mb-3" />
      {parsed.config ? (
        <PortalEntityGrid project={project} config={parsed.config} />
      ) : (
        // jc-core refuses these fields when the manifest is written, so this is the case where a
        // dashboard was committed against an older platform: the reason is on the page, not in a
        // console.
        <InlineError>
          {t("dashboards.widget.badGrid", {
            reason: parsed.findings.map((finding) => `${finding.path}: ${finding.message}`).join("; "),
          })}
        </InlineError>
      )}
    </Card>
  );
}
