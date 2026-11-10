//! `OrgQuotas.private_hosts` (E5): exact names only, empty by default, read from a quotas file.

use ekiben_core::{OrgContext, OrgId, OrgQuotas};

#[test]
fn private_hosts_are_empty_by_default_and_match_exact_names_only() {
    let org = OrgContext::new(OrgId::new("a"), OrgQuotas::default());
    assert!(org.get_quotas().private_hosts.is_empty());
    assert!(!org.is_private_host_allowed("context-gateway.jc.svc.cluster.local"));

    org.set_quotas(OrgQuotas {
        private_hosts: vec!["Context-Gateway.jc.svc.cluster.local".into()],
        ..OrgQuotas::default()
    });
    assert!(org.is_private_host_allowed("context-gateway.jc.svc.cluster.local"));
    assert!(!org.is_private_host_allowed("evil.context-gateway.jc.svc.cluster.local"));
    assert!(!org.is_private_host_allowed("context-gateway.jc.svc.cluster.local.evil.example"));
    assert!(!org.is_private_host_allowed(""));
}

#[test]
fn a_private_host_with_a_port_admits_that_port_alone() {
    let org = OrgContext::new(
        OrgId::new("a"),
        OrgQuotas {
            private_hosts: vec![
                "mqtt-helsinki-sensors.jc-mqtt.svc.cluster.local:1883".into(),
                "gateway.internal".into(),
            ],
            ..OrgQuotas::default()
        },
    );
    let broker = "mqtt-helsinki-sensors.jc-mqtt.svc.cluster.local";
    assert!(org.is_private_host_port_allowed(broker, 1883));
    assert!(org.is_private_host_port_allowed(&format!("{broker}."), 1883));
    assert!(!org.is_private_host_port_allowed(broker, 6060));
    assert!(!org.is_private_host_allowed(broker));
    assert!(!org.is_private_host_port_allowed(&format!("evil.{broker}"), 1883));
    assert!(!org.is_private_host_port_allowed("", 1883));
    // A bare name still admits every port.
    assert!(org.is_private_host_port_allowed("gateway.internal", 6060));
}

#[test]
fn private_hosts_parse_from_a_quotas_document_and_unknown_fields_stay_refused() {
    let q: OrgQuotas =
        serde_json::from_str(r#"{"private_hosts": ["context-gateway.jc.svc.cluster.local"]}"#)
            .expect("parses");
    assert_eq!(
        q.private_hosts,
        vec!["context-gateway.jc.svc.cluster.local"]
    );
    assert!(serde_json::from_str::<OrgQuotas>(r#"{"private_host": ["x"]}"#).is_err());
}
