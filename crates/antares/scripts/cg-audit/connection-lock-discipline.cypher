// cg-audit 5 (T-4307): a DB connection, transaction or async lock held while the same task awaits
// network I/O, the T-4298 outage class. Run: cg q - < scripts/cg-audit/connection-lock-discipline.cypher
// Each row is a candidate; read it with SHOW before it counts.

// 1. One function both takes a hold and calls network or channel I/O itself.
MATCH (f:Symbol)-[:CALLS]->(h:Symbol) WHERE h.external AND NOT f.is_test AND h.path IN ['sqlx_core::pool::Pool::begin', 'sqlx_core::pool::Pool::acquire', 'tokio::sync::mutex::Mutex::lock', 'tokio::sync::rwlock::RwLock::write', 'tokio::sync::rwlock::RwLock::read', 'tokio::sync::semaphore::Semaphore::acquire'] MATCH (f)-[:CALLS]->(n:Symbol) WHERE n.external AND (n.path STARTS WITH 'reqwest::' OR n.path STARTS WITH 'hyper::' OR n.path CONTAINS 'mpsc' OR n.path CONTAINS 'broadcast' OR n.path CONTAINS 'Sink' OR n.path CONTAINS 'WebSocket') RETURN DISTINCT f.path, f.file, f.line, h.name AS holds, collect(DISTINCT n.path) AS io ORDER BY f.file;

// 2. A function that opens a transaction or takes a connection, and reaches network or channel I/O
//    within three hops of its own calls (helpers awaited inside the hold).
MATCH (f:Symbol)-[:CALLS]->(h:Symbol) WHERE h.external AND NOT f.is_test AND h.path IN ['sqlx_core::pool::Pool::begin', 'sqlx_core::pool::Pool::acquire'] MATCH (f)-[:CALLS|DISPATCHES*1..3]->(m:Symbol)-[:CALLS]->(n:Symbol) WHERE NOT m.is_test AND n.external AND (n.path STARTS WITH 'reqwest::' OR n.path STARTS WITH 'hyper::' OR n.path CONTAINS 'mpsc' OR n.path CONTAINS 'broadcast' OR n.path CONTAINS 'WebSocket') RETURN DISTINCT f.path, f.file, f.line, collect(DISTINCT m.path)[0..5] AS via, collect(DISTINCT n.name)[0..5] AS io ORDER BY f.file;

// 3. Row streams (sqlx fetch) and the functions that take them: a stream holds its connection until
//    it is dropped, so a stream returned to a response body is held as long as the client reads.
MATCH (f:Symbol)-[:CALLS]->(s:Symbol) WHERE s.external AND NOT f.is_test AND s.path CONTAINS 'sqlx' AND s.name IN ['fetch', 'fetch_many'] RETURN DISTINCT f.path, f.file, f.line, f.is_async ORDER BY f.file;

// 4. Background loops: who takes a connection or a transaction outside a request handler, with or
//    without a timeout of its own (tokio::time::timeout among its callees).
MATCH (f:Symbol)-[:CALLS]->(h:Symbol) WHERE h.external AND NOT f.is_test AND h.path IN ['sqlx_core::pool::Pool::begin', 'sqlx_core::pool::Pool::acquire'] OPTIONAL MATCH (f)-[:CALLS]->(t:Symbol) WHERE t.external AND t.path = 'tokio::time::timeout::timeout' RETURN f.path, f.file, f.line, count(t) > 0 AS has_timeout ORDER BY f.file;

// 5. Where the pool is built: its size, acquire timeout and idle settings.
GREP PgPoolOptions|max_connections|acquire_timeout|idle_timeout|ANTARES_PG_POOL IN crates/**/*.rs;
