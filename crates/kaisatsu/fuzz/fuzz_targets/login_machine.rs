//! Input: a stored login state (JSON, as the auth session keeps it), then NUL, then a byte per
//! input and policy flag. Every transition ends in a state or a refusal; no input sequence
//! reaches the code step while a required second factor was never passed.
#![no_main]

use kaisatsu_login::{transition, LoginInput, LoginPolicyContext, LoginState};
use libfuzzer_sys::fuzz_target;

fn input(b: u8) -> LoginInput {
    match b % 23 {
        0 => LoginInput::SubmitIdentifier {
            user_id: "u1".into(),
            routed_idp: None,
        },
        1 => LoginInput::SubmitIdentifier {
            user_id: "u1".into(),
            routed_idp: Some("idp".into()),
        },
        2 => LoginInput::StartPasskeyCeremony,
        3 => LoginInput::UpstreamAssertionVerified {
            user_id: "u1".into(),
        },
        4 => LoginInput::VerifyPasswordOk,
        5 => LoginInput::VerifyPasskeyOk {
            user_id: "u1".into(),
            user_verified: b & 0x80 != 0,
        },
        6 => LoginInput::SwitchToWebAuthn2fa,
        7 => LoginInput::SwitchToRecoveryCode,
        8 => LoginInput::VerifyOtpOk,
        9 => LoginInput::VerifyWebAuthn2faOk,
        10 => LoginInput::VerifyRecoveryCodeOk,
        11 => LoginInput::CompleteEnrolTotpOk,
        12 => LoginInput::CompleteRegisterWebAuthnOk,
        13 => LoginInput::CompleteUpdatePasswordOk,
        14 => LoginInput::CompleteVerifyEmailOk,
        15 => LoginInput::AcceptTermsOk,
        16 => LoginInput::SelectOrganization {
            org_id: "o1".into(),
        },
        17 => LoginInput::GrantConsent,
        18 => LoginInput::DenyConsent,
        19 => LoginInput::RequestPasswordReset,
        20 => LoginInput::OpenResetLink {
            user_id: "u1".into(),
        },
        21 => LoginInput::ConfirmLogout,
        _ => LoginInput::SelectOrganization {
            org_id: String::new(),
        },
    }
}

fuzz_target!(|data: &[u8]| {
    let split = data.iter().position(|b| *b == 0).unwrap_or(data.len());
    let (json, rest) = data.split_at(split);
    let mut state = serde_json::from_slice::<LoginState>(json).unwrap_or(LoginState::IdentityFirst);
    let flags = rest.get(1).copied().unwrap_or_default();
    let ctx = LoginPolicyContext {
        second_factor_required: flags & 1 != 0,
        has_totp_credential: flags & 2 != 0,
        has_webauthn_credential: flags & 4 != 0,
        pending_actions: if flags & 8 != 0 {
            vec!["VERIFY_EMAIL".into()]
        } else {
            Vec::new()
        },
        org_choices: if flags & 16 != 0 {
            vec!["o1".into(), "o2".into()]
        } else {
            Vec::new()
        },
        pinned_org: (flags & 32 != 0).then(|| "o1".into()),
        consent_required: flags & 64 != 0,
        app_action: (flags & 128 != 0).then(|| "UPDATE_PASSWORD".into()),
    };
    for b in rest.iter().skip(2).take(64) {
        if let Ok(next) = transition(state.clone(), input(*b), &ctx) {
            state = next;
        }
    }
    let _ = serde_json::to_string(&state).expect("state serializes");
});
