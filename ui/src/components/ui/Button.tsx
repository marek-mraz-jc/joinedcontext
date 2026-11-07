import { forwardRef, useId } from "react";
import type { ButtonHTMLAttributes, ReactNode } from "react";
import { clsx } from "clsx";

export type ButtonVariant = "primary" | "secondary" | "ghost" | "danger";
export type ButtonSize = "xs" | "sm" | "md" | "lg";

// No `whitespace-nowrap`: a German label is half again as long as its English original
// ("Eine Änderung mit dem Assistenten vorschlagen"), and with `shrink-0` beside it the label ran
// past its container instead of wrapping (UI-30).
// `touch-size`: a `Link` that wears these classes is a finger's 44 px like the buttons (T-5596).
const BASE =
  "focus-ring touch-size inline-flex shrink-0 select-none items-center justify-center gap-1.5 rounded-md font-medium transition-colors disabled:pointer-events-none disabled:opacity-50 aria-disabled:opacity-50 aria-disabled:cursor-not-allowed";

const VARIANTS: Record<ButtonVariant, string> = {
  primary: "bg-primary text-primary-fg shadow-1 hover:bg-primary-hover active:bg-primary-active",
  secondary:
    "border border-border bg-surface text-fg shadow-1 hover:bg-surface-subtle active:bg-surface-muted",
  // Hover and active were the same token, so pressing a ghost button looked like hovering it.
  ghost: "text-fg hover:bg-surface-subtle active:bg-surface-muted",
  // Tokens, not opacity: fading the whole button took `text-danger-fg` down with the background
  // and the label lost its contrast, and `transition-colors` never covered opacity, so danger
  // snapped while the other three faded.
  danger: "bg-danger text-danger-fg shadow-1 hover:bg-danger-hover active:bg-danger-active",
};

const SIZES: Record<ButtonSize, string> = {
  // For a chip bar, where a 32 px button beside a 20 px pill is the odd one out: the assistant's
  // data bar (T-1798) had two hand-made buttons of its own for exactly this. 24 px is still the
  // smallest target WCAG 2.5.8 accepts, which the hand-made ones were not.
  xs: "h-control-xs gap-1 px-2 text-caption",
  sm: "h-control px-2.5 text-caption",
  md: "h-control px-3.5 text-body",
  lg: "h-control-lg px-5 text-body",
};

/** The classes of a button, for a `Link` or an `<a>` that has to look like one. */
export function buttonClass(
  variant: ButtonVariant = "secondary",
  size: ButtonSize = "md",
  className?: string,
): string {
  return clsx(BASE, VARIANTS[variant], SIZES[size], className);
}

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: ButtonVariant;
  size?: ButtonSize;
  /** Drawn before the label; decorative, so pass an `aria-hidden` SVG. */
  icon?: ReactNode;
  /** Shows a spinner and disables the button; the label stays so the width does not jump. */
  loading?: boolean;
  /**
   * Why this button cannot be used, in words (UI-44). Passed *beside* `disabled`, never instead
   * of it: `disabled` is what refuses, the reason is what the refusal says. A reason on its own
   * changes nothing — it is not shown and the button stays live — because a call site may hand
   * down one constant reason for a `disabled` it computes per row (T-2428).
   *
   * A disabled button with a reason is `aria-disabled` rather than `disabled`: it keeps its
   * place in the tab order, so somebody who cannot see that it is greyed out can reach it and be
   * told why.
   * `disabled` would remove it from the tab order and, with `disabled:pointer-events-none`,
   * suppress the tooltip too — two call sites wrote a reason that the browser then threw away.
   * The click is refused either way.
   */
  disabledReason?: string;
}

/** The one button of the Portal: a primary per page, secondaries around it (UI-01). */
export const Button = forwardRef<HTMLButtonElement, ButtonProps>(function Button(
  {
    variant = "secondary",
    size = "md",
    icon,
    loading = false,
    className,
    children,
    type,
    disabled,
    disabledReason,
    onClick,
    title,
    "aria-describedby": describedBy,
    ...rest
  },
  ref,
) {
  const id = useId();
  // `disabled` decides, the reason explains: a reason on its own changes nothing, because call
  // sites pass a constant reason beside a conditional `disabled` (T-2428).
  const explained = Boolean(disabled && disabledReason);
  const button = (
    <button
      ref={ref}
      type={type ?? "button"}
      disabled={explained ? undefined : disabled || loading}
      aria-disabled={explained || undefined}
      // The reason first, then what the caller describes the button with (F-UI1-014).
      aria-describedby={[explained ? id : undefined, describedBy].filter(Boolean).join(" ") || undefined}
      aria-busy={loading || undefined}
      // `preventDefault`, not just "no handler": a refused button that is `type="submit"` still
      // submits the form it stands in, because `aria-disabled` means nothing to the browser.
      // Every schema-driven form of the Portal closes its submit this way, so "proposing is
      // closed" and "your role may not propose" sent the proposal all the same.
      onClick={explained ? (event) => event.preventDefault() : onClick}
      title={explained ? disabledReason : title}
      className={buttonClass(variant, size, className)}
      {...rest}
    >
      {loading ? <Spinner /> : icon}
      {children}
    </button>
  );

  // The reason is a *description*, so it lives beside the button rather than inside it: text
  // inside would join the accessible name, and "New group" would be announced as "New group,
  // disabled: your role does not permit…" every time the list is read.
  return explained ? (
    <>
      {button}
      <span id={id} className="sr-only">
        {disabledReason}
      </span>
    </>
  ) : (
    button
  );
});

function Spinner(): React.JSX.Element {
  return (
    <svg
      aria-hidden="true"
      viewBox="0 0 24 24"
      // `motion-safe:`: a spinner that never stops is what a vestibular disorder reacts to.
      className="size-4 motion-safe:animate-spin"
      fill="none"
      stroke="currentColor"
      strokeWidth="2.5"
      strokeLinecap="round"
    >
      <path d="M12 3a9 9 0 1 0 9 9" />
    </svg>
  );
}
