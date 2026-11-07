import { useMemo } from "react";
import { displayName, endpointSource, parseGridConfig, projectRow, useClient, useSchema } from "@joinedcontext/sdk";
import type { GridColumn, RichRow } from "@joinedcontext/sdk";
import { EntityGrid } from "@joinedcontext/sdk/grid";
import { columnsOf, shapeOf, spokenName } from "./shape";
import { Problem } from "./states";

export interface TypeGridProps {
  type: string;
  /** Where the type is read, in an application reading several (SDK-02). */
  endpoint?: string;
  /** Without them the type's model picks: its name, measures, categories and time. */
  columns?: GridColumn[];
  /** Keeps only these rows, an NGSI-LD query: `availableBikeNumber==0`. */
  q?: string;
  pageSize?: number;
  /** Opens a row, by its name in the first column. */
  onSelect?: (id: string) => void;
}

/**
 * One type in the SDK's `EntityGrid`, read, paged and filtered at the endpoint. A column: `{ attr }`
 * or `{ attr, formula }`. Each row leads with its name, so a phone sees a value beside it.
 */
export function TypeGrid({ type, endpoint, columns, q, pageSize = 50, onSelect }: TypeGridProps) {
  const { config, transport } = useClient();
  const { typeSchema } = useSchema(type);
  const slug = config.endpoints?.find((each) => each.name === endpoint)?.slug ?? config.slug;
  const { language } = config;
  const source = useMemo(
    () => endpointSource(slug, transport, language),
    [slug, transport, language],
  );
  // ponytail: an endpoint with no compiled model gets the grid's own columns, geometry and references too.
  const shape = useMemo(() => shapeOf(typeSchema, []), [typeSchema]);
  const nameAttr = shape.label;
  // The name leads the row, sorted and filtered like any column; the id and type say nothing here.
  const listed = useMemo(() => {
    const asked = columns ?? columnsOf(shape);
    if (!nameAttr) return asked;
    const name = asked.find((each) => each.attr === nameAttr) ?? { attr: nameAttr, label: spokenName(nameAttr) };
    return [{ ...name, pinned: true }, ...asked.filter((each) => each.attr !== nameAttr)];
  }, [columns, shape, nameAttr]);
  const { config: grid, findings } = useMemo(
    () => parseGridConfig({ source: { kind: "endpoint", slug }, type, pageSize, ...(listed.length > 0 ? { columns: listed } : {}), ...(q ? { filters: { preset: { q } } } : {}) }),
    [slug, type, pageSize, listed, q],
  );
  const hidden = useMemo(() => (nameAttr ? ["id", "entity:type"] : ["entity:type"]), [nameAttr]);
  const renderers = useMemo(
    () =>
      onSelect && {
        [nameAttr ?? "id"]: (_cell: unknown, row: RichRow) => {
          const projected = projectRow(row, language);
          return (
            <button type="button" className="jc-open" onClick={() => onSelect(row.id)}>
              {String((nameAttr && projected[nameAttr]) || displayName(projected))}
            </button>
          );
        },
      },
    [onSelect, language, nameAttr],
  );
  if (!grid) return <Problem error={new Error(findings.map((each) => `${each.path}: ${each.message}`).join("; "))} />;
  return (
    <div className="jc-type-grid">
      <EntityGrid config={grid} source={source} renderers={renderers || undefined} hidden={hidden} filterRow={false} queryBox={false} />
    </div>
  );
}
