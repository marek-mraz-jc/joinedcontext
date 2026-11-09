//! What the tests of the Portal's own build lane share: a database of each test's own, a folder
//! for the uploads, and the lane's settings (AP-130, jc build lane).

use joinedcontext_portal::apps::build_pods::Settings;
use joinedcontext_portal::apps::build_records;
use sqlx::PgPool;

/// A database of this test's own, migrated; `None` without `JC_PORTAL_TEST_DATABASE_URL`.
pub async fn fresh_db() -> Option<PgPool> {
    let url = std::env::var("JC_PORTAL_TEST_DATABASE_URL")
        .ok()
        .filter(|url| !url.trim().is_empty())?;
    let admin = PgPool::connect(&url).await.expect("the test database");
    let name = format!(
        "build_lane_{}",
        build_records::new_token().get(..16).unwrap_or("x")
    );
    sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE {name}")))
        .execute(&admin)
        .await
        .expect("a database of this test's own");
    let mut own = url::Url::parse(&url).expect("a URL");
    own.set_path(&name);
    Some(
        joinedcontext_portal::db::connect(own.as_str())
            .await
            .expect("connect + migrate"),
    )
}

/// The test's own database, or the test returns early saying it skipped.
#[macro_export]
macro_rules! db_or_skip {
    () => {
        match common::build_lane::fresh_db().await {
            Some(db) => db,
            None => {
                eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
                return;
            }
        }
    };
}

/// A folder of this test's own for the uploads, removed when the test ends.
pub struct Work(std::path::PathBuf);

impl Work {
    pub fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("build-lane-{}", build_records::new_token()));
        std::fs::create_dir_all(&dir).expect("a folder");
        Self(dir)
    }

    pub fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for Work {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The lane in namespace `smartcity`, its uploads in `work_dir`.
pub fn settings(work_dir: &std::path::Path) -> Settings {
    Settings {
        namespace: "smartcity".into(),
        api: "http://jc-build.smartcity.svc.cluster.local:9092".into(),
        bind: "127.0.0.1:0".parse().expect("an address"),
        node_image: "r.example.org/builder@sha256:aa".into(),
        rust_image: "r.example.org/builder-rust@sha256:bb".into(),
        cache_size: "1Gi".into(),
        tmp_size: "4Gi".into(),
        work_dir: work_dir.to_owned(),
    }
}
