import { useEffect, useRef } from "react";
import type { JSX, ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "./Button";
import { Dialog } from "./Dialog";

export interface ConfirmDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** What is about to happen, as a question naming the thing: "Delete the endpoint ep-bikes?" */
  title: ReactNode;
  /** What it costs, in a sentence: what is lost, what keeps running, what cannot be undone. */
  description?: ReactNode;
  /** The verb on the button, not "OK": "Delete", "Detach", "Cancel the run". */
  confirmLabel: string;
  /** `danger` for anything destroyed; the default for anything else that wants a second look. */
  tone?: "danger" | "primary";
  /** Shown instead of the buttons while the action runs, so it cannot be asked for twice. */
  pending?: boolean;
  onConfirm: () => void;
  children?: ReactNode;
}

/**
 * UI-16, UI-44: the one place the Portal asks "are you sure". It replaces `window.confirm`, which
 * cannot be styled, cannot be translated, names nothing, and blocks the page.
 *
 * Cancel holds the focus when it opens and is the button Enter presses: a dialog that arrives
 * under the pointer must not destroy something because a key was already on its way down. The
 * destructive button says the verb, never "OK", so it reads the same in the dialog as in the menu
 * item that opened it.
 *
 * It is an `alertdialog` (T-3023): announced as a question that needs an answer, and a click
 * beside it does not answer it; Escape, Cancel and the close control do.
 *
 * It is a courtesy, not a control: whatever the person may not do, the server still refuses.
 */
export function ConfirmDialog({
  open,
  onOpenChange,
  title,
  description,
  confirmLabel,
  tone = "danger",
  pending,
  onConfirm,
  children,
}: ConfirmDialogProps): JSX.Element {
  const { t } = useTranslation();
  const cancel = useRef<HTMLButtonElement | null>(null);

  // Radix hands focus to the first tabbable element, which is the close control in the header.
  // Cancel takes it instead, so the focused button is the one that undoes rather than the one
  // that destroys, and Enter on a dialog that arrived under the pointer costs nothing.
  useEffect(() => {
    if (!open) return;
    const frame = requestAnimationFrame(() => cancel.current?.focus());
    return () => cancelAnimationFrame(frame);
  }, [open]);

  return (
    <Dialog
      open={open}
      onOpenChange={onOpenChange}
      title={title}
      description={description}
      size="sm"
      role="alertdialog"
      closeLabel={t("app.close")}
      footer={
        <div className="flex items-center justify-end gap-2">
          <Button
            ref={cancel}
            variant="secondary"
            data-testid="confirm-cancel"
            disabled={pending} disabledReason={t("app.busyReason")}
            onClick={() => onOpenChange(false)}
          >
            {t("app.cancel")}
          </Button>
          <Button
            variant={tone === "danger" ? "danger" : "primary"}
            data-testid="confirm-accept"
            disabled={pending} disabledReason={t("app.busyReason")}
            onClick={onConfirm}
          >
            {pending ? t("app.loading") : confirmLabel}
          </Button>
        </div>
      }
    >
      {children}
    </Dialog>
  );
}
