import type { JSX } from "react";
import type { ResourceFormDialogBody, ResourceFormDialogProps } from "./ResourceFormDialogBody";
import { useLoadedWhenOpen } from "./loadWhenOpen";

export type { ManifestSource, ResourceFormDialogProps } from "./ResourceFormDialogBody";

// The form engine, the shipped forms and their widgets load when a create dialog opens, not with
// every page that offers New (T-3316).
const body: { current?: typeof ResourceFormDialogBody } = {};
const load = () => import("./ResourceFormDialogBody").then((module) => module.ResourceFormDialogBody);

function Loaded<T>(props: ResourceFormDialogProps<T>): JSX.Element | null {
  const Dialog = body.current;
  return Dialog ? <Dialog<T> {...props} /> : null;
}

/** A create dialog: the kind's form, rendered once it is open (see ResourceFormDialogBody). */
export function ResourceFormDialog<T>(props: ResourceFormDialogProps<T>): JSX.Element | null {
  const ready = useLoadedWhenOpen(props.open, body, load);
  return props.open && ready ? <Loaded<T> {...props} /> : null;
}
