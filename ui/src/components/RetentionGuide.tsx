import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { guideUrl, useBranding } from "../branding";
import { ExternalLink } from "./ui";

/** The User Guide's retention page, which every expiry and retention choice links (T-4822). */
export const RETENTION_GUIDE = "User-Guide/14-retention-and-expiry";

/** The sections of that page the Portal opens, by their heading's anchor. */
export type RetentionSection =
  | "2-every-way-data-is-removed"
  | "which-one-wins"
  | "4-recipe-buses-and-live-vehicles"
  | "8-recipe-data-imported-from-a-file";

/**
 * A link to one section of the retention guide, only when the installation serves a guide:
 * `guideUrl` answers nothing without a documentation address, so no dead link shows (UI-02).
 */
export function RetentionGuide({ section }: { section: RetentionSection }): JSX.Element | null {
  const { t } = useTranslation();
  const href = guideUrl(useBranding(), `${RETENTION_GUIDE}#${section}`);
  if (href === undefined) return null;
  return (
    <ExternalLink href={href} className="text-caption underline">
      {t(`retentionGuide.${section}`)}
    </ExternalLink>
  );
}
