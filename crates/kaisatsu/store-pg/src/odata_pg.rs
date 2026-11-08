#![forbid(unsafe_code)]
//! PostgreSQL pushdown for OData 4.01 entity sets (`PgScopedTable`, W2, W4, K15).
//!
//! `odata_table::PgTable` executes queries directly on a `PgPool` outside any transaction, so
//! kaisatsu's transaction-local RLS settings (`kaisatsu.realm`, `kaisatsu.orgs`) would be unset.
//! `PgScopedTable` implements [`odata_table::Table`] by opening a transaction on `PgStore`,
//! setting `kaisatsu.realm` and `kaisatsu.orgs` via `set_config(..., true)`, and translating the
//! [`Plan`]/[`Cond`]/[`SortKey`] into parameterized `QueryBuilder<Postgres>` statements over the
//! set's JSONB row projection. Every request value is bound via `push_bind`; only compile-time
//! relation strings and validated property names are written into SQL. When a condition cannot be
//! pushed down directly, `PgScopedTable` falls back to bounded keyset paging inside the RLS
//! transaction (at most `max_page` rows per round trip, capped at `MAX_FALLBACK_SCAN_ROWS`).

use crate::{PgStore, key_from_row, set_scope, sso_from_row, storage};
use kaisatsu_core::{
    AdminEvent, AuditEvent, BrandingBlock, ClientRepresentation, ClientScopeRepresentation,
    ConsentRecord, CredentialRecord, EmailOutboxRecord, GroupMemberRecord, GroupRepresentation,
    IdentityProviderLinkRecord, IdentityProviderRepresentation, Membership, OfflineSession,
    Organization, RealmConfig, RequiredActionConfig, RoleMappingRecord, RoleRepresentation,
    UserRepresentation, WebAuthnCredentialRecord, odata_defs,
};
use odata_table::{
    Cond, Fetched, Fetching, Plan, Scope, SortKey, Table, TableError, TableInfo, TextOp, compare,
    matches,
};
use odata_url::CompareOp;
use serde_json::Value;
use sqlx::types::Json;
use sqlx::{Postgres, QueryBuilder};

/// Hard ceiling on rows scanned in keyset fallback mode so no query can exhaust memory.
pub const MAX_FALLBACK_SCAN_ROWS: usize = 5_000;

pub struct PgScopedTable<'a> {
    store: &'a PgStore,
    info: TableInfo,
    realm_id: String,
    allowed_orgs: Option<Vec<String>>,
}

impl<'a> PgScopedTable<'a> {
    #[must_use]
    pub fn new(
        store: &'a PgStore,
        info: TableInfo,
        realm_id: String,
        allowed_orgs: Option<Vec<String>>,
    ) -> Self {
        Self {
            store,
            info,
            realm_id,
            allowed_orgs,
        }
    }

