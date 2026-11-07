//! The database of the server WASM Apps (AP-149, ADR-N-044): for each `wasm` App its id, its
//! shard, its schema `app_<id>`, its owner and run-time roles, and its migrations.
//!
//! The Portal logs in as the database's owner (a CREATEROLE role) and builds no SQL from an App's
//! id: every statement naming one runs inside a function of `jc_apps` that quotes it (migration
//! 0001 of `apps_db/migrations`). An App's migrations run in a login as its own owner role, given
//! a SCRAM verifier for that run only and `NOLOGIN` again after it, so a migration has no role to
//! switch to but its own.

use argon2::password_hash::rand_core::{OsRng, RngCore};
use base64::Engine;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use sqlx::postgres::{PgConnectOptions, PgPool};
use sqlx::{ConnectOptions, Connection};

/// The schema of `jc_apps`, applied when the Portal starts with an apps database.
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./apps_db/migrations");

/// How long an owner may log in for one migration run, minutes.
const OWNER_LOGIN_MINUTES: i32 = 10;
/// How long one migration file may run.
const MIGRATION_TIMEOUT: &str = "60s";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("the apps database: {0}")]
    Db(#[from] sqlx::Error),
    #[error("migration file `{0}`: a name of letters, digits, `.`, `-` and `_` ending in `.sql`")]
    FileName(String),
    #[error(
        "migration `{0}` was applied and has changed since; add a new file instead of editing it"
    )]
    Changed(String),
    #[error("migration `{file}` failed and was rolled back, the schema is as `{last}` left it: {reason}")]
    Failed {
        file: String,
        last: String,
        reason: String,
    },
    #[error("`{0}` is not an App id: sixteen lowercase hex digits")]
    NotAnId(String),
    #[error("the apps database has {0} shards configured; it needs at least one")]
    NoShards(u32),
    #[error("migration `{file}` holds {what}, which an App's migration may not (AP-144): it would run later as the App inside its shard's session, where it could become another App; use tables, indexes, views and constraints")]
    Refused { file: String, what: &'static str },
}

/// The App's id: the first 16 hex digits of the SHA-256 of `{project}/{name}` (AP-149). Stable,
/// a Postgres identifier in every name built from it, and never parsed back into a project.
pub fn app_id(project: &str, name: &str) -> String {
    let digest = Sha256::digest(format!("{project}/{name}").as_bytes());
    hex(&digest[..8])
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// One migration of an App: its file name and its text.
#[derive(Debug, Clone)]
pub struct Migration {
    pub file: String,
    pub sql: String,
}

impl Migration {
    fn sha256(&self) -> String {
        hex(&Sha256::digest(self.sql.as_bytes()))
    }
}

/// What a migration run did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Migrated {
    /// The files applied by this run, in order.
    pub applied: Vec<String>,
}

/// The apps database as the reconciler reaches it.
#[derive(Clone)]
pub struct AppsDb {
    /// The Portal's own login, the database's owner.
    pub admin: PgPool,
    /// Where an owner's login connects: the same server and database, with TLS as configured.
    pub connect: PgConnectOptions,
    /// How many shards the host runs (AP-143).
    pub shards: u32,
}

impl AppsDb {
    /// Applies `jc_apps`'s own schema.
    pub async fn bootstrap(&self) -> Result<(), Error> {
        MIGRATOR.run(&self.admin).await.map_err(sqlx::Error::from)?;
        Ok(())
    }

