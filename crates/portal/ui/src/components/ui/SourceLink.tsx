import type { JSX } from "react";
import { buttonClass } from "./Button";
import { Icon } from "./icons";
import { safeHref } from "./safeHref";

export interface SourceLinkProps {
  /** `status.sourceUrl`, which a manifest author writes: data, and checked before it is a link. */
  href: string | undefined | null;
  /** The link's accessible name, also its tooltip: the icon alone is what shows. */
  label: string;
}

/**
 * The manifest or the code in the forge, as one icon beside the row or the run (MF-11).
 *
 * `status.sourceUrl` is written by whoever authored the manifest, so it is data: a `javascript:`
 * URL there would run on this origin the moment somebody clicked the icon. An address that
 * cannot navigate draws no icon at all — an icon that does nothing is worse than none (PF-50).
 */
export function SourceLink({ href, label }: SourceLinkProps): JSX.Element | null {
  const safe = safeHref(href);
  if (!safe) {
    return null;
  }
  return (
    <a
      href={safe}
      target="_blank"
      rel="noreferrer"
      aria-label={label}
      title={label}
      className={buttonClass("ghost", "sm", "text-primary-soft-fg")}
    >
      <Icon name="git" className="size-4" />
    </a>
  );
}
