# minted-cleanup

Removes the 244,837 entities jc-prod's pipelines minted under
`urn:ngsi-ld:<Type>:portal.smartcity.joinedcontext.com:` on 2026-11-10 11:49:54-12:29:26Z, when the
org domain was unset (T-6676 section R, fixed in deployment e4dd3cde). The copies go through each
space's Endpoint as a temporary service account holding one temporary Policy per space: deleteBatch
only, one selector per minted type, id pattern `^urn:ngsi-ld:<Type>:portal\.smartcity\.joinedcontext\.com:`,
valid for a few hours. No raw SQL delete, no import window. `minted_cleanup.py --help` and the module
docstring describe the checks; `python3 -m unittest discover -s bins/jc/deploy/minted-cleanup` runs
the selftests against a stub gateway (ci.yml runs them in `changes`).

## 1. Inputs (read-only, by the deployer)

Work in a 0700 directory `W`. All SQL runs in `BEGIN READ ONLY` against `jc_antares`.

`W/list.tsv`: tenant, id, types, expires_at, the minted ids the document names.

```sql
\copy (SELECT tenant_id, id, array_to_string(types, ' '),
  coalesce(to_char(expires_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"'), ''),
  coalesce((SELECT string_agg(DISTINCT m[1], ' ')
            FROM regexp_matches(entity::text, 'urn:ngsi-ld:[A-Za-z]+:portal\.smartcity\.joinedcontext\.com:[^"\\[:space:]]+', 'g') AS m
            WHERE m[1] <> id), '')
  FROM entities WHERE split_part(id, ':', 4) = 'portal.smartcity.joinedcontext.com'
  ORDER BY tenant_id, id) TO 'W/list.tsv'
```

Its line count matches the counts per tenant (`evidence/jc-prod/minted-prefix-counts-1230Z.tsv` at
12:30Z; 244,837 at the end).

`W/endpoints.tsv`: tenant, Endpoint slug, the space that Endpoint names (spec.contextSpaceRef, read
back from the Portal, not copied from this table), project. Use the space-wide Endpoint without a
`policyRef`: an Endpoint whose view leaves a type out answers its ids as absent, and the tool stops
on that.

```text
helsinki	si6epqkx364lprho5uaigutk274r5grb	helsinki	helsinki       # helsinki-all
praha-mesto	jw7tffshzip7igmzcszgffkkkuetnc6v	praha-mesto	praha    # praha-mesto
barcelona	pbyqbuyoeikryocvqb5uev7nxbx5mr4a	barcelona	barcelona # barcelona
bratislava	rbamgl4sqke6y5gws3mdgj3qe2zhw7ev	bratislava	bratislava # bratislava
```

(slugs from the frozen export prod-20261109T1647Z; check each on jc-prod; tabs, no comments in the file).

## 2. Runbook

1. Account, per affected project (helsinki, praha, barcelona, bratislava), proposed and approved as
   a Change in the Portal (`POST /api/v1/projects/<p>/serviceaccounts`, `?dryRun=All` first):
   `ServiceAccount cleanup-t6676`, owner the deployer's steward, purpose "temporary: deletes the
   T-6676 R copies; removed after the run", `roles: []`, `credentials: [{kind: oauth-client, name: main}]`.
   The gateway resolves an account by its manifest (client `<project>-cleanup-t6676`); a bare realm
   client without the manifest gets no subject. Issue the secret once
   (`POST .../serviceaccounts/cleanup-t6676/secret`) into a 0600 file under W; never in argv.
2. Policies: `minted_cleanup.py policies --list W/list.tsv --endpoints W/endpoints.tsv --state W/st --until <now+6h UTC>`
   writes `W/st/policy-<space>.json`; propose and approve each as a Change
   (`POST /api/v1/projects/<p>/policies`, `?dryRun=All` first). Read each back: operations
   `[deleteBatch]`, assignee `serviceAccount cleanup-t6676`, the types of that space only.
3. Dry run: `minted_cleanup.py run --list W/list.tsv --endpoints W/endpoints.tsv --state W/st`.
   It checks every row and prints listed, layers and batches per space; nothing is sent.
4. Token: a client-credentials token of `<project>-cleanup-t6676` per project into `W/token-<space>`
   (0600), refreshed by a loop that rewrites the file; the tool rereads it per batch.
5. Apply, one space at a time (one token per project):
   `minted_cleanup.py run --list W/list.tsv --endpoints W/endpoints.tsv --state W/st --only helsinki --apply --gateway https://<jc-prod portal host> --token-file W/token-helsinki --mem-cmd '<command printing the node memory %>'`
   (for instance `KUBECONFIG=... kubectl top node --no-headers | awk "{print \$5}"`). A stop prints
   why on its last line; fix it and rerun the same command: it resumes from `W/st/done.txt`.
6. Recount (read-only):
   `W/recount.tsv`: `SELECT tenant_id, id, <expires_at as in step 1> FROM entities WHERE split_part(id,':',4)='portal.smartcity.joinedcontext.com'`;
   `W/history.tsv`: per id under the prefix, `1` per temporal_entities row and the attr_instances count
   (`SELECT coalesce(t.tenant_id,a.tenant_id), coalesce(t.id,a.entity_id), (t.id IS NOT NULL)::int, coalesce(a.n,0) FROM (SELECT tenant_id,id FROM temporal_entities WHERE split_part(id,':',4)='portal.smartcity.joinedcontext.com') t FULL JOIN (SELECT tenant_id,entity_id,count(*) n FROM attr_instances WHERE split_part(entity_id,':',4)='portal.smartcity.joinedcontext.com' GROUP BY 1,2) a ON a.tenant_id=t.tenant_id AND a.entity_id=t.id`).
   Then `minted_cleanup.py verify --recount W/recount.tsv --history W/history.tsv --state W/st`:
   0 non-expired copies left passes; `history-for-sweep.tsv` lists expired copies whose history
   waits for the broker's expiry sweep (not forced); `history-left-after-delete.tsv` lists deleted
   ids whose history stayed (the broker's history delete is best effort), for a decision.
7. Remove the grant: propose and approve the deletion of each `Policy cleanup-t6676`, then of each
   `ServiceAccount cleanup-t6676` (its Keycloak client goes with it). Read back: GET of each answers
   404, `GET /api/v1/projects/<p>/policies?q=cleanup-t6676` lists none, and the realm has no
   `*-cleanup-t6676` client. `validity.to` ends the grant by itself if this step is late.
8. Evidence: counts (`W/st/log.txt`, the verify lines) and the delete list only; no entity values,
   no token, no secret.