    /// The App's shard: the one recorded, else the shard with the fewest Apps, recorded now and
    /// kept (AP-149). One reconciler at a time places, under a transaction-level lock.
    pub async fn place(&self, project: &str, name: &str) -> Result<(String, u32), Error> {
        if self.shards == 0 {
            return Err(Error::NoShards(0));
        }
        let id = app_id(project, name);
        let mut tx = self.admin.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtext('jc_apps.place'))")
            .execute(&mut *tx)
            .await?;
        let held: Option<i32> = sqlx::query_scalar("SELECT shard FROM jc_apps.apps WHERE id = $1")
            .bind(&id)
            .fetch_optional(&mut *tx)
            .await?;
        let shard = match held {
            Some(shard) => shard,
            None => {
                let shards =
                    i32::try_from(self.shards).map_err(|_| Error::NoShards(self.shards))?;
                let least: i32 = sqlx::query_scalar(
                    "SELECT s FROM generate_series(0, $1 - 1) AS s \
                     LEFT JOIN jc_apps.apps a ON a.shard = s \
                     GROUP BY s ORDER BY count(a.id), s LIMIT 1",
                )
                .bind(shards)
                .fetch_one(&mut *tx)
                .await?;
                sqlx::query(
                    "INSERT INTO jc_apps.apps (id, project, name, shard) VALUES ($1, $2, $3, $4)",
                )
                .bind(&id)
                .bind(project)
                .bind(name)
                .bind(least)
                .execute(&mut *tx)
                .await?;
                least
            }
        };
        tx.commit().await?;
        Ok((id, u32::try_from(shard).unwrap_or_default()))
    }

    /// The App's roles and schema, its shard's login a member of its run-time role (AP-149).
    pub async fn provision(&self, id: &str, shard: u32) -> Result<(), Error> {
        sqlx::query("SELECT jc_apps.provision($1, $2)")
            .bind(id)
            .bind(format!("wasm_host_{shard}"))
            .execute(&self.admin)
            .await?;
        Ok(())
    }

    /// Runs the files not yet applied, in the order of their names, each in one transaction of
    /// the App's owner login and recorded in it; a recorded file that changed is refused before
    /// anything runs, and a failed file stops the run with the schema as the last good one left
    /// it (AP-149). Running it again with the same files applies nothing.
    pub async fn migrate(&self, id: &str, files: &[Migration]) -> Result<Migrated, Error> {
        if id.len() != 16
            || !id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(Error::NotAnId(id.chars().take(32).collect()));
        }
        let mut files: Vec<&Migration> = files.iter().collect();
        for file in &files {
            let ok = file.file.len() <= 128
                && file.file.ends_with(".sql")
                && file
                    .file
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b));
            if !ok {
                return Err(Error::FileName(file.file.chars().take(64).collect()));
            }
        }
        files.sort_by(|a, b| a.file.cmp(&b.file));
        // Every file is judged before any runs, so a refused one never leaves half a schema.
        for file in &files {
            if let Some(what) = refused_in(&file.sql) {
                return Err(Error::Refused {
                    file: file.file.clone(),
                    what,
                });
            }
        }
        let recorded: Vec<(String, String)> =
            sqlx::query_as("SELECT file, sha256 FROM jc_apps.migrations WHERE app_id = $1")
                .bind(id)
                .fetch_all(&self.admin)
                .await?;
        for file in &files {
            if let Some((_, sha)) = recorded.iter().find(|(name, _)| *name == file.file) {
                if *sha != file.sha256() {
                    return Err(Error::Changed(file.file.clone()));
                }
            }
        }
        let pending: Vec<&Migration> = files
            .into_iter()
            .filter(|file| !recorded.iter().any(|(name, _)| *name == file.file))
            .collect();

        let password = random_password();
        sqlx::query("SELECT jc_apps.open_owner($1, $2, $3)")
            .bind(id)
            .bind(scram_verifier(&password))
            .bind(OWNER_LOGIN_MINUTES)
            .execute(&self.admin)
            .await?;
        let last = recorded
            .iter()
            .map(|(name, _)| name.clone())
            .max()
            .unwrap_or_else(|| "no migration".to_owned());
        let outcome = self.migrate_as_owner(id, &password, &pending, last).await;
        // The login closes whatever the run did; a close that fails is the error worth reporting.
        sqlx::query("SELECT jc_apps.close_owner($1)")
            .bind(id)
            .execute(&self.admin)
            .await?;
        outcome
    }

    async fn migrate_as_owner(
        &self,
        id: &str,
        password: &str,
        pending: &[&Migration],
        mut last: String,
    ) -> Result<Migrated, Error> {
        let owner = format!("app_{id}_owner");
        let mut conn = self
            .connect
            .clone()
            .username(&owner)
            .password(password)
            .disable_statement_logging()
            .connect()
            .await?;
        let schema = format!("app_{id}");
        let mut migrated = Migrated::default();
        for file in pending {
            let mut tx = conn.begin().await?;
            // `SET LOCAL`, not `set_config`: no role of an App may execute it (T-3362). The id is
            // sixteen hex digits, checked at the top of `migrate`, so the name is safe as text.
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
                "SET LOCAL search_path TO {schema}; SET LOCAL statement_timeout = '{MIGRATION_TIMEOUT}'"
            )))
            .execute(&mut *tx)
            .await?;
            // The App's own SQL, run as its own owner and nothing more (AP-149).
            let run = sqlx::raw_sql(sqlx::AssertSqlSafe(file.sql.clone()))
                .execute(&mut *tx)
                .await;
            if let Err(err) = run {
                tx.rollback().await.ok();
                return Err(Error::Failed {
                    file: file.file.clone(),
                    last,
                    reason: database_reason(&err),
                });
            }
            sqlx::query("SELECT jc_apps.record_migration($1, $2)")
                .bind(&file.file)
                .bind(file.sha256())
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
            last = file.file.clone();
            migrated.applied.push(file.file.clone());
        }
        sqlx::query("SELECT jc_apps.owner_grants()")
            .execute(&mut conn)
            .await?;
        conn.close().await.ok();
        Ok(migrated)
    }

    /// The App's recorded shard, `None` for an App never placed.
    pub async fn shard_of(&self, id: &str) -> Result<Option<u32>, Error> {
        let shard: Option<i32> = sqlx::query_scalar("SELECT shard FROM jc_apps.apps WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.admin)
            .await?;
        Ok(shard.and_then(|s| u32::try_from(s).ok()))
    }

    /// The App's schema as files (AP-150): `schema.sql`, each table's columns, constraints and
    /// indexes and each sequence's value, and `<table>.csv` per table with a header. Every name
    /// is quoted by Postgres (`format('%I')`); the App's id reaches no SQL text from here.
    pub async fn export(&self, id: &str) -> Result<Vec<(String, Vec<u8>)>, Error> {
        if id.len() != 16
            || !id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(Error::NotAnId(id.chars().take(32).collect()));
        }
        let schema = format!("app_{id}");
        let mut conn = self.admin.acquire().await?;
        let tables: Vec<String> = sqlx::query_scalar(
            "SELECT c.relname::text FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE n.nspname = $1 AND c.relkind IN ('r', 'p') ORDER BY 1",
        )
        .bind(&schema)
        .fetch_all(&mut *conn)
        .await?;
        let mut ddl: Vec<String> = Vec::new();
        for table in &tables {
            let create: String = sqlx::query_scalar(
                "SELECT format('CREATE TABLE %I.%I (%s);', $1::text, $2::text, string_agg(format('%I %s%s', a.attname, \
                 format_type(a.atttypid, a.atttypmod), CASE WHEN a.attnotnull THEN ' NOT NULL' ELSE '' END), ', ' ORDER BY a.attnum)) \
                 FROM pg_attribute a JOIN pg_class c ON c.oid = a.attrelid JOIN pg_namespace n ON n.oid = c.relnamespace \
                 WHERE n.nspname = $1 AND c.relname = $2 AND a.attnum > 0 AND NOT a.attisdropped",
            )
            .bind(&schema)
            .bind(table)
            .fetch_one(&mut *conn)
            .await?;
            ddl.push(create);
        }
        let constraints: Vec<String> = sqlx::query_scalar(
            "SELECT format('ALTER TABLE %I.%I ADD CONSTRAINT %I %s;', $1::text, c.relname, con.conname, pg_get_constraintdef(con.oid)) \
             FROM pg_constraint con JOIN pg_class c ON c.oid = con.conrelid JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE n.nspname = $1 ORDER BY con.contype = 'f', c.relname, con.conname",
        )
        .bind(&schema)
        .fetch_all(&mut *conn)
        .await?;
        let indexes: Vec<String> = sqlx::query_scalar(
            "SELECT pg_get_indexdef(i.indexrelid) || ';' FROM pg_index i JOIN pg_class c ON c.oid = i.indrelid \
             JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE n.nspname = $1 AND NOT EXISTS (SELECT 1 FROM pg_constraint con WHERE con.conindid = i.indexrelid) ORDER BY 1",
        )
        .bind(&schema)
        .fetch_all(&mut *conn)
        .await?;
        let sequences: Vec<String> = sqlx::query_scalar(
            "SELECT format('SELECT setval(%L, %s);', format('%I.%I', schemaname, sequencename), coalesce(last_value, 1)) \
             FROM pg_sequences WHERE schemaname = $1 ORDER BY sequencename",
        )
        .bind(&schema)
        .fetch_all(&mut *conn)
        .await?;
        let mut files = vec![(
            "schema.sql".to_owned(),
            [ddl, constraints, indexes, sequences]
                .concat()
                .join("\n")
                .into_bytes(),
        )];
        for table in &tables {
            let copy: String = sqlx::query_scalar("SELECT format('COPY %I.%I TO STDOUT WITH (FORMAT csv, HEADER)', $1::text, $2::text)")
                .bind(&schema)
                .bind(table)
                .fetch_one(&mut *conn)
                .await?;
            // The statement is Postgres's own `format('%I')` of names it read from its catalog.
            let mut stream = conn.copy_out_raw(&copy).await?;
            let mut rows: Vec<u8> = Vec::new();
            while let Some(chunk) = futures_util::StreamExt::next(&mut stream).await {
                let chunk = chunk?;
                rows.extend_from_slice(&chunk);
            }
            drop(stream);
            files.push((format!("{table}.csv"), rows));
        }
        Ok(files)
    }

    /// Drops the App's schema, roles and records; the caller has written its exports (AP-150).
    pub async fn retire(&self, id: &str) -> Result<(), Error> {
        sqlx::query("SELECT jc_apps.retire($1)")
            .bind(id)
            .execute(&self.admin)
            .await?;
        Ok(())
    }
}

