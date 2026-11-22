//! A forge of two Organizations on a real listener, for the tests that drive it with `git`.

use shako_core::{
    Action, InstanceId, LocalAuthorizer, OrgId, OutboundGuard, OutboundPolicy, RoleBinding,
    ShakoSigningKey, StaticTokenBinding, Subject, TokenVerifier,
};
use shako_server::{ShakoState, router};
use shako_store_memory::MemoryStore;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

pub struct World {
    _repositories: tempfile::TempDir,
    pub addr: SocketAddr,
    http: reqwest::Client,
}

impl World {
    /// `city` and `town` are Organizations of their own, each with a token named after it.
    pub async fn start(policy: OutboundPolicy) -> Self {
        let repositories = tempfile::tempdir().expect("tempdir");
        let orgs = [
            ("city", OrgId::from_u128(10)),
            ("town", OrgId::from_u128(20)),
        ];
        let bindings = orgs.map(|(name, org_id)| RoleBinding {
            org_id,
            principal: name.into(),
            actions: vec![Action::Admin, Action::Write],
            project_id: None,
            repo: None,
        });
        let tokens = orgs.map(|(name, org)| {
            StaticTokenBinding::new(
                &format!("{name}-token"),
                Subject::authenticated(name, name, vec![org], vec![]),
            )
        });
        let verifier = TokenVerifier::new(ShakoSigningKey::generate().expect("key"))
            .with_static_tokens(tokens.to_vec());
        let state = ShakoState::new(
            InstanceId::from_u128(1),
            orgs[0].1,
            repositories.path().to_path_buf(),
            Arc::new(MemoryStore::new()),
            Arc::new(LocalAuthorizer::new(bindings.to_vec())),
            verifier,
            OutboundGuard::new(policy),
        );
        for (name, org) in orgs {
            state.register_owner_org(name, org);
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listen");
        let addr = listener.local_addr().expect("address");
        let mount = jc_router::Mount::new("shako", "http", addr.to_string(), "").expect("mount");
        let app = router(state, mount);
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Self {
            _repositories: repositories,
            addr,
            http: reqwest::Client::new(),
        }
    }

    pub async fn post(&self, owner: &str, path: &str, body: serde_json::Value) {
        let status = self.post_status(owner, path, body).await;
        assert!(status.is_success(), "{path}: {status}");
    }

    /// The status of an API `POST` as `owner`.
    pub async fn post_status(
        &self,
        owner: &str,
        path: &str,
        body: serde_json::Value,
    ) -> reqwest::StatusCode {
        self.http
            .post(format!("http://{}/api/v1/{path}", self.addr))
            .header("authorization", format!("token {owner}-token"))
            .json(&body)
            .send()
            .await
            .expect("request")
            .status()
    }

    /// An empty repository and the address `git` uses for it.
    pub async fn repository(&self, owner: &str, name: &str) -> String {
        self.post(
            owner,
            &format!("orgs/{owner}/repos"),
            serde_json::json!({ "name": name, "default_branch": "main" }),
        )
        .await;
        format!(
            "http://{owner}:{owner}-token@{}/{owner}/{name}.git",
            self.addr
        )
    }
}

/// Runs `git` away from this checkout's configuration.
pub async fn git(dir: &Path, args: &[&str]) {
    let mut cmd = std::process::Command::new("git");
    cmd.current_dir(dir)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0");
    let out = tokio::task::spawn_blocking(move || cmd.output())
        .await
        .expect("join")
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A working copy on `main` that pushes to `url`.
pub async fn working_copy(dir: &Path, url: &str) {
    git(dir, &["init", "-q", "-b", "main"]).await;
    git(dir, &["config", "user.name", "Test"]).await;
    git(dir, &["config", "user.email", "test@example.invalid"]).await;
    git(dir, &["remote", "add", "origin", url]).await;
}

/// Commits `mib` MiB that no compression shrinks, as one file per `each` MiB.
pub async fn commit_random(dir: &Path, seed: u64, mib: usize, each: usize) {
    let mut state = seed | 1;
    for (index, start) in (0..mib).step_by(each).enumerate() {
        let mut bytes = Vec::new();
        for _ in 0..each.min(mib - start) * (1 << 17) {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            bytes.extend_from_slice(&state.to_le_bytes());
        }
        std::fs::write(dir.join(format!("{seed}-{index}.bin")), bytes).expect("write");
    }
    git(dir, &["add", "."]).await;
    git(dir, &["commit", "-q", "-m", "data"]).await;
}
