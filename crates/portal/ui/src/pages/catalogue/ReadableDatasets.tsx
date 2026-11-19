import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import type { components } from "../../api/schema";
import { Badge } from "../../components/ui";
import type { BadgeTone } from "../../components/ui/Badge";

type ReadableDataset = components["schemas"]["ReadableDataset"];

const TONE: Record<string, BadgeTone> = { public: "success", organization: "primary", group: "accent", granted: "info" };

/**
 * What grants a dataset, in words: public, your organization, the group, or you alone. `words`
 * names the locale group, so an administrator's view of another person says "everyone in the
 * organization" instead of "your organization".
 */
export function AccessBadge({
  dataset,
  words = "catalogue.readable.access",
}: {
  dataset: Pick<ReadableDataset, "access" | "group">;
  words?: "catalogue.readable.access" | "dataAccess.personAccess";
}): JSX.Element {
  const { t } = useTranslation();
  return (
    <Badge tone={TONE[dataset.access] ?? "neutral"}>
      {t(`${words}.${dataset.access}`, { group: dataset.group ?? "" })}
    </Badge>
  );
}