/// The words of a migration as SQL reads them: names and keywords lowercased, quoted names as
/// names, string constants and comments as nothing; `None` for what it cannot read safely (an
/// unterminated quote or comment, a dollar-quoted body) so the caller refuses it.
fn words(sql: &str) -> Result<Vec<String>, &'static str> {
    let chars: Vec<char> = sql.chars().collect();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if c == '-' && next == Some('-') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && next == Some('*') {
            // Block comments nest in PostgreSQL.
            let mut depth = 0;
            loop {
                match (chars.get(i), chars.get(i + 1)) {
                    (Some('/'), Some('*')) => {
                        depth += 1;
                        i += 2;
                    }
                    (Some('*'), Some('/')) => {
                        depth -= 1;
                        i += 2;
                        if depth == 0 {
                            break;
                        }
                    }
                    (Some(_), _) => i += 1,
                    (None, _) => return Err("an unterminated comment"),
                }
            }
        } else if (c == 'u' || c == 'U')
            && next == Some('&')
            && matches!(chars.get(i + 2), Some('"') | Some('\''))
        {
            // `U&"…"` and `U&'…'` spell a name or a string in escapes (`U&"set\005fconfig"`),
            // which this does not decode: refused, as nothing a migration needs is written so.
            return Err("a Unicode-escaped name or string (U&)");
        } else if c == '\'' {
            // A string constant, `''` an escaped quote; an E'' string's backslash escapes too.
            let escapes = matches!(out.last().map(String::as_str), Some("e"))
                && i > 0
                && matches!(chars[i - 1], 'e' | 'E');
            if escapes {
                out.pop();
            }
            i += 1;
            loop {
                match chars.get(i) {
                    Some('\\') if escapes => i += 2,
                    Some('\'') if chars.get(i + 1) == Some(&'\'') => i += 2,
                    Some('\'') => {
                        i += 1;
                        break;
                    }
                    Some(_) => i += 1,
                    None => return Err("an unterminated string"),
                }
            }
            out.push("'".to_owned());
        } else if c == '"' {
            let mut name = String::new();
            i += 1;
            loop {
                match chars.get(i) {
                    Some('"') if chars.get(i + 1) == Some(&'"') => {
                        name.push('"');
                        i += 2;
                    }
                    Some('"') => {
                        i += 1;
                        break;
                    }
                    Some(&ch) => {
                        name.push(ch);
                        i += 1;
                    }
                    None => return Err("an unterminated quoted name"),
                }
            }
            out.push(name.to_lowercase());
        } else if c == '$' {
            // `$1` is a parameter; `$tag$` or `$$` opens a dollar-quoted body, which this cannot
            // read into: a function's body, a DO block's, or a string a plain quote can carry.
            if next.is_some_and(|ch| ch == '$' || ch.is_alphabetic() || ch == '_') {
                return Err("a dollar-quoted body");
            }
            i += 1;
        } else if c.is_alphanumeric() || c == '_' {
            let start = i;
            while i < chars.len()
                && (chars[i].is_alphanumeric() || chars[i] == '_' || chars[i] == '$')
            {
                i += 1;
            }
            out.push(chars[start..i].iter().collect::<String>().to_lowercase());
        } else {
            if c == ';' || c == '(' || c == ')' {
                out.push(c.to_string());
            }
            i += 1;
        }
    }
    Ok(out)
}

