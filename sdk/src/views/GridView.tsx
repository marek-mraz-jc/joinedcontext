/**
 * The entity grid as a view of a generated application (SDK-30, UI-71, T-1440).
 *
 * Every other view of a spec draws the rows the app loaded once; this one reads the endpoint
 * itself, because paging, per-column filters, an attribute's history and a correction are the
 * grid's own. The source is always the app's own endpoint — the spec cannot name another one —
 * and the transport is the origin's, so the person's session travels and the Endpoint's Policy
 * decides every read and every write (EP-55, UI-67).
 *
 * In the sandboxed preview frame there is no origin to read from, so the rows the Portal inlined
 * are rendered from memory instead: the same component, the same configuration, no fetch.
 */
import { useMemo } from "react";
import { EntityGrid } from "../grid/EntityGrid";
import { parseGridConfig } from "../grid/config";
import type { EntityGridConfig } from "../grid/config";
import { fixtureSource, sourceFor } from "../grid/source";
import { originTransport } from "../sdk/transport";
import { enumsOf } from "../enums";
import type { Schema } from "../write";

export function GridView({
  slug,
  type,
  config,
  inline,
  schema,
}: {
  /** The app's own endpoint. */
  slug: string;
  /** The entity type of the view's source. */
  type: string;
  config?: Omit<EntityGridConfig, "source" | "type">;
  /** The entities the Portal inlined for a preview, if this is one. */
  inline?: Record<string, unknown>[];
  /** The endpoint's merged schema: an enum attribute is edited and filtered by picking (UI-86). */
  schema?: Schema;
}): React.JSX.Element {
  const parsed = useMemo(
    () => parseGridConfig({ ...(config ?? {}), source: { kind: "endpoint", slug }, type }),
    [config, slug, type],
  );
  const source = useMemo(
    () => (inline ? fixtureSource(inline) : sourceFor({ kind: "endpoint", slug }, originTransport())),
    [inline, slug],
  );

  const enums = useMemo(
    () => (schema ? enumsOf(schema[type], schema, document.documentElement.lang || navigator.language) : undefined),
    [schema, type],
  );

  if (!parsed.config) {
    // A spec that reached the browser with a bad grid says which field is wrong, the way the rest
    // of the specification does; it never renders an empty table instead.
    return (
      <p className="error">
        {parsed.findings.map((finding) => `${finding.path}: ${finding.message}`).join("; ")}
      </p>
    );
  }
  return <EntityGrid config={parsed.config} source={source} enums={enums} />;
}
