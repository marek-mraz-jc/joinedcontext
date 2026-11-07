export interface Grant {
  resource: {
    type: string;
    idPatterns?: string[];
  };
  actions: string[];
  attributes: string[] | "*";
  constraints?: Record<string, unknown>;
}

export interface AccessDocument {
  permissions: Grant[];
  prohibitions: Grant[];
}

export interface Decision {
  ok: boolean;
  reason?: string;
}

export function parseAccess(body: unknown): AccessDocument {
  if (typeof body !== "object" || body === null) {
    return { permissions: [], prohibitions: [] };
  }
  const raw = body as Record<string, unknown>;
  const permissions = Array.isArray(raw.permissions) ? (raw.permissions as Grant[]) : [];
  const prohibitions = Array.isArray(raw.prohibitions) ? (raw.prohibitions as Grant[]) : [];
  return { permissions, prohibitions };
}

function matchesType(grant: Grant, type: string): boolean {
  return grant.resource.type === "*" || grant.resource.type === type;
}

function matchesAction(grant: Grant, action: string): boolean {
  return grant.actions.includes("*") || grant.actions.includes(action);
}

function matchesAttr(grant: Grant, attr?: string): boolean {
  if (attr === undefined) return true;
  if (grant.attributes === "*") return true;
  return Array.isArray(grant.attributes) && grant.attributes.includes(attr);
}

/**
 * Whether the endpoint's access document allows `operation` on `type` (and `attr`). A refusal to a
 * person who holds roles in the application names them (SDK-36), so a control disabled with the
 * reason tells them which role falls short rather than blaming "your role".
 */
export function can(
  access: AccessDocument | null,
  operation: string,
  type: string,
  attr?: string,
  roles?: readonly string[],
): Decision {
  if (access === null) {
    return { ok: false, reason: "Checking your permissions…" };
  }
  const decision = decide(access, operation, type, attr);
  if (decision.ok || roles === undefined || roles.length === 0) {
    return decision;
  }
  const held = roles.length === 1 ? `Your role ${roles[0]} does not` : `Your roles ${roles.join(", ")} do not`;
  const what = attr === undefined ? type : `${attr} of ${type}`;
  return { ok: false, reason: `${held} permit ${operation} on ${what}.` };
}

function decide(access: AccessDocument, operation: string, type: string, attr?: string): Decision {

  for (const prohibition of access.prohibitions) {
    if (matchesType(prohibition, type) && matchesAction(prohibition, operation) && matchesAttr(prohibition, attr)) {
      return { ok: false, reason: `Your role may not ${operation} ${type}.` };
    }
  }

  for (const permission of access.permissions) {
    if (matchesType(permission, type) && matchesAction(permission, operation) && matchesAttr(permission, attr)) {
      return { ok: true };
    }
  }

  if (attr !== undefined) {
    const partial = access.permissions.some(
      (p) => matchesType(p, type) && matchesAction(p, operation) && !matchesAttr(p, attr),
    );
    if (partial) {
      return { ok: false, reason: `Your role may not change ${attr} of ${type}.` };
    }
  }

  return { ok: false, reason: `Your role may not ${operation} ${type}.` };
}
