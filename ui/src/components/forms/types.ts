import type { RJSFSchema, UiSchema } from "@rjsf/utils";

export type { RJSFSchema as JsonSchema, UiSchema };

/**
 * How the form's data and the manifest it writes translate into each other, for the YAML
 * view of a dialog: what the form submits is the manifest the YAML shows, and the YAML the
 * author edits becomes the form again on the way back.
 */
export interface ManifestSource<T> {
  toManifest: (form: T) => unknown;
  fromManifest: (manifest: unknown) => T;
}
