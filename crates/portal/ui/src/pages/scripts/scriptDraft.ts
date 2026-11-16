/**
 * What the Scripts page computes without the network (PL-118…PL-121, T-7084): the next version
 * to suggest, a manifest with one more version, which pipelines pin a script and the pipeline
 * manifests whose pins move. The server judges every one of them again.
 */
import type { Manifest } from "../../api/manifest";

export interface ScriptTest {
  name: string;
  input: unknown;
  expected: unknown;
}

export interface ScriptLimits {
  timeoutMs?: number;
  memoryMb?: number;
}

export interface ScriptVersion {
  version: string;
  state: "draft" | "published";
  code: string;
  sha256: string;
  limits?: ScriptLimits;
  tests?: ScriptTest[];
}

export interface ScriptSpec {
  language: "javascript";
  description?: string;
  versions: ScriptVersion[];
}

/** The largest script source the kind takes, in bytes. */
export const MAX_CODE_BYTES = 64 * 1024;

const SEMVER = /^(0|[1-9]\d{0,8})\.(0|[1-9]\d{0,8})\.(0|[1-9]\d{0,8})$/;

export function parseVersion(version: string): [number, number, number] | null {
  const m = SEMVER.exec(version);
  return m ? [Number(m[1]), Number(m[2]), Number(m[3])] : null;
}

function compare(a: [number, number, number], b: [number, number, number]): number {
  return a[0] - b[0] || a[1] - b[1] || a[2] - b[2];
}

export function specOf(manifest: Manifest | undefined): ScriptSpec | undefined {
  const spec = manifest?.spec as Partial<ScriptSpec> | undefined;
  return spec && Array.isArray(spec.versions) ? (spec as ScriptSpec) : undefined;
}

export function newest(spec: ScriptSpec | undefined): ScriptVersion | undefined {
  return spec?.versions[spec.versions.length - 1];
}

/**
 * The version to suggest: a minor step when some test output changed (a behaviour change), a patch
 * step when every output stayed the same, `1.0.0` for a first version.
 */
export function suggestVersion(spec: ScriptSpec | undefined, outputsChanged: boolean): string {
  const last = parseVersion(newest(spec)?.version ?? "");
  if (!last) return "1.0.0";
  return outputsChanged ? `${last[0]}.${last[1] + 1}.0` : `${last[0]}.${last[1]}.${last[2] + 1}`;
}

/** Why `version` cannot follow the script's versions, or `undefined` when it can. */
export function versionProblem(spec: ScriptSpec | undefined, version: string): "format" | "order" | undefined {
  const parsed = parseVersion(version);
  if (!parsed) return "format";
  const last = parseVersion(newest(spec)?.version ?? "");
  return last && compare(parsed, last) <= 0 ? "order" : undefined;
}

/** The lower-case hex SHA-256 of `code`, as a version's `sha256` holds it. */
export async function digest(code: string): Promise<string> {
  const bytes = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(code));
  return Array.from(new Uint8Array(bytes), (b) => b.toString(16).padStart(2, "0")).join("");
}

/** The Script manifest with `version` appended; a new Script when there is none yet. */
export function withVersion(
  project: string,
  name: string,
  stored: Manifest | undefined,
  version: ScriptVersion,
  description?: string,
): Manifest {
  const spec = specOf(stored);
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Script",
    metadata: { ...(stored?.metadata ?? {}), name, namespace: project },
    spec: {
      language: "javascript",
      ...(description ?? spec?.description ? { description: description ?? spec?.description } : {}),
      versions: [...(spec?.versions ?? []), version],
    },
  } as Manifest;
}

interface Pin {
  name?: string;
  version?: string;
}

type Step = { kind?: string; scriptRef?: Pin };

/** Every pin a pipeline spec holds, steps, route steps and `spec.scripts` alike. */
function pinsOf(spec: Record<string, unknown>): Pin[] {
  const steps = (list: unknown): Pin[] =>
    Array.isArray(list) ? (list as Step[]).flatMap((s) => (s?.scriptRef ? [s.scriptRef] : [])) : [];
  const routes = Array.isArray(spec.routes) ? (spec.routes as { steps?: unknown }[]) : [];
  const applied = Array.isArray(spec.scripts) ? (spec.scripts as { scriptRef?: Pin }[]) : [];
  return [
    ...steps(spec.steps),
    ...routes.flatMap((r) => steps(r.steps)),
    ...applied.flatMap((a) => (a.scriptRef ? [a.scriptRef] : [])),
  ];
}

export interface Use {
  pipeline: string;
  versions: string[];
}

/** The pipelines that pin script `name`, each with the versions it pins. */
export function usedBy(pipelines: Manifest[], name: string): Use[] {
  return pipelines.flatMap((p) => {
    const versions = [
      ...new Set(
        pinsOf((p.spec ?? {}) as Record<string, unknown>)
          .filter((pin) => pin.name === name && typeof pin.version === "string")
          .map((pin) => pin.version as string),
      ),
    ];
    return versions.length > 0 ? [{ pipeline: p.metadata.name, versions }] : [];
  });
}

/** `pipeline` with every pin of script `name` moved to `version`. */
export function movedPins(pipeline: Manifest, name: string, version: string): Manifest {
  const move = (value: unknown): unknown => {
    if (Array.isArray(value)) return value.map(move);
    if (value && typeof value === "object") {
      const entries = Object.entries(value as Record<string, unknown>).map(([key, inner]) => {
        if (key === "scriptRef" && inner && typeof inner === "object" && (inner as Pin).name === name) {
          return [key, { ...(inner as Pin), version }];
        }
        return [key, move(inner)];
      });
      return Object.fromEntries(entries);
    }
    return value;
  };
  return { ...pipeline, spec: move(pipeline.spec) as Manifest["spec"] };
}
