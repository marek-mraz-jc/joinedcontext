//! What the binary runs, `run` (coverage T-7896): the stub build refuses, a configuration that
//! cannot be read or an address that cannot be bound is an error, and a good one serves until
//! its shutdown resolves. The environment is the process's, so one test walks the cases in order.

fn set(pairs: &[(&str, &str)]) {
    for (key, value) in pairs {
        // This binary holds one test, so no other thread reads the environment.
        std::env::set_var(key, value);
    }
}

#[tokio::test]
async fn run_refuses_a_stub_build_a_missing_credential_and_a_taken_address() {
    // The stub build refuses before it reads anything.
    let error = agent_proxy::run(true, std::future::ready(()))
        .await
        .expect_err("a stub build does not start");
    assert!(error.to_string().contains("does not start"), "{error}");

    // A configuration without its credentials is refused with the variable named.
    let error = agent_proxy::run(false, std::future::ready(()))
        .await
        .expect_err("no credentials, no proxy");
    assert!(
        error.to_string().contains("missing environment variable"),
        "{error}"
    );

    // An address already taken cannot be bound.
    let taken = std::net::TcpListener::bind("127.0.0.1:0").expect("a port");
    let address = taken.local_addr().expect("an address").to_string();
    set(&[
        ("JC_OIDC_CLIENT_SECRET", "s"),
        ("JC_FORGE_TOKEN", "f"),
        ("JC_MODEL_KEY", "k"),
        ("JC_PORTAL_BASE", "http://portal.invalid:8080"),
        ("JC_PROXY_BIND", &address),
    ]);
    let error = agent_proxy::run(false, std::future::ready(()))
        .await
        .expect_err("the address is in use");
    assert!(
        error.to_string().to_lowercase().contains("in use"),
        "{error}"
    );
    drop(taken);

    // A free address serves until the shutdown resolves.
    set(&[("JC_PROXY_BIND", "127.0.0.1:0")]);
    agent_proxy::run(false, std::future::ready(()))
        .await
        .expect("a proxy that serves and stops");
}
