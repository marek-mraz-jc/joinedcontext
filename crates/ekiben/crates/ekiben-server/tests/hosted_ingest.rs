//! `hosted_ingest_router`: the shared ingest listener a host (jc) serves on its own listener. With no
//! stream behind a path, the listener answers 404 and never binds an address of its own.

#[tokio::test]
async fn the_hosted_ingest_router_answers_404_for_a_path_without_a_stream() {
    let app = ekiben_server::hosted_ingest_router();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await;
    });
    let res = reqwest::Client::new()
        .post(format!("http://{addr}/org-a/nothing/here"))
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::NOT_FOUND);
}