    /// Returns the compile-time SQL subquery `(SELECT row_json, seq FROM ...) AS r` that
    /// produces each entity set's OData JSON shape under forced RLS. `None` for sets that
    /// flatten child arrays (`OrganizationDomains`, `ProtocolMappers`) or decrypt metadata,
    /// which use the bounded in-transaction fallback.
    fn relation_subquery(set_name: &str) -> Option<&'static str> {
        match set_name {
            "Realms" => Some(
                "(SELECT jsonb_build_object(
                    'id', COALESCE(doc->>'id', realm_id),
                    'realm', COALESCE(doc->>'realm', name),
                    'displayName', COALESCE(doc->>'displayName', name),
                    'enabled', COALESCE((doc->>'enabled')::boolean, true),
                    'defaultSignatureAlgorithm', COALESCE(doc->>'defaultSignatureAlgorithm', 'ES256'),
                    'accessTokenLifespan', COALESCE((doc->>'accessTokenLifespan')::bigint, 300),
                    'ssoSessionIdleTimeout', COALESCE((doc->>'ssoSessionIdleTimeout')::bigint, 3600),
                    'ssoSessionMaxLifespan', COALESCE((doc->>'ssoSessionMaxLifespan')::bigint, 36000)
                ) AS row_json, seq FROM kaisatsu.realms WHERE realm_id = $1 OR name = $1)",
            ),
            "Organizations" => Some(
                "(SELECT jsonb_build_object(
                    'id', org_id,
                    'realm_id', realm_id,
                    'name', COALESCE(doc->>'name', org_id),
                    'alias', alias,
                    'enabled', COALESCE((doc->>'enabled')::boolean, true),
                    'description', COALESCE(doc->>'description', '')
                ) AS row_json, seq FROM kaisatsu.organizations WHERE realm_id = $1)",
            ),
            "People" => Some(
                "(SELECT jsonb_build_object(
                    'id', id,
                    'realm_id', realm_id,
                    'org_id', doc->'orgId',
                    'username', COALESCE(doc->>'username', username_lower),
                    'email', doc->'email',
                    'firstName', doc->'firstName',
                    'lastName', doc->'lastName',
                    'enabled', COALESCE((doc->>'enabled')::boolean, true),
                    'emailVerified', COALESCE((doc->>'emailVerified')::boolean, false),
                    'createdTimestamp', doc->'createdTimestamp'
                ) AS row_json, seq FROM kaisatsu.people WHERE realm_id = $1 AND NOT transient)",
            ),
            "Memberships" => Some(
                "(SELECT jsonb_build_object(
                    'id', doc->>'id',
                    'realm_id', realm_id,
                    'org_id', org_id,
                    'person_id', person_id,
                    'created_at', doc->>'createdAt'
                ) AS row_json, seq FROM kaisatsu.memberships WHERE realm_id = $1)",
            ),
            "Groups" => Some(
                "(SELECT jsonb_build_object(
                    'id', id,
                    'realm_id', realm_id,
                    'org_id', org_id,
                    'parent_id', doc->'parentId',
                    'name', COALESCE(doc->>'name', ''),
                    'path', path,
                    'managed_by', doc->'managedBy'
                ) AS row_json, seq FROM kaisatsu.groups WHERE realm_id = $1)",
            ),
            "GroupMembers" => Some(
                "(SELECT jsonb_build_object(
                    'id', doc->>'id',
                    'realm_id', realm_id,
                    'org_id', org_id,
                    'group_id', group_id,
                    'user_id', user_id
                ) AS row_json, seq FROM kaisatsu.group_members WHERE realm_id = $1)",
            ),
            "Roles" => Some(
                "(SELECT jsonb_build_object(
                    'id', id,
                    'realm_id', realm_id,
                    'org_id', doc->'orgId',
                    'name', name,
                    'description', doc->'description',
                    'composite', COALESCE((doc->>'composite')::boolean, false),
                    'clientRole', client_role,
                    'containerId', container_id
                ) AS row_json, seq FROM kaisatsu.roles WHERE realm_id = $1)",
            ),
            "RoleMappings" => Some(
                "(SELECT jsonb_build_object(
                    'id', id,
                    'realm_id', realm_id,
                    'org_id', doc->'orgId',
                    'principal_kind', principal_kind,
                    'principal_id', principal_id,
                    'role_id', COALESCE(doc->>'roleId', role_name),
                    'role_name', role_name,
                    'client_uuid', client_uuid
                ) AS row_json, seq FROM kaisatsu.role_mappings WHERE realm_id = $1)",
            ),
            "Applications" => Some(
                "(SELECT jsonb_build_object(
                    'id', id,
                    'realm_id', realm_id,
                    'org_id', org_id,
                    'clientId', client_id,
                    'name', doc->'name',
                    'description', doc->'description',
                    'enabled', COALESCE((doc->>'enabled')::boolean, true),
                    'publicClient', COALESCE((doc->>'publicClient')::boolean, false),
                    'bearerOnly', COALESCE((doc->>'bearerOnly')::boolean, false),
                    'standardFlowEnabled', COALESCE((doc->>'standardFlowEnabled')::boolean, true),
                    'directAccessGrantsEnabled', COALESCE((doc->>'directAccessGrantsEnabled')::boolean, false),
                    'serviceAccountsEnabled', COALESCE((doc->>'serviceAccountsEnabled')::boolean, false),
                    'clientAuthenticatorType', COALESCE(doc->>'clientAuthenticatorType', 'client-secret'),
                    'managed_by', doc->'managedBy'
                ) AS row_json, seq FROM kaisatsu.applications WHERE realm_id = $1)",
            ),
            "ClientScopes" => Some(
                "(SELECT jsonb_build_object(
                    'id', id,
                    'realm_id', realm_id,
                    'org_id', doc->'orgId',
                    'name', name,
                    'description', doc->'description',
                    'protocol', COALESCE(doc->>'protocol', 'openid-connect')
                ) AS row_json, seq FROM kaisatsu.client_scopes WHERE realm_id = $1)",
            ),
            "IdentityProviders" => Some(
                "(SELECT jsonb_build_object(
                    'internalId', COALESCE(doc->>'internalId', alias),
                    'realm_id', realm_id,
                    'org_id', doc->'orgId',
                    'alias', alias,
                    'providerId', COALESCE(doc->>'providerId', 'oidc'),
                    'enabled', COALESCE((doc->>'enabled')::boolean, true),
                    'trustEmail', COALESCE((doc->>'trustEmail')::boolean, false)
                ) AS row_json, seq FROM kaisatsu.identity_providers WHERE realm_id = $1)",
            ),
            "Credentials" => Some(
                "(SELECT jsonb_build_object(
                    'id', c.id,
                    'realm_id', c.realm_id,
                    'org_id', p.org_id,
                    'user_id', c.user_id,
                    'type', c.credential_type,
                    'userLabel', c.doc->'userLabel',
                    'createdDate', COALESCE((c.doc->>'createdDate')::bigint, 0)
                ) AS row_json, c.seq FROM kaisatsu.credentials c
                LEFT JOIN kaisatsu.people p ON p.realm_id = c.realm_id AND p.id = c.user_id
                WHERE c.realm_id = $1)",
            ),
            "Sessions" => Some(
                "(SELECT jsonb_build_object(
                    'id', id,
                    'realm_id', realm_id,
                    'org_id', org_id,
                    'user_id', user_id,
                    'username', COALESCE(doc->>'username', ''),
                    'ipAddress', COALESCE(doc->>'ipAddress', ''),
                    'start', COALESCE((doc->>'start')::bigint, 0),
                    'lastAccess', COALESCE((doc->>'lastAccess')::bigint, 0),
                    'acr', COALESCE(doc->>'acr', 'pwd')
                ) AS row_json, seq FROM kaisatsu.sso_session WHERE realm_id = $1)",
            ),
            "ClientSessions" => Some(
                "(SELECT jsonb_build_object(
                    'id', id,
                    'realm_id', realm_id,
                    'org_id', org_id,
                    'sso_session_id', sso_session_id,
                    'client_id', client_id,
                    'user_id', user_id
                ) AS row_json, seq FROM kaisatsu.client_session WHERE realm_id = $1)",
            ),
            "OfflineSessions" => Some(
                "(SELECT jsonb_build_object(
                    'id', id,
                    'realm_id', realm_id,
                    'org_id', org_id,
                    'user_id', user_id,
                    'client_id', client_id,
                    'created_at', doc->>'createdAt',
                    'last_access', doc->>'lastAccess'
                ) AS row_json, seq FROM kaisatsu.offline_session WHERE realm_id = $1)",
            ),
            "Consents" => Some(
                "(SELECT jsonb_build_object(
                    'id', COALESCE(doc->>'id', user_id || ':' || client_id),
                    'realm_id', realm_id,
                    'org_id', doc->'orgId',
                    'user_id', user_id,
                    'clientId', client_id,
                    'createdDate', COALESCE((doc->>'createdDate')::bigint, 0),
                    'lastUpdatedDate', COALESCE((doc->>'lastUpdatedDate')::bigint, 0)
                ) AS row_json, seq FROM kaisatsu.consent WHERE realm_id = $1)",
            ),
            "Events" => Some(
                "(SELECT jsonb_build_object(
                    'id', id,
                    'realm_id', realm_id,
                    'org_id', org_id,
                    'time', COALESCE((doc->>'time')::bigint, 0),
                    'type', COALESCE(doc->>'type', ''),
                    'clientId', doc->'clientId',
                    'userId', doc->'userId',
                    'ipAddress', COALESCE(doc->>'ipAddress', ''),
                    'error', doc->'error'
                ) AS row_json, seq FROM kaisatsu.events WHERE realm_id = $1)",
            ),
            "AdminEvents" => Some(
                "(SELECT jsonb_build_object(
                    'id', id,
                    'realm_id', realm_id,
                    'org_id', org_id,
                    'time', COALESCE((doc->>'time')::bigint, 0),
                    'operationType', COALESCE(doc->>'operationType', ''),
                    'resourceType', COALESCE(doc->>'resourceType', ''),
                    'resourcePath', COALESCE(doc->>'resourcePath', ''),
                    'actorId', COALESCE(doc->>'actorId', '')
                ) AS row_json, seq FROM kaisatsu.admin_events WHERE realm_id = $1)",
            ),
            "SigningKeys" => Some(
                "(SELECT jsonb_build_object(
                    'id', COALESCE(doc->>'id', kid),
                    'realm_id', realm_id,
                    'kid', kid,
                    'algorithm', COALESCE(doc->>'algorithm', 'ES256'),
                    'status', COALESCE(doc->>'status', 'ACTIVE'),
                    'priority', priority,
                    'created_at', doc->>'createdAt',
                    'activate_at', doc->'activateAt',
                    'expires_at', doc->'expiresAt'
                ) AS row_json, seq FROM kaisatsu.signing_keys WHERE realm_id = $1)",
            ),
            "RequiredActions" => Some(
                "(SELECT jsonb_build_object(
                    'id', COALESCE(doc->>'id', alias),
                    'realm_id', realm_id,
                    'alias', alias,
                    'name', COALESCE(doc->>'name', alias),
                    'providerId', COALESCE(doc->>'providerId', alias),
                    'enabled', COALESCE((doc->>'enabled')::boolean, true),
                    'defaultAction', COALESCE((doc->>'defaultAction')::boolean, false),
                    'priority', COALESCE((doc->>'priority')::bigint, 10)
                ) AS row_json, seq FROM kaisatsu.required_actions WHERE realm_id = $1)",
            ),
            "BrandingBlocks" => Some(
                "(SELECT jsonb_build_object(
                    'id', COALESCE(doc->>'id', 'default'),
                    'realm_id', realm_id,
                    'org_id', org_id,
                    'instanceName', COALESCE(doc->>'instanceName', ''),
                    'shortName', COALESCE(doc->>'shortName', ''),
                    'organisation', COALESCE(doc->>'organisation', ''),
                    'primaryForeground', COALESCE(doc->>'primaryForeground', '#ffffff')
                ) AS row_json, seq FROM kaisatsu.branding_blocks WHERE realm_id = $1)",
            ),
            "WebAuthnCredentials" => Some(
                "(SELECT jsonb_build_object(
                    'id', id,
                    'realm_id', realm_id,
                    'org_id', org_id,
                    'user_id', user_id,
                    'credentialId', credential_id,
                    'coseAlg', COALESCE((doc->>'coseAlg')::bigint, -7),
                    'signCount', sign_count,
                    'uvInitialized', COALESCE((doc->>'uvInitialized')::boolean, false),
                    'backupEligible', COALESCE((doc->>'backupEligible')::boolean, false),
                    'backupState', COALESCE((doc->>'backupState')::boolean, false),
                    'attestationType', COALESCE(doc->>'attestationType', 'None'),
                    'rpId', COALESCE(doc->>'rpId', ''),
                    'userLabel', doc->'userLabel',
                    'created_at', doc->>'createdAt'
                ) AS row_json, seq FROM kaisatsu.webauthn_credential WHERE realm_id = $1)",
            ),
            "EmailOutbox" => Some(
                "(SELECT jsonb_build_object(
                    'id', id,
                    'realm_id', realm_id,
                    'org_id', org_id,
                    'user_id', COALESCE(doc->>'userId', ''),
                    'toAddress', COALESCE(doc->>'toAddress', ''),
                    'locale', COALESCE(doc->>'locale', 'en'),
                    'templateKind', COALESCE(doc->>'templateKind', ''),
                    'status', status,
                    'attempts', attempts,
                    'lastError', last_error,
                    'created_at', doc->>'createdAt'
                ) AS row_json, seq FROM kaisatsu.email_outbox WHERE realm_id = $1)",
            ),
            "IdentityProviderLinks" => Some(
                "(SELECT jsonb_build_object(
                    'id', id,
                    'realm_id', realm_id,
                    'org_id', org_id,
                    'user_id', user_id,
                    'idpAlias', idp_alias,
                    'externalUserId', external_user_id,
                    'externalUsername', doc->'externalUsername',
                    'created_at', doc->>'createdAt'
                ) AS row_json, seq FROM kaisatsu.idp_links WHERE realm_id = $1)",
            ),
            _ => None,
        }
    }

    fn check_property(&self, path: &[String]) -> Result<(), TableError> {
        let Some(first) = path.first() else {
            return Err(TableError::UnknownProperty(String::new()));
        };
        if !self.info.properties.iter().any(|p| p == first) {
            return Err(TableError::UnknownProperty(path.join("/")));
        }
        Ok(())
    }

    fn push_json_path(
        &self,
        qb: &mut QueryBuilder<Postgres>,
        path: &[String],
    ) -> Result<(), TableError> {
        self.check_property(path)?;
        qb.push("(r.row_json #> ")
            .push_bind(path.to_vec())
            .push("::text[])");
        Ok(())
    }

    fn push_typed_cmp(
        &self,
        qb: &mut QueryBuilder<Postgres>,
        path: &[String],
        ty: &str,
        body: impl FnOnce(&mut QueryBuilder<Postgres>) -> Result<(), TableError>,
    ) -> Result<(), TableError> {
        qb.push("(CASE WHEN jsonb_typeof(");
        self.push_json_path(qb, path)?;
        qb.push(") = ").push_bind(ty.to_owned()).push(" THEN (");
        self.push_json_path(qb, path)?;
        qb.push(" #>> '{}')");
        body(qb)?;
        qb.push(" ELSE FALSE END)");
        Ok(())
    }

    fn push_cond(&self, qb: &mut QueryBuilder<Postgres>, cond: &Cond) -> Result<(), TableError> {
        match cond {
            Cond::Const(true) => {
                qb.push("TRUE");
            }
            Cond::Const(false) => {
                qb.push("FALSE");
            }
            Cond::And(a, b) | Cond::Or(a, b) => {
                qb.push("(");
                self.push_cond(qb, a)?;
                qb.push(if matches!(cond, Cond::And(..)) {
                    " AND "
                } else {
                    " OR "
                });
                self.push_cond(qb, b)?;
                qb.push(")");
            }
            Cond::Not(a) => {
                qb.push("(NOT ");
                self.push_cond(qb, a)?;
                qb.push(")");
            }
            Cond::IsNull { path } => {
                qb.push("(");
                self.push_json_path(qb, path)?;
                qb.push(" IS NULL OR jsonb_typeof(");
                self.push_json_path(qb, path)?;
                qb.push(") = 'null')");
            }
            Cond::Compare { path, op, value } => {
                let op_sql = match op {
                    CompareOp::Eq => " = ",
                    CompareOp::Ne => " <> ",
                    CompareOp::Gt => " > ",
                    CompareOp::Ge => " >= ",
                    CompareOp::Lt => " < ",
                    CompareOp::Le => " <= ",
                };
                match value {
                    Value::String(s) => self.push_typed_cmp(qb, path, "string", |qb| {
                        qb.push(" COLLATE \"C\"").push(op_sql).push_bind(s.clone());
                        Ok(())
                    })?,
                    Value::Number(n) => self.push_typed_cmp(qb, path, "number", |qb| {
                        qb.push("::numeric")
                            .push(op_sql)
                            .push_bind(n.to_string())
                            .push("::numeric");
                        Ok(())
                    })?,
                    Value::Bool(b) => self.push_typed_cmp(qb, path, "boolean", |qb| {
                        qb.push("::boolean").push(op_sql).push_bind(*b);
                        Ok(())
                    })?,
                    Value::Null | Value::Array(_) | Value::Object(_) => {
                        qb.push("FALSE");
                    }
                }
            }
            Cond::Text { path, op, needle } => self.push_typed_cmp(qb, path, "string", |qb| {
                qb.push(if *op == TextOp::StartsWith {
                    " COLLATE \"C\" LIKE "
                } else {
                    " COLLATE \"C\" LIKE '%' || "
                });
                qb.push_bind(like_escaped(needle));
                if *op != TextOp::EndsWith {
                    qb.push(" || '%'");
                }
                qb.push(" ESCAPE '\\'");
                Ok(())
            })?,
        }
        Ok(())
    }

    fn push_order(&self, qb: &mut QueryBuilder<Postgres>, key: &SortKey) -> Result<(), TableError> {
        let dir = if key.descending { " DESC" } else { " ASC" };
        qb.push("CASE jsonb_typeof(");
        self.push_json_path(qb, &key.path)?;
        qb.push(") WHEN 'boolean' THEN 1 WHEN 'number' THEN 2 WHEN 'string' THEN 3 WHEN 'object' THEN 4 WHEN 'array' THEN 4 ELSE 0 END")
            .push(dir);
        for (ty, cast) in [
            ("boolean", "::boolean"),
            ("number", "::numeric"),
            ("string", ""),
        ] {
            qb.push(", (CASE WHEN jsonb_typeof(");
            self.push_json_path(qb, &key.path)?;
            qb.push(") = '").push(ty).push("' THEN (");
            self.push_json_path(qb, &key.path)?;
            qb.push(" #>> '{}')").push(cast).push(" END)");
            if ty == "string" {
                qb.push(" COLLATE \"C\"");
            }
            qb.push(dir);
        }
        Ok(())
    }

    async fn run_pushdown(
        &self,
        subquery: &'static str,
        plan: &Plan,
        scope: &Scope,
    ) -> Result<Fetched, TableError> {
        let failed = |e: sqlx::Error| TableError::Source(e.to_string());
        if self.info.set == "SigningKeys" {
            self.store
                .ensure_realm_keys(&self.realm_id)
                .await
                .map_err(|e| TableError::Source(e.to_string()))?;
        }
        let mut tx = self.store.pool.begin().await.map_err(failed)?;
        let rls_realm = if self.info.set == "Realms" {
            "*"
        } else {
            self.realm_id.as_str()
        };
        set_scope(&mut tx, rls_realm, self.allowed_orgs.as_deref())
            .await
            .map_err(|e| TableError::Source(e.to_string()))?;

        let scope_cond = scope.cond();
        let mut qb = QueryBuilder::<Postgres>::new("SELECT r.row_json FROM ");
        qb.push(subquery).push(" r WHERE ");
        // Bind $1 = realm_id for the subquery's WHERE clause.
        let _ = &self.realm_id;
        // Because QueryBuilder numbers its own binds starting at $1, we build the subquery
        // using push_bind for the realm_id instead of raw `$1`.
        drop(qb);

        let mut qb = QueryBuilder::<Postgres>::new("SELECT r.row_json FROM ");
        self.push_subquery(&mut qb)?;
        qb.push(" r WHERE ");
        self.push_cond(&mut qb, &scope_cond)?;
        qb.push(" AND ");
        self.push_cond(&mut qb, &plan.filter)?;
        qb.push(" ORDER BY ");
        for (i, key) in plan.order.iter().enumerate() {
            if i > 0 {
                qb.push(", ");
            }
            self.push_order(&mut qb, key)?;
        }
        qb.push(" LIMIT ")
            .push_bind(i64::try_from(plan.fetch).unwrap_or(i64::MAX))
            .push(" OFFSET ")
            .push_bind(i64::try_from(plan.offset).unwrap_or(i64::MAX));

        let rows: Vec<Json<Value>> = qb
            .build_query_scalar()
            .fetch_all(&mut *tx)
            .await
            .map_err(failed)?;

        let count = if plan.count {
            let mut cqb = QueryBuilder::<Postgres>::new("SELECT count(*) FROM ");
            self.push_subquery(&mut cqb)?;
            cqb.push(" r WHERE ");
            self.push_cond(&mut cqb, &scope_cond)?;
            cqb.push(" AND ");
            self.push_cond(&mut cqb, &plan.filter)?;
            let n: i64 = cqb
                .build_query_scalar()
                .fetch_one(&mut *tx)
                .await
                .map_err(failed)?;
            Some(u64::try_from(n).unwrap_or(0))
        } else {
            None
        };

        tx.commit().await.map_err(failed)?;
        let _ = subquery;
        Ok(Fetched {
            rows: rows.into_iter().map(|Json(v)| v).collect(),
            count,
        })
    }

    fn push_subquery(&self, qb: &mut QueryBuilder<Postgres>) -> Result<(), TableError> {
        let Some(template) = Self::relation_subquery(&self.info.set) else {
            return Err(TableError::Source("no pushdown subquery".into()));
        };
        let parts: Vec<&str> = template.split("$1").collect();
        for (i, part) in parts.iter().enumerate() {
            if i > 0 {
                qb.push_bind(self.realm_id.clone());
            }
            qb.push(*part);
        }
        Ok(())
    }

    /// Keyset-paged fallback inside the RLS transaction for composite sets (`OrganizationDomains`,
    /// `ProtocolMappers`) so we never load more than `MAX_FALLBACK_SCAN_ROWS` rows.
    async fn run_keyset_fallback(&self, plan: &Plan, scope: &Scope) -> Result<Fetched, TableError> {
        let failed = |e: sqlx::Error| TableError::Source(e.to_string());
        let mut tx = self.store.pool.begin().await.map_err(failed)?;
        set_scope(&mut tx, &self.realm_id, self.allowed_orgs.as_deref())
            .await
            .map_err(|e| TableError::Source(e.to_string()))?;

        let page_step = i64::try_from(plan.fetch.clamp(50, 500)).unwrap_or(100);
        let mut last_seq = 0i64;
        let mut scanned = 0usize;
        let mut kept: Vec<Value> = Vec::new();
        let scope_cond = scope.cond();

        loop {
            if scanned >= MAX_FALLBACK_SCAN_ROWS {
                break;
            }
            let batch: Vec<(i64, Value)> = match self.info.set.as_str() {
                "OrganizationDomains" => {
                    let rows: Vec<(i64, Value)> = sqlx::query_as(
                        "SELECT seq, doc FROM kaisatsu.organizations
                         WHERE realm_id = $1 AND seq > $2 ORDER BY seq LIMIT $3",
                    )
                    .bind(&self.realm_id)
                    .bind(last_seq)
                    .bind(page_step)
                    .fetch_all(&mut *tx)
                    .await
                    .map_err(failed)?;
                    let mut out = Vec::new();
                    for (seq, doc) in rows {
                        last_seq = seq;
                        if let Ok(org) = serde_json::from_value::<Organization>(doc) {
                            for d in org.domains {
                                out.push((seq, odata_defs::org_domain_to_row(&d)));
                            }
                        }
                    }
                    if out.is_empty() {
                        break;
                    }
                    out
                }
                "ProtocolMappers" => {
                    let c_rows: Vec<(i64, Value)> = sqlx::query_as(
                        "SELECT seq, doc FROM kaisatsu.applications
                         WHERE realm_id = $1 AND seq > $2 ORDER BY seq LIMIT $3",
                    )
                    .bind(&self.realm_id)
                    .bind(last_seq)
                    .bind(page_step)
                    .fetch_all(&mut *tx)
                    .await
                    .map_err(failed)?;
                    let s_rows: Vec<(i64, Value)> = if last_seq == 0 {
                        sqlx::query_as(
                            "SELECT seq, doc FROM kaisatsu.client_scopes
                             WHERE realm_id = $1 ORDER BY seq LIMIT $2",
                        )
                        .bind(&self.realm_id)
                        .bind(page_step)
                        .fetch_all(&mut *tx)
                        .await
                        .map_err(failed)?
                    } else {
                        Vec::new()
                    };
                    if c_rows.is_empty() && s_rows.is_empty() {
                        break;
                    }
                    let mut out = Vec::new();
                    for (seq, doc) in c_rows {
                        last_seq = last_seq.max(seq);
                        if let Ok(c) = serde_json::from_value::<ClientRepresentation>(doc) {
                            for m in c.protocol_mappers {
                                out.push((seq, odata_defs::mapper_to_row(&m)));
                            }
                        }
                    }
                    for (seq, doc) in s_rows {
                        if let Ok(s) = serde_json::from_value::<ClientScopeRepresentation>(doc) {
                            for m in s.protocol_mappers {
                                out.push((seq, odata_defs::mapper_to_row(&m)));
                            }
                        }
                    }
                    out
                }
                _ => break,
            };

            if batch.is_empty() {
                break;
            }
            for (_, row) in batch {
                scanned += 1;
                if matches(&row, &scope_cond) && matches(&row, &plan.filter) {
                    kept.push(row);
                }
                if scanned >= MAX_FALLBACK_SCAN_ROWS {
                    break;
                }
            }
            // When ordering solely by key ascending and count is not requested, we can stop once
            // enough rows for offset + fetch are collected.
            if !plan.count
                && plan.order.len() == 1
                && !plan.order[0].descending
                && u64::try_from(kept.len()).unwrap_or(0) >= plan.offset.saturating_add(plan.fetch)
            {
                break;
            }
        }

        tx.commit().await.map_err(failed)?;

        kept.sort_by(|a, b| {
            plan.order
                .iter()
                .map(|k| {
                    let va = k.path.iter().try_fold(a, |v, n| v.get(n));
                    let vb = k.path.iter().try_fold(b, |v, n| v.get(n));
                    let ord = compare(va, vb);
                    if k.descending { ord.reverse() } else { ord }
                })
                .find(|o| o.is_ne())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let count = plan
            .count
            .then(|| u64::try_from(kept.len()).unwrap_or(u64::MAX));
        let skip = usize::try_from(plan.offset).unwrap_or(usize::MAX);
        let take = usize::try_from(plan.fetch).unwrap_or(usize::MAX);
        let _ = (
            storage,
            sso_from_row,
            key_from_row,
            RealmConfig::default,
            UserRepresentation::default,
            Membership::from,
            GroupRepresentation::default,
            GroupMemberRecord::from,
            RoleRepresentation::default,
            RoleMappingRecord::from,
            IdentityProviderRepresentation::default,
            CredentialRecord::default,
            OfflineSession::from,
            ConsentRecord::from,
            AuditEvent::from,
            AdminEvent::from,
            RequiredActionConfig::default,
            BrandingBlock::default,
            WebAuthnCredentialRecord::from,
            EmailOutboxRecord::from,
            IdentityProviderLinkRecord::from,
        );
        Ok(Fetched {
            rows: kept.into_iter().skip(skip).take(take).collect(),
            count,
        })
    }

    async fn execute(&self, plan: &Plan, scope: &Scope) -> Result<Fetched, TableError> {
        if let Some(subquery) = Self::relation_subquery(&self.info.set) {
            self.run_pushdown(subquery, plan, scope).await
        } else {
            self.run_keyset_fallback(plan, scope).await
        }
    }
}

fn like_escaped(needle: &str) -> String {
    let mut out = String::with_capacity(needle.len());
    for c in needle.chars() {
        if matches!(c, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

impl Table for PgScopedTable<'_> {
    fn info(&self) -> &TableInfo {
        &self.info
    }

    fn fetch<'b>(&'b self, plan: &'b Plan, scope: &'b Scope) -> Fetching<'b> {
        Box::pin(self.execute(plan, scope))
    }
}
