import type { JSX } from "react";
import { clsx } from "clsx";
import { logoUrl, useBranding } from "../../branding";

// The frame pieces a public page draws too. They live outside Shell.tsx so the catalogue's
// first load does not pull the signed-in shell (assistant, menus, palette) into the entry (T-5122).

const NAV_LINK =
  "focus-ring-inset flex items-center gap-2.5 rounded-md px-2.5 py-2 text-body text-fg-muted transition-colors hover:bg-surface-muted hover:text-fg";
const NAV_LINK_ACTIVE = "bg-primary-soft font-semibold text-primary-soft-fg hover:bg-primary-soft hover:text-primary-soft-fg";

/** The instance name and its logo, which is what every page is titled with (UI-30). */
export function BrandMark({ short = false }: { short?: boolean }): JSX.Element {
  const branding = useBranding();
  const logo = logoUrl(branding);
  const name = short ? (branding.shortName ?? branding.instanceName) : branding.instanceName;
  return (
    <span className="inline-flex min-w-0 items-center gap-2.5">
      {logo ? (
        <img src={logo} alt="" aria-hidden="true" className="h-7 w-auto max-w-32 shrink-0 object-contain" />
      ) : (
        <span
          aria-hidden="true"
          className="inline-flex size-7 shrink-0 items-center justify-center rounded-md bg-primary text-caption font-bold text-primary-fg"
        >
          {name.trim().charAt(0).toUpperCase()}
        </span>
      )}
      {/* Beside a logo the name is cut to "joi…" on a phone and at 200 % zoom; below md the logo
          stands alone and the name stays for a screen reader (T-4165, audits T-4157, T-4163). */}
      <span className={clsx("truncate font-heading font-semibold tracking-tight", logo && "max-md:sr-only")}>
        {name}
      </span>
    </span>
  );
}

export function navLinkClass(active: boolean): string {
  return clsx(NAV_LINK, active && NAV_LINK_ACTIVE);
}
