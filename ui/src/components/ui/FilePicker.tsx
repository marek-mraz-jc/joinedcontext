import type { ReactNode } from "react";
import { clsx } from "clsx";

export interface FilePickerProps {
  /** The accessible name of the input: "Choose a CSV file", not "Choose file". */
  label: string;
  /** Which file types the browser's dialog offers, as an `accept` attribute. */
  accept?: string;
  /** The chosen file. The input is cleared first, so the same file can be chosen twice. */
  onFile?: (file: File) => void;
  /** Every chosen file, for an input that takes several (`multiple`). */
  onFiles?: (files: File[]) => void;
  multiple?: boolean;
  /** A tooltip on the trigger, for the icon-only form where the label is not on screen. */
  title?: string;
  className?: string;
  /** What the trigger looks like: a button-shaped span, a dashed drop zone, an icon. */
  children: ReactNode;
}

/**
 * The one file input of the Portal (UI-01, UI-16).
 *
 * A file input cannot be styled and must not be hidden from the keyboard, so every page that
 * takes a file writes the same three lines: an `sr-only` `<input type="file">` inside a
 * `<label>` that carries the visible trigger. Four pages wrote it by hand, and each drifted —
 * one lost its focus ring, another kept the chosen file in the input so picking the same file
 * twice did nothing the second time.
 *
 * The input keeps its place in the tab order and is reached by `Tab` like any other control;
 * the ring is drawn on the label, because that is what a person sees.
 */
export function FilePicker({
  label,
  accept,
  onFile,
  onFiles,
  multiple,
  title,
  className,
  children,
}: FilePickerProps): React.JSX.Element {
  return (
    <label
      title={title}
      className={clsx("focus-ring-within inline-flex cursor-pointer items-center", className)}
    >
      <input
        type="file"
        accept={accept}
        multiple={multiple}
        aria-label={label}
        className="sr-only"
        onChange={(event) => {
          const files = Array.from(event.target.files ?? []);
          // The Files are held; emptying the input's value only empties its FileList, and without
          // it choosing the same file again fires no `change` at all.
          event.target.value = "";
          if (files.length > 0) {
            onFiles?.(files);
            onFile?.(files[0]);
          }
        }}
      />
      {children}
    </label>
  );
}
