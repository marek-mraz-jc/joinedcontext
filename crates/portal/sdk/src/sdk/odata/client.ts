// The typed client of the Portal's OData service per Organization (ADR-N-070, API/13 §2a, §2b;
// T-6816). Its types are generated from `$metadata` (./generated.ts); this file holds only what a
// generator cannot know: building a query, paging on `@odata.nextLink`, `If-Match` on a bound
// action and reading the OData error object as an API/09 problem.

import { originTransport, type Transport } from "../transport";
import {
  ENTITY_KEYS,
  type Actions,
  type EntitySets,
  type Functions,
  type Singletons,
} from "./generated";

export * from "./generated";

const NAMESPACE = "JoinedContext.Portal";

/** An OData refusal, read as the API/09 problem it carries (`type` is what a caller branches on). */
export class ODataError extends Error {
  readonly status: number;
  /** The problem type URI, e.g. `https://joinedcontext.com/errors/precondition-failed`. */
  readonly type: string;
  /** The problem slug, the OData error's `code`, e.g. `precondition-failed`. */
  readonly code: string;
  readonly title: string;
  /** The current `ETag` a `412` answers, to read the entity again with. */
  readonly etag?: string;

  constructor(status: number, body: unknown, etag?: string) {
    const error = (body as { error?: Record<string, unknown> } | null)?.error ?? {};
    const problem = (body ?? {}) as Record<string, unknown>;
    const text = (v: unknown) => (typeof v === "string" ? v : undefined);
    const message =
      text(error.message) ?? text(problem.detail) ?? text(problem.title) ?? `HTTP ${status}`;
    super(message);
    this.name = "ODataError";
    this.status = status;
    this.code = text(error.code) ?? text(problem.type)?.split("/").pop() ?? "invalid-request";
    this.type = text(error.type) ?? text(problem.type) ?? `https://joinedcontext.com/errors/${this.code}`;
    this.title = text(error.title) ?? text(problem.title) ?? "Error";
    this.etag = etag;
  }

  /** `412`: the entity changed since it was read. */
  get stale(): boolean {
    return this.status === 412;
  }

  /** `428`: a bound write sent no `If-Match`. */
  get preconditionRequired(): boolean {
    return this.status === 428;
  }
}

/** One page of a set: its rows, the count when `$count` was asked, and the next page's address. */
export interface Page<T> {
  value: T[];
  count?: number;
  nextLink?: string;
}

/** A key: one value, or each member of a compound key, e.g. `{ project: "air", name: "pm10" }`. */
export type KeyOf = string | number | Record<string, string | number>;

/** An OData literal of a JavaScript value: text quoted with `''` for a quote. */
export function literal(value: unknown): string {
  if (typeof value === "string") return `'${value.replace(/'/g, "''")}'`;
  if (typeof value === "number" || typeof value === "boolean" || value === null) return String(value);
  return JSON.stringify(value);
}

/** The key predicate of `key`, e.g. `('air')` or `(project='air',name='pm10')`. */
export function keyPredicate(key: KeyOf): string {
  if (typeof key !== "object") return `(${literal(key)})`;
  return `(${Object.entries(key)
    .map(([name, value]) => `${name}=${literal(value)}`)
    .join(",")})`;
}

type Direction = "asc" | "desc";

/** A read of one set, built option by option; nothing is sent until `page` or `all`. */
export class Query<T> {
  private readonly options: Record<string, string> = {};

  constructor(
    private readonly client: ODataClient,
    private readonly path: string,
  ) {}

  /** `$filter`, as OData writes it: `phase eq 'Ready' and project eq 'air'`. */
  filter(expression: string): this {
    this.options.$filter = expression;
    return this;
  }

  select(...members: (keyof T & string)[]): this {
    this.options.$select = members.join(",");
    return this;
  }

  orderBy(member: keyof T & string, direction: Direction = "asc"): this {
    const by = direction === "desc" ? `${member} desc` : member;
    this.options.$orderby = this.options.$orderby ? `${this.options.$orderby},${by}` : by;
    return this;
  }

  top(n: number): this {
    this.options.$top = String(Math.trunc(n));
    return this;
  }

  skip(n: number): this {
    this.options.$skip = String(Math.trunc(n));
    return this;
  }

  /** Asks for `@odata.count`: how many rows the filter holds, over every page. */
  count(): this {
    this.options.$count = "true";
    return this;
  }

  /** `$expand` of a project's navigations, e.g. `expand("endpoints", "spaces")`. */
  expand(...navigations: string[]): this {
    this.options.$expand = navigations.join(",");
    return this;
  }

  /** The address the query reads, relative to the service root. */
  url(): string {
    const query = Object.entries(this.options)
      .map(([name, value]) => `${name}=${encodeURIComponent(value)}`)
      .join("&");
    return query ? `${this.path}?${query}` : this.path;
  }

  /** The first page. */
  page(): Promise<Page<T>> {
    return this.client.page<T>(this.url());
  }

  /** Every row, page after page on `@odata.nextLink`. */
  async *all(): AsyncGenerator<T> {
    let next: string | undefined = this.url();
    while (next) {
      const page: Page<T> = await this.client.page<T>(next);
      yield* page.value;
      next = page.nextLink;
    }
  }
}

