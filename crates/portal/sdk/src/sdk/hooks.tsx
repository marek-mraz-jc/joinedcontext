import React, { createContext, useCallback, useContext, useEffect, useRef, useState } from "react";
import type { Row, WriteValue } from "../ngsi";
import type { Schema, TypeSchema } from "../write";
import type { AccessDocument, Decision } from "./access";
import { can as evalCan } from "./access";
import type { Client, EndpointOption, Query } from "./client";
import { ProblemError, jc } from "./client";
import type { JcUser } from "./config";

const JcContext = createContext<Client | null>(null);

export function JcProvider({ client, children }: { client?: Client; children: React.ReactNode }): React.JSX.Element {
  const resolved = client ?? jc();
  return <JcContext.Provider value={resolved}>{children}</JcContext.Provider>;
}

export function useClient(): Client {
  const context = useContext(JcContext);
  return context ?? jc();
}

/**
 * The provided client, or `null` where there is none: the shell of an App that reads through its
 * own backend has no served configuration to build one from (SDK-39).
 */
export function useOptionalClient(): Client | null {
  return useContext(JcContext);
}

export interface Loaded {
  loading: boolean;
  error: ProblemError | null;
  reload(): void;
}

export function useEntities<T extends Row = Row>(
  type: string,
  query?: Query,
  options?: { refreshMs?: number; enabled?: boolean; all?: boolean },
): Loaded & { rows: T[] } {
  const client = useClient();
  const enabled = options?.enabled !== false;
  const all = options?.all !== false;
  const refreshMs = options?.refreshMs !== undefined ? Math.max(options.refreshMs, 2000) : undefined;

  const [rows, setRows] = useState<T[]>([]);
  const [loading, setLoading] = useState(enabled);
  const [error, setError] = useState<ProblemError | null>(null);
  const [nonce, setNonce] = useState(0);

  const queryKey = JSON.stringify([type, query, all]);
  const requestId = useRef(0);
  // The query the rows on screen answer; rows of another type or filter are never shown under a new one.
  const shownKey = useRef<string | null>(null);

  const reload = useCallback(() => {
    setNonce((n) => n + 1);
  }, []);

  useEffect(() => {
    if (!enabled) {
      setLoading(false);
      return;
    }

    const id = ++requestId.current;
    if (shownKey.current !== queryKey) {
      setRows([]);
      setLoading(true);
    }
    setError(null);

    const load = async (isPoll: boolean) => {
      try {
        const result = all ? await client.entities.all<T>(type, query) : await client.entities.list<T>(type, query);
        if (requestId.current === id) {
          shownKey.current = queryKey;
          setRows(result);
          setError(null);
        }
      } catch (err) {
        if (requestId.current === id) {
          setError(err instanceof ProblemError ? err : new ProblemError(0, { title: err instanceof Error ? err.message : String(err) }));
        }
      } finally {
        if (requestId.current === id && !isPoll) {
          setLoading(false);
        }
      }
    };

    void load(false);

    if (refreshMs) {
      const timer = setInterval(() => {
        void load(true);
      }, refreshMs);
      return () => {
        clearInterval(timer);
      };
    }
  }, [client, queryKey, enabled, nonce, refreshMs]);

  return { rows, loading, error, reload };
}

export function useEntity<T extends Row = Row>(
  id: string | null,
  attrs?: string[],
): Loaded & { entity: T | null } {
  const client = useClient();
  const [entity, setEntity] = useState<T | null>(null);
  const [loading, setLoading] = useState(id !== null);
  const [error, setError] = useState<ProblemError | null>(null);
  const [nonce, setNonce] = useState(0);

  const key = JSON.stringify([id, attrs]);
  const requestId = useRef(0);

  const reload = useCallback(() => setNonce((n) => n + 1), []);

  useEffect(() => {
    if (!id) {
      setEntity(null);
      setLoading(false);
      setError(null);
      return;
    }

    const currentId = ++requestId.current;
    setLoading(true);
    setError(null);

    void client.entities
      .get<T>(id, attrs)
      .then((res) => {
        if (requestId.current === currentId) {
          setEntity(res);
          setLoading(false);
        }
      })
      .catch((err) => {
        if (requestId.current === currentId) {
          setError(err instanceof ProblemError ? err : new ProblemError(0, { title: err instanceof Error ? err.message : String(err) }));
          setLoading(false);
        }
      });
  }, [client, key, nonce]);

  return { entity, loading, error, reload };
}

