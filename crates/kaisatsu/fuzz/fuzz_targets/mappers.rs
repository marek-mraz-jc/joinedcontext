//! Input: JSON with a client (its protocol mappers), a user, groups, roles and client scopes, as
//! an admin import or manifest carries them. Every token kind ends in claims or an error.
#![no_main]

use kaisatsu_core::claims::{evaluate_mappers, MapperContext, TokenKind};
use kaisatsu_core::{
    check_signature_alg, ClientRepresentation, ClientScopeRepresentation, GroupRepresentation,
    RealmConfig, RoleRepresentation, UserRepresentation,
};
use libfuzzer_sys::fuzz_target;
use serde::Deserialize;

#[derive(Deserialize, Default)]
#[serde(default)]
struct Input {
    realm: Option<RealmConfig>,
    client: Option<ClientRepresentation>,
    user: Option<UserRepresentation>,
    groups: Vec<GroupRepresentation>,
    roles: Vec<RoleRepresentation>,
    scopes: Vec<ClientScopeRepresentation>,
    scope: String,
    alg: String,
}

fuzz_target!(|data: &[u8]| {
    let Ok(input) = serde_json::from_slice::<Input>(data) else {
        return;
    };
    let realm = input.realm.unwrap_or_default();
    let client = input.client.unwrap_or_default();
    let user = input.user.unwrap_or_default();
    let _ = check_signature_alg(&client.client_id, "alg", &input.alg);
    let _ = client.check_signature_algorithms();
    let _ = realm.acr_loa_map();
    let amr = vec!["pwd".to_owned()];
    for kind in [
        TokenKind::AccessToken,
        TokenKind::IdToken,
        TokenKind::UserInfo,
        TokenKind::Introspection,
    ] {
        let ctx = MapperContext {
            realm: &realm,
            issuer: "https://id.example/realms/r",
            client: &client,
            user: &user,
            user_groups: &input.groups,
            all_realm_roles: &input.roles,
            active_scopes: &input.scopes,
            granted_scope_string: &input.scope,
            acr: "1",
            amr: &amr,
            session_id: Some("s"),
            nonce: None,
            jti: "j",
            iat: 0,
            exp: 60,
            auth_time: None,
            dpop_jkt: None,
            act: None,
            active_org: None,
        };
        let _ = evaluate_mappers(&ctx, kind);
    }
});
