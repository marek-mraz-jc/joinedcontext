-- iri_no in PL/pgSQL (T-5243). The LANGUAGE sql body of 0020 has a FROM, so
-- the planner never inlines it, and PostgreSQL 17 plans a SQL function's
-- body again in every statement that calls it: ~0.16 ms a call, 17 calls a
-- keyed history write (pg_stat_user_functions, task T-5243). PL/pgSQL keeps
-- the plan for the session. Same signature, volatility and invoker's
-- rights: RLS on iri_ids still applies, and as a STABLE index condition it
-- is still evaluated once per scan.
CREATE OR REPLACE FUNCTION public.iri_no(p_tenant text, p_iri text) RETURNS bigint
  LANGUAGE plpgsql STABLE PARALLEL SAFE
  AS $$
BEGIN
  RETURN (SELECT id FROM public.iri_ids WHERE tenant_id = p_tenant AND iri = p_iri);
END
$$;
