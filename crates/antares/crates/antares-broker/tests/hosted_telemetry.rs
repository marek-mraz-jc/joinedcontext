// SPDX-License-Identifier: EUPL-1.2
//! A broker hosted in a process that already installed a tracing subscriber
//! and a metrics recorder (`jc`): telemetry init neither panics on the
//! subscriber nor fails on the second recorder, and the hosted init
//! installs nothing.

#[test]
fn init_accepts_an_installed_subscriber_and_recorder() {
    tracing_subscriber::fmt().with_test_writer().init();
    let host = metrics_exporter_prometheus::PrometheusBuilder::new()
        .install_recorder()
        .expect("the host's recorder");

    assert!(antares_broker::telemetry::init_hosted().is_none());

    // The standalone init with telemetry on, inside the same process.
    // This test binary runs this one test, so the variable reaches no other.
    std::env::set_var("ANTARES_TELEMETRY", "1");
    let render = antares_broker::telemetry::init().expect("init keeps the host's stack");
    assert!(render.is_none(), "no second recorder, no /q/metrics");

    // The broker's metrics land in the host's recorder.
    metrics::counter!("antares_http_requests_total").increment(1);
    assert!(host.render().contains("antares_http_requests_total"));
}