export interface ODataOptions {
  /** How requests go out; the browser's same-origin fetch with the CSRF token by default. */
  transport?: Transport;
}

/**
 * The service of one Organization, at its root: `"/api/v1/"` on its own host, `"/bbsk/api/v1/"` on
 * a shared one.
 */
export class ODataClient {
  private readonly root: string;
  private readonly transport: Transport;

  constructor(root: string, options: ODataOptions = {}) {
    this.root = root.endsWith("/") ? root : `${root}/`;
    this.transport = options.transport ?? originTransport();
  }

  /** A read of `set`, e.g. `from("endpoints").filter("project eq 'air'").top(50)`. */
  from<S extends keyof EntitySets & string>(set: S): Query<EntitySets[S]> {
    return new Query<EntitySets[S]>(this, set);
  }

  /** The navigation `nav` of one project, e.g. `under("air", "endpoints")`. */
  under<S extends keyof EntitySets & string>(project: string, nav: S): Query<EntitySets[S]> {
    return new Query<EntitySets[S]>(this, `projects${keyPredicate(project)}/${nav}`);
  }

  /** One entity by its key; its `@odata.etag` is what a bound action sends as `If-Match`. */
  async get<S extends keyof EntitySets & string>(set: S, key: KeyOf): Promise<EntitySets[S]> {
    const members = ENTITY_KEYS[set];
    if (typeof key === "object" && members.some((m) => !(m in key))) {
      throw new ODataError(400, { error: { code: "invalid-request", message: `${set} is keyed by (${members.join(",")})` } });
    }
    const { body } = await this.send("GET", `${set}${keyPredicate(key)}`);
    return body as EntitySets[S];
  }

  /** A singleton, `me` or `Organization`. */
  async singleton<S extends keyof Singletons & string>(name: S): Promise<Singletons[S]> {
    const { body } = await this.send("GET", name);
    return body as Singletons[S];
  }

  /** One page at `address`, relative to the root or a `nextLink` the service answered. */
  async page<T>(address: string): Promise<Page<T>> {
    const { body } = await this.send("GET", address);
    const page = body as { value?: T[]; "@odata.count"?: number; "@odata.nextLink"?: string };
    return { value: page.value ?? [], count: page["@odata.count"], nextLink: page["@odata.nextLink"] };
  }

  /**
   * A bound function of a project, its parameters sent as aliases so a value holding `/` or a
   * JSON object passes: `call("jc_schema_get", "air", { kind: "Endpoint" })`.
   */
  async call<F extends keyof Functions & string>(name: F, project: string, params: Functions[F]): Promise<unknown> {
    const entries = Object.entries(params as Record<string, unknown>).filter(([, v]) => v !== undefined);
    const args = entries.map(([n]) => `${n}=@${n}`).join(",");
    const aliases = entries.map(([n, v]) => `@${n}=${encodeURIComponent(literal(v))}`).join("&");
    const address = `projects${keyPredicate(project)}/${NAMESPACE}.${name}(${args})${aliases ? `?${aliases}` : ""}`;
    const { body } = await this.send("GET", address);
    return (body as { value?: unknown })?.value;
  }

  /**
   * A bound action on the entity at `bound` (`projects('air')`, `spaces(project='air',name='pm10')`),
   * with `ifMatch` the entity's `@odata.etag` where the action changes it: a missing one is a `428`,
   * a stale one a `412` whose `etag` is the current one.
   */
  async act<A extends keyof Actions & string>(
    name: A,
    bound: string,
    params: Actions[A],
    ifMatch?: string,
  ): Promise<unknown> {
    const { body } = await this.send("POST", `${bound}/${NAMESPACE}.${name}`, params, ifMatch);
    return (body as { value?: unknown })?.value ?? body;
  }

  private async send(method: "GET" | "POST", address: string, body?: unknown, ifMatch?: string) {
    const path = this.resolve(address);
    const headers: Record<string, string> = { "OData-Version": "4.01" };
    if (ifMatch) headers["If-Match"] = ifMatch;
    const response = await this.transport({ method, path, body: method === "POST" ? (body ?? {}) : undefined, headers });
    if (response.status < 200 || response.status >= 300) {
      throw new ODataError(response.status, response.body, response.headers?.etag);
    }
    return response;
  }

  /** `address` under the root; a `nextLink` only on the root's own origin and path. */
  private resolve(address: string): string {
    if (!/^[a-z][a-z0-9+.-]*:/i.test(address)) return `${this.root}${address}`;
    const base = typeof location !== "undefined" ? location.href : "http://localhost/";
    const root = new URL(this.root, base);
    const next = new URL(address);
    if (next.origin !== root.origin || !next.pathname.startsWith(root.pathname)) {
      throw new ODataError(400, {
        error: { code: "invalid-request", message: `the next page ${next.origin}${next.pathname} is outside this service` },
      });
    }
    return `${next.pathname}${next.search}`;
  }
}

/** The OData service of one Organization at `root`. */
export function odata(root: string, options?: ODataOptions): ODataClient {
  return new ODataClient(root, options);
}