/// What a migration holds that an App's migration may not (AP-144, T-3358), or `None`. A
/// function, procedure, DO block, trigger or rule runs its SQL later as the App inside the shard's
/// session, a member of every App role of the shard, where a role switch the host cannot see
/// makes it another App; `set_config` in a default, a check or an index does the same; and a
/// role switch in the migration itself is refused as well.
pub fn refused_in(sql: &str) -> Option<&'static str> {
    let words = match words(sql) {
        Ok(words) => words,
        Err(what) => return Some(what),
    };
    if words.iter().any(|w| w == "set_config") {
        return Some("`set_config`");
    }
    for statement in words.split(|w| w == ";") {
        let mut lead: Vec<&str> = statement.iter().map(String::as_str).take(6).collect();
        if lead.len() >= 3 && lead[0] == "create" && lead[1] == "or" && lead[2] == "replace" {
            lead.drain(1..3);
        }
        let refused = match lead.as_slice() {
            ["do", ..] => Some("a DO block"),
            ["create", "function", ..] => Some("a function"),
            ["create", "procedure", ..] => Some("a procedure"),
            ["create", "trigger", ..] | ["create", "constraint", "trigger", ..] => {
                Some("a trigger")
            }
            ["create", "event", "trigger", ..] => Some("an event trigger"),
            ["create", "rule", ..] => Some("a rule"),
            ["create", "aggregate", ..] | ["create", "operator", ..] | ["create", "cast", ..] => {
                Some("an aggregate, operator or cast")
            }
            ["set", "role", ..]
            | ["set", "session", ..]
            | ["reset", "role", ..]
            | ["reset", "session", ..] => Some("a role switch"),
            ["set", "local", "role", ..] | ["set", "local", "session", ..] => Some("a role switch"),
            ["call", ..] => Some("a procedure call"),
            _ => None,
        };
        if refused.is_some() {
            return refused;
        }
    }
    None
}

