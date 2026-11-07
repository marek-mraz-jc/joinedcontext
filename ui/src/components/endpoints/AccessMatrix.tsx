/**
 * Who may do what on one Endpoint (API/01 §57, T-5531): every principal its space's Policies
 * name, against each type and read, write, delete and history, from the gateway's own grant
 * documents. A matrix to scan and a graph principal → Policy → Endpoint → type to follow.
 *
 * Display only: the Portal asks the gateway and the server decides who may read this.
 */
import { useId, useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { clsx } from "clsx";
import { api, unwrap } from "../../api/client";
import type { components } from "../../api/schema";
import { CARD_HEIGHT, CARD_WIDTH, curve } from "../../pages/pipelines/flowMap";
import { RecordLink } from "../RecordLink";
import {
  Badge,
  Button,
  EmptyState,
  Field,
  PageFailed,
  Select,
  Skeleton,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeaderCell,
  TableRow,
  TableRowHeaderCell,
  Tabs,
  tabPanelProps,
} from "../ui";

export type Matrix = components["schemas"]["Matrix"];
export type MatrixCell = components["schemas"]["MatrixCell"];

export const MATRIX_ACTIONS = ["read", "write", "delete", "history"] as const;
export type MatrixAction = (typeof MATRIX_ACTIONS)[number];
const KINDS = ["person", "group", "role", "serviceAccount", "public"] as const;

/** `allow` every attribute unconditioned; `partial` some attributes, a constraint or a prohibition in part; `deny` nothing. */
export type CellState = "allow" | "partial" | "deny";

/** What one principal may do on one type for one action, from the cells the server answered. */
export function cellState(cells: MatrixCell[]): CellState {
  const permit = cells.find((cell) => cell.effect === "permit");
  const prohibit = cells.find((cell) => cell.effect === "prohibit");
  if (!permit || (prohibit && !Array.isArray(prohibit.attributes))) return "deny";
  if (prohibit || Array.isArray(permit.attributes) || permit.constraint) return "partial";
  return "allow";
}

const STATE_STYLE: Record<CellState, string> = {
  allow: "border-success/30 bg-success-soft text-success",
  partial: "border-warning/30 bg-warning-soft text-warning",
  deny: "border-border bg-surface-subtle text-fg-muted",
};

export function accessMatrixQuery(project: string, name: string, kind: string) {
  return {
    queryKey: ["access-matrix", project, name, kind] as const,
    queryFn: async ({ signal }: { signal: AbortSignal }): Promise<Matrix> => {
      const answer: unknown = await unwrap(
        await api.GET("/api/v1/projects/{project}/endpoints/{name}/access/matrix", {
          params: { path: { project, name }, query: kind ? { kind } : {} },
          signal,
        }),
      );
      // An answer that is no matrix (a proxy's page, an older Portal) is said as such, never drawn.
      const fields = answer as Partial<Record<keyof Matrix, unknown>> | null;
      if (!fields || !["principals", "types", "cells", "edges"].every((key) => Array.isArray(fields[key as keyof Matrix]))) {
        throw new Error("the Portal answered no access matrix");
      }
      return answer as Matrix;
    },
    retry: false,
  };
}

type View = "matrix" | "graph";

export function AccessMatrix({ project, name }: { project: string; name: string }): JSX.Element {
  const { t } = useTranslation();
  const id = useId();
  const [kind, setKind] = useState("");
  const [view, setView] = useState<View>("matrix");
  const matrix = useQuery(accessMatrixQuery(project, name, kind));

  return (
    <div className="space-y-3">
      <div className="flex flex-wrap items-end gap-3">
        <Field
          id={`${id}-kind`}
          label={t("endpoints.accessMatrix.kind")}
          description={t("endpoints.accessMatrix.kindHint")}
          className="w-full sm:w-64"
        >
          <Select id={`${id}-kind`} value={kind} onChange={(event) => setKind(event.target.value)}>
            <option value="">{t("endpoints.accessMatrix.everyKind")}</option>
            {KINDS.map((option) => (
              <option key={option} value={option}>
                {t(`endpoints.accessMatrix.kinds.${option}`)}
              </option>
            ))}
          </Select>
        </Field>
        <Tabs
          id={`${id}-view`}
          label={t("endpoints.accessMatrix.views")}
          variant="pill"
          value={view}
          onChange={setView}
          tabs={[
            { value: "matrix", label: t("endpoints.accessMatrix.matrix") },
            { value: "graph", label: t("endpoints.accessMatrix.graph") },
          ]}
        />
      </div>
      <div {...tabPanelProps(`${id}-view`, view)}>
        {matrix.isPending ? (
          <div role="status" aria-busy="true" aria-label={t("app.loading")} className="space-y-2">
            <Skeleton className="h-6 w-48" />
            <Skeleton className="h-32" />
          </div>
        ) : matrix.isError ? (
          <PageFailed error={matrix.error} onRetry={() => void matrix.refetch()} />
        ) : matrix.data.principals.length === 0 ? (
          <EmptyState
            bare
            icon="access"
            title={t("endpoints.accessMatrix.empty")}
            description={t("endpoints.accessMatrix.emptyHint")}
          />
        ) : view === "matrix" ? (
          <MatrixTable project={project} matrix={matrix.data} />
        ) : (
          <MatrixGraph matrix={matrix.data} />
        )}
      </div>
    </div>
  );
}

interface Picked {
  principal: string;
  type: string;
  action: MatrixAction;
}

function MatrixTable({ project, matrix }: { project: string; matrix: Matrix }): JSX.Element {
  const { t } = useTranslation();
  const [picked, setPicked] = useState<Picked | null>(null);
  const at = (principal: string, type: string, action: string) =>
    matrix.cells.filter((cell) => cell.principal === principal && cell.type === type && cell.action === action);
  const typeLabel = (type: string) => (type === "*" ? t("endpoints.accessMatrix.everyType") : type);
  const pickedCells = picked ? at(picked.principal, picked.type, picked.action) : [];

  return (
    <div className="space-y-3">
      <Table caption={t("endpoints.accessMatrix.caption", { name: matrix.endpoint })} zebra={false}>
        <TableHead>
          <TableHeaderCell scope="col" className="sticky left-0 z-10 bg-surface">
            {t("endpoints.accessMatrix.who")}
          </TableHeaderCell>
          {matrix.types.flatMap((type) =>
            MATRIX_ACTIONS.map((action) => (
              <TableHeaderCell key={`${type}-${action}`} scope="col">
                <span className="block text-caption text-fg-muted">{typeLabel(type)}</span>
                {t(`endpoints.accessMatrix.action.${action}`)}
              </TableHeaderCell>
            )),
          )}
        </TableHead>
        <TableBody>
          {matrix.principals.map((principal) => {
            const key = `${principal.kind}:${principal.name}`;
            const who = principalLabel(t, principal.kind, principal.name);
            return (
              <TableRow key={key}>
                <TableRowHeaderCell className="sticky left-0 z-10 bg-surface">
                  <span className="block font-medium text-fg">{who}</span>
                  <span className="block text-caption text-fg-muted">
                    {t(`endpoints.accessMatrix.kinds.${principal.kind}`)}
                  </span>
                </TableRowHeaderCell>
                {matrix.types.flatMap((type) =>
                  MATRIX_ACTIONS.map((action) => {
                    const state = cellState(at(key, type, action));
                    const word = t(`endpoints.accessMatrix.state.${state}`);
                    const current =
                      picked?.principal === key && picked.type === type && picked.action === action;
                    return (
                      <TableCell key={`${type}-${action}`}>
                        <Button
                          variant="ghost"
                          size="sm"
                          aria-pressed={current}
                          aria-label={t("endpoints.accessMatrix.cellLabel", {
                            who,
                            action: t(`endpoints.accessMatrix.action.${action}`),
                            type: typeLabel(type),
                            state: word,
                          })}
                          onClick={() => setPicked(current ? null : { principal: key, type, action })}
                          className={clsx("min-w-16 border", STATE_STYLE[state], current && "ring-2 ring-primary")}
                        >
                          {word}
                        </Button>
                      </TableCell>
                    );
                  }),
                )}
              </TableRow>
            );
          })}
        </TableBody>
      </Table>
      {picked ? (
        <CellDetail
          project={project}
          who={principalLabel(t, ...splitKey(picked.principal))}
          type={typeLabel(picked.type)}
          action={picked.action}
          cells={pickedCells}
        />
      ) : (
        <p className="text-caption text-fg-muted">{t("endpoints.accessMatrix.pickHint")}</p>
      )}
    </div>
  );
}

function splitKey(key: string): [string, string] {
  const at = key.indexOf(":");
  return [key.slice(0, at), key.slice(at + 1)];
}

type Translate = (key: string, options?: Record<string, unknown>) => string;

function principalLabel(t: Translate, kind: string, name: string): string {
  return kind === "public" ? t("endpoints.accessMatrix.kinds.public") : name;
}

function CellDetail({
  project,
  who,
  type,
  action,
  cells,
}: {
  project: string;
  who: string;
  type: string;
  action: MatrixAction;
  cells: MatrixCell[];
}): JSX.Element {
  const { t } = useTranslation();
  const state = cellState(cells);
  return (
    <section aria-live="polite" className="space-y-2 rounded-md border border-border p-3">
      <h3 className="font-medium text-fg">
        {t("endpoints.accessMatrix.detailTitle", {
          who,
          action: t(`endpoints.accessMatrix.action.${action}`),
          type,
        })}{" "}
        <Badge tone={state === "allow" ? "success" : state === "partial" ? "warning" : "neutral"}>
          {t(`endpoints.accessMatrix.state.${state}`)}
        </Badge>
      </h3>
      {cells.length === 0 ? (
        <p className="text-body text-fg-muted">{t("endpoints.accessMatrix.noGrant")}</p>
      ) : (
        <ul className="space-y-2">
          {cells.map((cell) => (
            <li key={cell.effect} className="space-y-0.5 text-body">
              <p className="font-medium">
                {t(cell.effect === "permit" ? "endpoints.accessMatrix.permits" : "endpoints.accessMatrix.prohibits")}
              </p>
              <p>
                {Array.isArray(cell.attributes)
                  ? t("endpoints.accessMatrix.attributes", { names: cell.attributes.join(", ") })
                  : t("endpoints.accessMatrix.everyAttribute")}
              </p>
              {cell.constraint ? (
                <p>
                  {t("endpoints.accessMatrix.constraint")} <code className="font-mono text-caption">{cell.constraint}</code>
                </p>
              ) : null}
              {cell.decidedBy ? (
                <p>
                  {t("endpoints.accessMatrix.decidedBy")}{" "}
                  <RecordLink project={project} plural="policies" name={cell.decidedBy}>
                    {cell.decidedBy}
                  </RecordLink>
                </p>
              ) : null}
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

const COLUMN: Record<string, number> = { principal: 0, policy: 1, endpoint: 2, type: 3 };
const GAP_X = 56;
const GAP_Y = 16;

export interface GraphNode {
  id: string;
  column: number;
  label: string;
  x: number;
  y: number;
}

/** The graph's cards: one column per step of principal → Policy → Endpoint → type, in the answer's order. */
export function graphNodes(matrix: Matrix): GraphNode[] {
  const ids: string[] = [];
  for (const edge of matrix.edges) {
    for (const id of [edge.from, edge.to]) if (!ids.includes(id)) ids.push(id);
  }
  const rows = [0, 0, 0, 0];
  return ids.map((id) => {
    const step = id.slice(0, id.indexOf(":"));
    const column = COLUMN[step] ?? 0;
    const row = rows[column]++;
    return {
      id,
      column,
      label: id.slice(id.indexOf(":") + 1),
      x: column * (CARD_WIDTH + GAP_X),
      y: row * (CARD_HEIGHT + GAP_Y),
    };
  });
}

const STEP_WORD = ["principal", "policy", "endpoint", "type"] as const;

function MatrixGraph({ matrix }: { matrix: Matrix }): JSX.Element {
  const { t } = useTranslation();
  const nodes = graphNodes(matrix);
  const byId = new Map(nodes.map((node) => [node.id, node]));
  const width = 4 * CARD_WIDTH + 3 * GAP_X;
  const height = Math.max(...nodes.map((node) => node.y + CARD_HEIGHT), CARD_HEIGHT);
  const label = (node: GraphNode) => {
    if (node.column !== 0) return node.label;
    const [kind, name] = splitKey(node.label);
    return principalLabel(t, kind, name);
  };
  return (
    <div
      role="group"
      tabIndex={0}
      aria-label={t("endpoints.accessMatrix.graphLabel", { name: matrix.endpoint })}
      className="focus-ring overflow-x-auto rounded-md border border-border bg-surface-subtle p-3"
    >
      {/* One drawing: the edges under the cards, each card at its computed place (FlowMapView). */}
      <svg width={width} height={height}>
        <g aria-hidden="true" className="text-border-strong">
          {matrix.edges.map((edge) => {
            const from = byId.get(edge.from);
            const to = byId.get(edge.to);
            return from && to ? (
              <path key={`${edge.from}>${edge.to}`} d={curve(from, to)} fill="none" stroke="currentColor" strokeWidth={1.5} />
            ) : null;
          })}
        </g>
        {nodes.map((node) => (
          <foreignObject key={node.id} x={node.x} y={node.y} width={CARD_WIDTH} height={CARD_HEIGHT}>
            <div className="flex h-full flex-col justify-center rounded-md border border-border bg-surface px-3">
              <span className="text-caption text-fg-muted">{t(`endpoints.accessMatrix.step.${STEP_WORD[node.column]}`)}</span>
              <span className="truncate font-medium text-fg" title={label(node)}>
                {label(node)}
              </span>
              {node.column === 0 ? (
                <span className="sr-only">
                  {t("endpoints.accessMatrix.reaches", {
                    names: matrix.edges
                      .filter((edge) => edge.from === node.id)
                      .map((edge) => edge.to.slice(edge.to.indexOf(":") + 1))
                      .join(", "),
                  })}
                </span>
              ) : null}
            </div>
          </foreignObject>
        ))}
      </svg>
    </div>
  );
}
