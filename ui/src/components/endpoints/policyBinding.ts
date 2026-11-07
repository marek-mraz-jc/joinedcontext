/**
 * Which `Policy` manifests decide an endpoint's calls (EP-14, GW8, T-2282).
 *
 * The endpoint names its policy as a URN — `urn:ngsi-ld:Policy:{orgDomain}:{spaceSegment}:{name}`
 * — and the page used to show that string and stop there, because nothing in the Portal knew
 * which manifest it stood for. The gateway does know, and its rule is short enough to hold in
 * one function (`context-gateway/src/store.rs`, `bound_policies`):
 *
 * - without `spec.policyRefs` or `spec.policyRef`, every Policy of the endpoint's space applies — except one assigned
 *   to another Endpoint's role (`endpoint:{project}/{name}[/{role}]`): the gateway drops such a
 *   role from every token and gives a caller only the roles of the Endpoint it came through
 *   (`jc_core::kinds::endpoint_role`, AP-96, AP-97), so another App's grant never applies here;
 * - with references, each Policy of that space whose **manifest name** equals a URN's local id,
 *   where that URN's space segment is the endpoint's own space segment;
 * - a URN that matches none binds nothing at all — the endpoint grants nothing — which the page
 *   must say rather than falling back to "every policy of the space" and drawing grants the
 *   gateway would never honour.
 *
 * The space segment is the ContextSpace's `spec.urnSegment` when it pins one, else
 * `{project}-{space}` (`jc_core::kinds::urn_segment`).
 */
import { refName } from "../../api/manifest";
import type { Manifest } from "../../api/manifest";

/** The `{space}` segment of every URN minted for this space. */
export function spaceSegment(project: string, space: string, spaces: Manifest[]): string {
  const manifest = spaces.find((one) => one.metadata.name === space);
  const pinned = (manifest?.spec as { urnSegment?: string } | undefined)?.urnSegment;
  return typeof pinned === "string" && pinned !== "" ? pinned : `${project}-${space}`;
}

export interface PolicyUrn {
  orgDomain: string;
  space: string;
  localId: string;
}

/** `urn:ngsi-ld:Policy:{orgDomain}:{space}:{localId}`, or nothing when it is not one. */
export function parsePolicyUrn(urn: string): PolicyUrn | undefined {
  const parts = urn.split(":");
  // `urn`, `ngsi-ld`, `Policy`, orgDomain, space, localId — a local id never carries a colon.
  if (parts.length !== 6 || parts[0] !== "urn" || parts[1] !== "ngsi-ld" || parts[2] !== "Policy") {
    return undefined;
  }
  const [, , , orgDomain, space, localId] = parts;
  return orgDomain && space && localId ? { orgDomain, space, localId } : undefined;
}

/** What every role an Endpoint gives starts with (`jc_core::kinds::ENDPOINT_ROLE_PREFIX`). */
const ENDPOINT_ROLE_PREFIX = "endpoint:";

/**
 * The grants a Policy holds, each once: `rules[]` when it lists them, else the spec itself, the
 * single-rule shape that means a `rules[]` of one (ADR-N-057).
 */
export function policyRules(policy: Manifest): Record<string, unknown>[] {
  const spec = (policy.spec ?? {}) as Record<string, unknown> & { rules?: unknown };
  return Array.isArray(spec.rules) && spec.rules.length > 0 ? (spec.rules as Record<string, unknown>[]) : [spec];
}

/**
 * Whether a caller of this Endpoint can ever hold a role one of the policy's rules is assigned
 * to: a rule for another Endpoint's role decides nothing here, a policy whose every rule is one
 * decides nothing either (F-UI1-018).
 */
function reachable(policy: Manifest, project: string, endpoint: string): boolean {
  const own = `${ENDPOINT_ROLE_PREFIX}${project}/${endpoint}`;
  return policyRules(policy).some((rule) => {
    const id = (rule.assignee as { id?: unknown } | undefined)?.id;
    return typeof id !== "string" || !id.startsWith(ENDPOINT_ROLE_PREFIX) || id === own || id.startsWith(`${own}/`);
  });
}

export type Binding =
  | { kind: "all"; policies: Manifest[] }
  | { kind: "bound"; policies: Manifest[] }
  /** The reference names no Policy of this space: the endpoint grants nothing. */
  | { kind: "unbound"; urn: string };

/**
 * The policies that decide this endpoint's calls, the way the gateway resolves them.
 *
 * `policies` is the project's Policy list and `spaces` its ContextSpace list; both are what the
 * endpoint page already reads.
 */
export function bindingOf(
  endpoint: Manifest,
  project: string,
  space: string | undefined,
  policies: Manifest[],
  spaces: Manifest[],
): Binding {
  const ofSpace = policies.filter(
    (policy) =>
      (refName(policy.spec.contextSpaceRef) || undefined) === space &&
      reachable(policy, project, endpoint.metadata.name),
  );
  const references = policyRefsOf(endpoint);
  if (references === undefined) {
    return { kind: "all", policies: ofSpace };
  }
  const segment = space === undefined ? undefined : spaceSegment(project, space, spaces);
  const named = references
    .map(parsePolicyUrn)
    .filter((urn) => urn !== undefined && segment !== undefined && urn.space === segment)
    .map((urn) => urn?.localId);
  const bound = ofSpace.filter((policy) => named.includes(policy.metadata.name));
  return bound.length > 0
    ? { kind: "bound", policies: bound }
    : { kind: "unbound", urn: references.join(", ") };
}

/**
 * The Policy URNs an endpoint names: `spec.policyRefs`, or `spec.policyRef` as a list of one
 * (ADR-N-057 decision 2, `EndpointSpec::policy_refs`); nothing when it names none and so answers
 * by every Policy of its space.
 */
export function policyRefsOf(endpoint: Manifest): string[] | undefined {
  const spec = endpoint.spec as { policyRef?: unknown; policyRefs?: unknown };
  // A member present in an unreadable shape narrows to nothing rather than to the whole space.
  if (spec.policyRef !== undefined) {
    return typeof spec.policyRef === "string" ? [spec.policyRef] : [];
  }
  if (spec.policyRefs !== undefined) {
    return Array.isArray(spec.policyRefs)
      ? spec.policyRefs.filter((one): one is string => typeof one === "string")
      : [];
  }
  return undefined;
}