/// The database's own words for a failed statement, without the statement.
fn database_reason(err: &sqlx::Error) -> String {
    match err {
        sqlx::Error::Database(db) => db.message().to_owned(),
        other => other.to_string(),
    }
}

/// 32 random bytes as base64: a password for one migration run.
fn random_password() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    base64::engine::general_purpose::STANDARD_NO_PAD.encode(bytes)
}

/// The SCRAM-SHA-256 verifier Postgres stores for `password` (RFC 5802, RFC 7677), so the
/// password itself never reaches a statement the server could log.
pub fn scram_verifier(password: &str) -> String {
    let mut salt = [0u8; 16];
    OsRng.fill_bytes(&mut salt);
    scram_verifier_with(password, &salt, 4096)
}

fn scram_verifier_with(password: &str, salt: &[u8], iterations: u32) -> String {
    type HmacSha256 = Hmac<Sha256>;
    let mac = |key: &[u8], data: &[u8]| -> [u8; 32] {
        let mut m = HmacSha256::new_from_slice(key).expect("HMAC takes a key of any length");
        m.update(data);
        m.finalize().into_bytes().into()
    };
    // PBKDF2-HMAC-SHA-256 with one block: its output is exactly one hash long.
    let mut first = salt.to_vec();
    first.extend_from_slice(&1u32.to_be_bytes());
    let mut u = mac(password.as_bytes(), &first);
    let mut salted = u;
    for _ in 1..iterations {
        u = mac(password.as_bytes(), &u);
        for (s, x) in salted.iter_mut().zip(u) {
            *s ^= x;
        }
    }
    let client_key = mac(&salted, b"Client Key");
    let stored_key = Sha256::digest(client_key);
    let server_key = mac(&salted, b"Server Key");
    let b64 = base64::engine::general_purpose::STANDARD;
    format!(
        "SCRAM-SHA-256${iterations}:{}${}:{}",
        b64.encode(salt),
        b64.encode(stored_key),
        b64.encode(server_key)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// T-3358, AP-144: what runs later inside the shard's session is refused, however written.
    #[test]
    fn a_migration_defining_code_or_switching_role_is_refused() {
        for (sql, what) in [
            ("CREATE FUNCTION f() RETURNS int LANGUAGE sql AS 'select 1'", "a function"),
            ("create or replace function f() returns int language plpgsql as $$ begin execute 'set role app_x'; end $$", "a dollar-quoted body"),
            ("CREATE   OR\n REPLACE PROCEDURE p() LANGUAGE sql AS 'select 1'", "a procedure"),
            ("DO 'begin null; end'", "a DO block"),
            ("create trigger t before insert on notes for each row execute function f()", "a trigger"),
            ("CREATE CONSTRAINT TRIGGER t AFTER INSERT ON notes FOR EACH ROW EXECUTE FUNCTION f()", "a trigger"),
            ("create event trigger e on ddl_command_start execute function f()", "an event trigger"),
            ("CREATE RULE r AS ON INSERT TO notes DO ALSO NOTIFY x", "a rule"),
            ("set role app_0123456789abcdef", "a role switch"),
            ("SET LOCAL ROLE x", "a role switch"),
            ("SET SESSION AUTHORIZATION x", "a role switch"),
            ("RESET ROLE", "a role switch"),
            ("CALL p()", "a procedure call"),
            ("CREATE TABLE t (r text DEFAULT set_config('role', 'app_x', true))", "`set_config`"),
            ("CREATE TABLE t (x int CHECK (pg_catalog.\"SET_CONFIG\"('role','x',true) IS NOT NULL))", "`set_config`"),
            ("CREATE TABLE t (x int); /* unterminated", "an unterminated comment"),
            ("INSERT INTO t VALUES ('open", "an unterminated string"),
            ("CREATE TABLE t (x text DEFAULT U&\"set\\005fconfig\"('role', 'x', true))", "a Unicode-escaped name or string (U&)"),
            ("CREATE TABLE t (x text DEFAULT u&'\\0041')", "a Unicode-escaped name or string (U&)"),
        ] {
            assert_eq!(refused_in(sql), Some(what), "{sql}");
        }
        // A second statement is judged as the first is.
        assert_eq!(
            refused_in("CREATE TABLE a (x int);\n-- then\nDO 'x';"),
            Some("a DO block")
        );
    }

    #[test]
    fn tables_indexes_views_constraints_and_data_pass() {
        for sql in [
            "CREATE TABLE notes (id bigserial PRIMARY KEY, body text NOT NULL CHECK (length(body) < 4000), created timestamptz DEFAULT now())",
            "CREATE INDEX notes_created ON notes (created DESC); CREATE UNIQUE INDEX ON notes (lower(body))",
            "CREATE VIEW recent AS SELECT * FROM notes WHERE created > now() - interval '1 day'",
            "ALTER TABLE notes ADD COLUMN tags text[] DEFAULT '{}'",
            // Words a refused statement starts with, inside a string, a comment or a name, are data.
            "INSERT INTO notes (body) VALUES ('create function; do; set role x; set_config'), (E'it''s \\' do')",
            "-- create function f\n/* do /* nested */ set role */ CREATE TABLE \"do\" (\"set\" int)",
            "COMMENT ON TABLE notes IS 'Trigger warnings live here'",
        ] {
            assert_eq!(refused_in(sql), None, "{sql}");
        }
    }

    #[test]
    fn an_app_id_is_sixteen_hex_digits_stable_and_per_project() {
        let id = app_id("helsinki", "feedback-box");
        assert_eq!(id.len(), 16);
        assert!(id
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
        assert_eq!(id, app_id("helsinki", "feedback-box"));
        assert_ne!(id, app_id("espoo", "feedback-box"));
        // `{project}/{name}` cannot be forged by moving the slash: a DNS label holds none.
        assert_ne!(app_id("a", "b-c"), app_id("a-b", "c"));
    }

    /// RFC 7677's password and salt, checked against Python's `hashlib.pbkdf2_hmac` and `hmac`,
    /// an implementation of its own; the login test in apps_db_tests.rs checks it against Postgres.
    #[test]
    fn the_verifier_matches_an_independent_implementation() {
        let salt = base64::engine::general_purpose::STANDARD
            .decode("W22ZaJ0SNY7soEsUEjb6gQ==")
            .expect("salt");
        let verifier = scram_verifier_with("pencil", &salt, 4096);
        assert_eq!(
            verifier,
            "SCRAM-SHA-256$4096:W22ZaJ0SNY7soEsUEjb6gQ==$WG5d8oPm3OtcPnkdi4Uo7BkeZkBFzpcXkuLmtbsT4qY=:wfPLwcE6nTWhTAmQ7tl2KeoiWGPlZqQxSrmfPwDl2dU="
        );
        assert_ne!(
            scram_verifier("pencil"),
            scram_verifier("pencil"),
            "a fresh salt each time"
        );
    }
}
