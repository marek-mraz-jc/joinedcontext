/**
 * The form arrangements the Portal ships (UI-02, Architecture/09 section 2).
 *
 * One `kind: UiSchema` manifest per kind that has a form, read from the `.uischema.yaml` files
 * beside this one. They are the default: a fresh install shows help and an example on every field
 * before anything is committed to `portal/forms/`, and a manifest in the configuration repository
 * then overrides them field by field. Before them, dev carried one arrangement for one kind and
 * covered three fields of nine, so 80 of the 87 fields a person fills had no help at all (T-1612).
 *
 * The files are read at build time, not fetched: a form dialog opens on a click and its help cannot
 * wait for a round trip.
 */

import { parse as parseYaml } from "yaml";
import type { UiSchemaManifest } from "../../components/forms/uischema";

const documents = import.meta.glob<string>("./*.uischema.yaml", {
  query: "?raw",
  import: "default",
  eager: true,
});

/** Every shipped arrangement, in the order their files are named. */
export const shippedForms: UiSchemaManifest[] = Object.keys(documents)
  .sort()
  .map((path) => parseYaml(documents[path]) as UiSchemaManifest);
