import { useState } from "react";
import type { JSX } from "react";
import { ExportModal } from "./ExportModal";
import type { ExportTarget } from "./ExportModal";
import { Button } from "../ui";
import type { ButtonSize, ButtonVariant } from "../ui";

/** The button that opens the download modal, wherever a page offers an export (MF-16). */
export function ExportButton({
  project,
  target,
  label,
  className,
  variant = "secondary",
  size = "md",
  open: openedByRow,
  onOpenChange,
  trigger = true,
}: {
  project: string;
  target: ExportTarget;
  label: string;
  className?: string;
  variant?: ButtonVariant;
  size?: ButtonSize;
  /** The row holds the state when the export lives in its menu (T-2279, T-2287). */
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  trigger?: boolean;
}): JSX.Element {
  const [ownOpen, setOwnOpen] = useState(false);
  // The row may open this, and so may the URL (`?edit=`/`?delete=`) or the assistant's hand-off: both
  // are honoured, and closing clears both, so a page opened on one resource still opens its dialog
  // when the row owns the trigger (T-2287).
  const open = ownOpen || (openedByRow ?? false);
  const setOpen = (next: boolean) => {
    setOwnOpen(next);
    onOpenChange?.(next);
  };
  return (
    <>
      {trigger ? (
        <Button variant={variant} size={size} className={className} onClick={() => setOpen(true)}>
          {label}
        </Button>
      ) : null}
      <ExportModal project={project} target={target} open={open} onOpenChange={setOpen} />
    </>
  );
}
