import { useId } from "react";
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
        <JcSymbol />
      )}
      {/* Beside a logo the name is cut to "joi…" on a phone and at 200 % zoom; below md the logo
          stands alone and the name stays for a screen reader (T-4165, audits T-4157, T-4163). */}
      <span className={clsx("truncate font-heading font-semibold tracking-tight", logo && "max-md:sr-only")}>
        {name}
      </span>
    </span>
  );
}

// The two hexagons drawn as one interlocking pair: each mask cuts a gap where the other ring
// passes over it. Light theme in the lockup's own colours; in the dark theme the indigo ring
// is lightened, #2f3f9e on the dark surface is under 3:1. Cut from the dataroom lockup
// crates/dataroom/web/assets/public__jc-logo.svg (symbol only, its translate dropped).
const RING_A = "29.0 21.2 21.2 29.0 10.6 26.1 7.7 15.5 15.5 7.7 26.1 10.6";
const RING_B = "38.1 31.8 30.3 39.6 19.7 36.7 16.8 26.1 24.6 18.3 35.2 21.2";

/** The joinedcontext symbol, drawn when the installation configures no logo. */
function JcSymbol(): JSX.Element {
  // One id set per drawing: the frame draws the mark more than once on a page.
  const id = useId().replace(/[^A-Za-z0-9_-]/g, "");
  const ring = (points: string, stroke: string, width: number, className?: string): JSX.Element => (
    <polygon points={points} fill="none" stroke={stroke} strokeWidth={width} strokeLinejoin="round" className={className} />
  );
  return (
    <svg viewBox="4.9 5.6 36 36" aria-hidden="true" focusable="false" className="size-7 shrink-0" data-testid="jc-symbol">
      <defs>
        <clipPath id={`${id}cA`}>
          <polygon points="-64 -77 100 114 5 196 -159 5" />
        </clipPath>
        <clipPath id={`${id}cB`}>
          <polygon points="-64 -77 100 114 196 32 32 -159" />
        </clipPath>
        <mask id={`${id}mA`} maskUnits="userSpaceOnUse" x="-50" y="-50" width="150" height="150">
          <rect x="-50" y="-50" width="150" height="150" fill="white" />
          <g clipPath={`url(#${id}cB)`}>{ring(RING_B, "black", 6)}</g>
        </mask>
        <mask id={`${id}mB`} maskUnits="userSpaceOnUse" x="-50" y="-50" width="150" height="150">
          <rect x="-50" y="-50" width="150" height="150" fill="white" />
          <g clipPath={`url(#${id}cA)`}>{ring(RING_A, "black", 6)}</g>
        </mask>
      </defs>
      <g mask={`url(#${id}mA)`}>{ring(RING_A, "#2f3f9e", 4, "dark:stroke-[#8c9cf5]")}</g>
      <g mask={`url(#${id}mB)`}>{ring(RING_B, "#0e8fa3", 4)}</g>
    </svg>
  );
}

export function navLinkClass(active: boolean): string {
  return clsx(NAV_LINK, active && NAV_LINK_ACTIVE);
}
