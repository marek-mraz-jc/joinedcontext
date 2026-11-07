import { forwardRef } from "react";
import type { HTMLAttributes, ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { clsx } from "clsx";
import { Icon } from "./icons";
import type { IconName } from "./icons";

export type AlertTone = "info" | "success" | "warning" | "danger";

/**
 * What each tone looks like, what it is announced as, and how a screen reader hears it.
 *
 * `role` comes from the tone rather than from the caller. Seven of the eighty-two alerts in the
 * Portal were rendered with no role at all, three of them `danger` — an error that is drawn in
 * red and never spoken. The tone already carries the intent, so the component takes it from
 * there; a caller with a different need still passes its own `role` through.
 */
const TONES: Record<AlertTone, { box: string; icon: IconName; role: string; label: string }> = {
  info: { box: "border-info/30 bg-info-soft text-fg", icon: "info", role: "status", label: "app.alert.info" },
  success: { box: "border-success/30 bg-success-soft text-fg", icon: "check", role: "status", label: "app.alert.success" },
  warning: { box: "border-warning/30 bg-warning-soft text-fg", icon: "warning", role: "status", label: "app.alert.warning" },
  danger: { box: "border-danger/30 bg-danger-soft text-fg", icon: "error", role: "alert", label: "app.alert.danger" },
};

export interface AlertProps extends Omit<HTMLAttributes<HTMLDivElement>, "title"> {
  tone?: AlertTone;
  title?: ReactNode;
  /** Buttons or links that answer the message: a retry, a review link. */
  actions?: ReactNode;
}

/**
 * An inline message with the tone in its icon, its words and its role — never in colour alone
 * (UI-15, UI-16, UI-30).
 *
 * The icon is decorative, so the tone reaches a screen reader as a word instead: "Error",
 * "Warning", "Done", "Note", read before the message. `role` follows the tone, and `ref` is
 * forwarded so a form can move the person to the error it just rendered.
 */
export const Alert = forwardRef<HTMLDivElement, AlertProps>(function Alert(
  { tone = "info", title, actions, className, children, role, ...rest },
  ref,
): React.JSX.Element {
  const { t } = useTranslation();
  const { box, icon, role: byTone, label } = TONES[tone];
  return (
    <div
      ref={ref}
      role={role ?? byTone}
      className={clsx("flex gap-3 rounded-lg border px-4 py-3 text-body", box, className)}
      {...rest}
    >
      <Icon
        name={icon}
        className={clsx(
          "mt-0.5 size-4",
          tone === "danger" && "text-danger",
          tone === "warning" && "text-warning",
          tone === "success" && "text-success",
          tone === "info" && "text-info",
        )}
      />
      {/* The eye reads the colour and the glyph; a screen reader is told the tone in a word. */}
      <span className="sr-only">{t(label)}</span>
      <div className="flex min-w-0 flex-1 flex-col gap-1">
        {title ? <p className="font-semibold">{title}</p> : null}
        {/* A URL or an id in an error has no space to break at: it wraps anywhere rather than
            pushing the page sideways on a phone (T-3088). */}
        {children ? <div className="[overflow-wrap:anywhere] [&_a]:underline">{children}</div> : null}
        {actions ? <div className="mt-1 flex flex-wrap items-center gap-2">{actions}</div> : null}
      </div>
    </div>
  );
});