export function useSave(): {
  /** `options.endpoint` names the endpoint of a type more than one serves (SDK-02). */
  create(type: string, attrs: Record<string, WriteValue>, localId?: string, options?: EndpointOption): Promise<string | null>;
  update(id: string, patch: Record<string, WriteValue>): Promise<boolean>;
  remove(id: string): Promise<boolean>;
  saving: boolean;
  problem: ProblemError | null;
  clear(): void;
} {
  const client = useClient();
  const [saving, setSaving] = useState(false);
  const [problem, setProblem] = useState<ProblemError | null>(null);

  const clear = useCallback(() => setProblem(null), []);

  const create = async (type: string, attrs: Record<string, WriteValue>, localId?: string, options?: EndpointOption): Promise<string | null> => {
    setSaving(true);
    setProblem(null);
    try {
      const id = await client.entities.create(type, attrs, localId, options);
      return id;
    } catch (err) {
      const p = err instanceof ProblemError ? err : new ProblemError(0, { title: err instanceof Error ? err.message : String(err) });
      setProblem(p);
      return null;
    } finally {
      setSaving(false);
    }
  };

  const update = async (id: string, patch: Record<string, WriteValue>): Promise<boolean> => {
    setSaving(true);
    setProblem(null);
    try {
      await client.entities.update(id, patch);
      return true;
    } catch (err) {
      const p = err instanceof ProblemError ? err : new ProblemError(0, { title: err instanceof Error ? err.message : String(err) });
      setProblem(p);
      return false;
    } finally {
      setSaving(false);
    }
  };

  const remove = async (id: string): Promise<boolean> => {
    setSaving(true);
    setProblem(null);
    try {
      await client.entities.remove(id);
      return true;
    } catch (err) {
      const p = err instanceof ProblemError ? err : new ProblemError(0, { title: err instanceof Error ? err.message : String(err) });
      setProblem(p);
      return false;
    } finally {
      setSaving(false);
    }
  };

  return { create, update, remove, saving, problem, clear };
}

export function useSchema(type?: string): {
  schema: Schema | null;
  typeSchema: TypeSchema | null;
  error: ProblemError | null;
} {
  const client = useClient();
  const [schema, setSchema] = useState<Schema | null>(null);
  const [error, setError] = useState<ProblemError | null>(null);

  useEffect(() => {
    void client
      .schema()
      .then((s) => {
        setSchema(s);
        setError(null);
      })
      .catch((err) => {
        setError(err instanceof ProblemError ? err : new ProblemError(0, { title: err instanceof Error ? err.message : String(err) }));
      });
  }, [client]);

  const typeSchema = type && schema ? (schema[type] ?? null) : null;
  return { schema, typeSchema, error };
}

export function useAccess(endpoint?: string): {
  access: AccessDocument | null;
  error: ProblemError | null;
  can(operation: string, type: string, attr?: string): Decision;
} {
  const client = useClient();
  const [access, setAccess] = useState<AccessDocument | null>(null);
  const [error, setError] = useState<ProblemError | null>(null);

  useEffect(() => {
    void client
      .access(endpoint)
      .then((a) => {
        setAccess(a);
        setError(null);
      })
      .catch((err) => {
        setError(err instanceof ProblemError ? err : new ProblemError(0, { title: err instanceof Error ? err.message : String(err) }));
      });
  }, [client, endpoint]);

  const roles = client.me()?.roles;
  const can = useCallback(
    (operation: string, type: string, attr?: string): Decision => {
      return evalCan(access, operation, type, attr, roles);
    },
    [access, roles],
  );

  return { access, error, can };
}

export function useMe(): JcUser | null {
  const client = useClient();
  return client.me();
}

export function useFunction<T = unknown>(
  name: string,
  body?: unknown,
  options?: { enabled?: boolean },
): Loaded & { data: T | null } {
  const client = useClient();
  const enabled = options?.enabled !== false;

  const [data, setData] = useState<T | null>(null);
  const [loading, setLoading] = useState(enabled);
  const [error, setError] = useState<ProblemError | null>(null);
  const [nonce, setNonce] = useState(0);

  const key = JSON.stringify([name, body]);
  const requestId = useRef(0);

  const reload = useCallback(() => setNonce((n) => n + 1), []);

  useEffect(() => {
    if (!enabled) {
      setLoading(false);
      return;
    }

    const currentId = ++requestId.current;
    setLoading(true);
    setError(null);

    void client.functions
      .call<T>(name, body)
      .then((res) => {
        if (requestId.current === currentId) {
          setData(res);
          setLoading(false);
        }
      })
      .catch((err) => {
        if (requestId.current === currentId) {
          setError(err instanceof ProblemError ? err : new ProblemError(0, { title: err instanceof Error ? err.message : String(err) }));
          setLoading(false);
        }
      });
  }, [client, key, enabled, nonce]);

  return { data, loading, error, reload };
}
