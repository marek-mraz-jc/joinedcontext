import { forwardRef } from "react";
import type { HTMLAttributes } from "react";
import { clsx } from "clsx";

export type BadgeTone = "neutral" | "primary" | "success" | "warning" | "danger" | "info" | "accent";

const TONES: Record<BadgeTone, string> = {
  neutral: "border-border bg-surface-subtle text-fg-muted",
  primary: "border-primary-200 bg-primary-soft text-primary-soft-fg",
  success: "border-success/30 bg-success-soft text-success",
  warning: "border-warning/30 bg-warning-soft text-warning",
  danger: "border-danger/30 bg-danger-soft text-danger",
  info: "border-info/30 bg-info-soft text-info",
  // Was `purple`, on Tailwind's own `purple-500`: the one tone in the Portal that ignored both
  // the dark theme and the installation's brand, because no `--color-purple-*` exists.
  accent: "border-accent/40 bg-accent/15 text-fg",
};

export interface BadgeProps extends HTMLAttributes<HTMLSpanElement> {
  tone?: BadgeTone;
  /** Monospace: for a slug, a representation name, a secret reference. */
  mono?: boolean;
}

/**
 * A small labelled chip; the tone is never the only carrier of meaning, the text is (UI-30).
 *
 * The text wraps. A chip is put in a table cell and handed a translated label — "natives
 * Bloblang, ungeprüft", "Wartet auf Freigabe" — and `whitespace-nowrap` made those push the
 * column wide or run past it with no way to read the end.
 *
 * Wrapping between words is not enough for the one label that has none: an entity id
 * (`urn:ngsi-ld:AirQualityObserved:banskabystrica:ovzdusie:stanica-01`) is a single token, so its
 * minimum width is the whole string and every ancestor inherits it. At 400px that pushed the
 * page sideways against UI-27 — and only where the monospace font draws a shade wider, which is
 * why it failed on the CI runner and nowhere else (T-2446). `anywhere`, not `break-word`: only
 * `anywhere` lets the break opportunity count towards the minimum width, which is the number
 * that decides whether an ancestor can shrink.
 *
 * On `mono` alone, which is the variant that carries an id. `anywhere` puts a break opportunity
 * between every pair of characters, so a chip that is a flex item shrinks to one letter if the
 * row is tight: applied to every chip it turned "Pending approval" into a circle reading
 * "Pendin g approv al". A label of words wraps between them and keeps the longest word as its
 * minimum, which is what a person reads.
 */
export const Badge = forwardRef<HTMLSpanElement, BadgeProps>(function Badge(
  { tone = "neutral", mono, className, children, ...rest },
  ref,
): React.JSX.Element {
  return (
    <span
      ref={ref}
      className={clsx(
        "inline-flex items-center gap-1 rounded-full border px-2 py-0.5 text-caption font-medium",
        mono && "font-mono [overflow-wrap:anywhere]",
        TONES[tone],
        className,
      )}
      {...rest}
    >
      {children}
    </span>
  );
});
