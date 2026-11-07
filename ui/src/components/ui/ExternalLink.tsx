import type { AnchorHTMLAttributes, JSX, ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { clsx } from "clsx";
import { Icon } from "./icons";
import { safeHref } from "./safeHref";

export interface ExternalLinkProps
  extends Omit<AnchorHTMLAttributes<HTMLAnchorElement>, "href" | "rel" | "target"> {
  /** Where it goes. A scheme that is not http, https or mailto renders as text, never as a link. */
  href: string | undefined | null;
  children: ReactNode;
  /** Leave the icon out where the surrounding text already says the link leaves the Portal. */
  hideIcon?: boolean;
}

/**
 * PF-50, UI-16: a link that leaves the Portal. It carries `rel="noopener noreferrer"` once,
 * here, instead of being written out at each of the thirteen places that need it; it says it
 * opens a new tab, to the eye with an icon and to a screen reader in words; and an `href` the
 * Portal did not write — a URL out of a manifest, a catalogue or a status field — is checked
 * before it becomes a link at all.
 */
export function ExternalLink({
  href,
  children,
  hideIcon,
  className,
  ...rest
}: ExternalLinkProps): JSX.Element {
  const { t } = useTranslation();
  const safe = safeHref(href);
  const opensNewTab = t("app.opensNewTab");

  if (!safe) {
    // The words stay; only the link is withheld. A person still reads what was there, and a
    // `javascript:` URL out of a manifest never becomes something to click.
    return (
      <span className={className} {...rest}>
        {children}
      </span>
    );
  }

  return (
    <a
      href={safe}
      target="_blank"
      rel="noopener noreferrer"
      className={clsx("focus-ring inline-flex items-center gap-1 underline", className)}
      {...rest}
    >
      {children}
      {hideIcon ? null : <Icon name="external" className="size-3.5" aria-hidden="true" />}
      <span className="sr-only">{opensNewTab}</span>
    </a>
  );
}
