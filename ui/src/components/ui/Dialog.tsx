import { useRef } from "react";
import type { ReactNode } from "react";
import * as RadixDialog from "@radix-ui/react-dialog";
import { clsx } from "clsx";
import { Icon } from "./icons";

export type DialogSize = "sm" | "md" | "lg" | "xl";

const SIZES: Record<DialogSize, string> = {
  sm: "w-[min(28rem,92vw)]",
  md: "w-[min(40rem,92vw)]",
  lg: "w-[min(56rem,94vw)] 3xl:w-[min(80rem,90vw)]",
  /** A manifest form: wide enough for two columns of fields and the panels beside them. */
  xl: "w-[min(72rem,94vw)] 3xl:w-[min(110rem,90vw)]",
};

export interface DialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: ReactNode;
  /** One sentence on what the dialog is for; announced with the title. */
  description?: ReactNode;
  size?: DialogSize;
  /** The label of the close control in the corner. */
  closeLabel: string;
  /** A small control right beside the title, such as a form's User Guide link (T-6105). */
  titleAction?: ReactNode;
  /** Rendered under the body, right-aligned: the primary and its cancel. */
  footer?: ReactNode;
  /**
   * Added to the dialog's own frame. It exists so a caller that needs one thing the sizes do not
   * give — a taller body, a fixed width for a diff — can have it and still get the focus trap,
   * the escape key and the labelling; without it such a caller built its own Radix dialog and
   * lost all three.
   */
  className?: string;
  /** Added to the scrolling body, for a caller that needs it flush or padded differently. */
  bodyClassName?: string;
  /**
   * `alertdialog` for a question that must be answered, an "are you sure" (WAI-ARIA): a screen
   * reader announces it as an alert, and a click outside it answers nothing, so it stays open;
   * Escape and its own buttons still close it.
   */
  role?: "dialog" | "alertdialog";
  /** The body; a dialog whose title and description say everything has none, and no empty band. */
  children?: ReactNode;
}

/**
 * The Portal's modal: Radix Dialog for the focus trap, the escape key and the labelling, the
 * tokens for how it looks. The body scrolls inside the dialog, never the page behind it.
 */
export function Dialog({
  open,
  onOpenChange,
  title,
  description,
  size = "md",
  closeLabel,
  titleAction,
  footer,
  className,
  bodyClassName,
  role = "dialog",
  children,
}: DialogProps): React.JSX.Element {
  // The dialog is opened from outside, so Radix has no trigger to hand focus back to and leaves
  // it on the page body. What held focus when it opened gets it back (UI-44, T-1490).
  const opener = useRef<HTMLElement | null>(null);
  return (
    <RadixDialog.Root open={open} onOpenChange={onOpenChange}>
      <RadixDialog.Portal>
        <RadixDialog.Overlay className="fixed inset-0 z-40 bg-overlay backdrop-blur-[2px]" />
        <RadixDialog.Content
          role={role}
          onInteractOutside={role === "alertdialog" ? (event) => event.preventDefault() : undefined}
          onEscapeKeyDown={(event) => {
            // Escape in an open picker closes its list, not the whole form around it (T-4948).
            if (event.target instanceof Element && event.target.closest('[role="combobox"][aria-expanded="true"]')) {
              event.preventDefault();
            }
          }}
          onOpenAutoFocus={(event) => {
            opener.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
            // Radix would take the first tabbable element, which is the Close button in the
            // header. A form starts in its first field; a dialog without one keeps Radix's choice.
            const field = (event.currentTarget as HTMLElement).querySelector<HTMLElement>(
              "input:not([type=hidden]):not(:disabled), select:not(:disabled), textarea:not(:disabled)",
            );
            if (field) {
              event.preventDefault();
              field.focus();
            }
          }}
          onCloseAutoFocus={(event) => {
            if (opener.current?.isConnected) {
              event.preventDefault();
              opener.current.focus();
            }
          }}
          className={clsx(
            "fixed left-1/2 top-1/2 z-50 flex max-h-[88vh] -translate-x-1/2 -translate-y-1/2 flex-col overflow-hidden rounded-xl border border-border bg-surface text-fg shadow-3 focus:outline-none",
            SIZES[size],
            className,
          )}
        >
          <div className="flex items-start justify-between gap-4 border-b border-border px-6 py-4">
            <div className="min-w-0">
              <div className="flex items-center gap-1">
                <RadixDialog.Title className="text-title font-semibold">{title}</RadixDialog.Title>
                {titleAction}
              </div>
              {description ? (
                <RadixDialog.Description className="mt-1 text-body text-fg-muted">
                  {description}
                </RadixDialog.Description>
              ) : null}
            </div>
            <RadixDialog.Close
              aria-label={closeLabel}
              className="focus-ring -mr-2 -mt-1 inline-flex size-control shrink-0 items-center justify-center rounded-md text-fg-muted hover:bg-surface-muted hover:text-fg"
            >
              <Icon name="close" className="size-4" />
            </RadixDialog.Close>
          </div>
          {children != null && children !== false ? (
            <div className={clsx("min-h-0 flex-1 overflow-y-auto px-6 py-5", bodyClassName)}>
              {children}
            </div>
          ) : null}
          {footer ? (
            <div className="flex flex-wrap items-center justify-end gap-2 border-t border-border bg-surface-subtle px-6 py-3">
              {footer}
            </div>
          ) : null}
        </RadixDialog.Content>
      </RadixDialog.Portal>
    </RadixDialog.Root>
  );
}

export const DialogClose = RadixDialog.Close;
