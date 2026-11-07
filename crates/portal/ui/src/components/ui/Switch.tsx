import { forwardRef } from "react";
import type { ButtonHTMLAttributes } from "react";
import { clsx } from "clsx";

export interface SwitchProps
  extends Omit<ButtonHTMLAttributes<HTMLButtonElement>, "onChange" | "type" | "role"> {
  checked: boolean;
  onCheckedChange: (checked: boolean) => void;
  /** Rendered after the track; the switch is labelled by it. */
  label?: string;
}

/**
 * A `role="switch"` button: on or off, toggled by click, Space and Enter, with the state in
 * `aria-checked` rather than in the colour alone. Radix ships no Switch in this bundle, and a
 * button is all the pattern needs.
 */
export const Switch = forwardRef<HTMLButtonElement, SwitchProps>(function Switch(
  { checked, onCheckedChange, label, className, disabled, id, ...rest },
  ref,
) {
  const track = (
    <button
      ref={ref}
      id={id}
      type="button"
      role="switch"
      aria-checked={checked}
      disabled={disabled}
      onClick={() => onCheckedChange(!checked)}
      className={clsx(
        "focus-ring relative inline-flex h-control-xs w-[2.75rem] shrink-0 items-center rounded-full border border-transparent motion-safe:transition-colors hover:brightness-95 active:brightness-90 disabled:cursor-not-allowed disabled:opacity-50 disabled:hover:brightness-100",
        checked ? "bg-primary" : "bg-neutral-300",
        className,
      )}
      {...rest}
    >
      <span
        aria-hidden="true"
        className={clsx(
          "pointer-events-none block size-[1.25rem] rounded-full bg-neutral-0 shadow-1 motion-safe:transition-transform",
          checked ? "translate-x-[1.25rem]" : "translate-x-[0.125rem]",
        )}
      />
    </button>
  );

  if (!label) {
    return track;
  }
  return (
    <label className="inline-flex cursor-pointer items-center gap-2.5 text-body text-fg">
      {track}
      <span>{label}</span>
    </label>
  );
});
