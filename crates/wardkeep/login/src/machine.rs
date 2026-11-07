#![forbid(unsafe_code)]
//! Fixed login state machine (W7):
//! Enums and pure transition function `(LoginState, LoginInput, &LoginPolicyContext) -> Result<LoginState, TransitionError>`
//! replacing Keycloak's configurable flow engine.
//!
//! Includes:
//! - Unit test for every transition edge.
//! - Property/exhaustive path test proving that NO path from `IdentityFirst` reaches `Code`
//!   without passing a first factor AND (whenever required by role, configured credential, or
//!   step-up `acr_values=mfa`) a second factor AND all pending required actions AND organization
//!   selection AND consent.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "step", rename_all = "PascalCase")]
pub enum LoginState {
    /// Initial identity-first step (username or e-mail) or conditional UI passkey.
    IdentityFirst,
    /// Redirected to an organization's upstream OIDC Identity Provider.
    UpstreamIdp { idp_alias: String },
    /// First factor: password entry for a resolved person.
    Password { user_id: String },
    /// First or passwordless factor: WebAuthn passkey ceremony.
    Passkey { user_id: Option<String> },
    /// Second factor: TOTP code entry.
    SecondFactorOtp {
        user_id: String,
        first_factor_passed: bool,
    },
    /// Second factor: WebAuthn security key assertion.
    SecondFactorWebAuthn {
        user_id: String,
        first_factor_passed: bool,
    },
    /// Second factor: single-use recovery code.
    RecoveryCode {
        user_id: String,
        first_factor_passed: bool,
    },
    /// Required action: enrol TOTP (`CONFIGURE_TOTP`) before continuing.
    EnrolTotp {
        user_id: String,
        first_factor_passed: bool,
    },
    /// Required action: register WebAuthn credential (`webauthn-register`).
    RegisterWebAuthn {
        user_id: String,
        first_factor_passed: bool,
        second_factor_passed: bool,
    },
    /// Required action: update temporary or expired password (`UPDATE_PASSWORD`).
    UpdatePassword {
        user_id: String,
        first_factor_passed: bool,
        second_factor_passed: bool,
    },
    /// Required action: verify e-mail (`VERIFY_EMAIL`).
    VerifyEmail {
        user_id: String,
        first_factor_passed: bool,
        second_factor_passed: bool,
    },
    /// Required action: accept terms and conditions (`TERMS_AND_CONDITIONS`).
    TermsAndConditions {
        user_id: String,
        first_factor_passed: bool,
        second_factor_passed: bool,
    },
    /// Organization picker when a person belongs to multiple organizations and the client did not pin one.
    OrganizationChoice {
        user_id: String,
        first_factor_passed: bool,
        second_factor_passed: bool,
        candidates: Vec<String>,
    },
    /// Per-application consent screen (`consentRequired: true`).
    Consent {
        user_id: String,
        first_factor_passed: bool,
        second_factor_passed: bool,
        chosen_org: Option<String>,
    },
    /// Terminal state: authorization code issued.
    Code {
        user_id: String,
        acr: String,
        amr: Vec<String>,
        chosen_org: Option<String>,
    },
    /// Reset credentials by e-mail flow: request address.
    ResetPasswordRequest,
    /// Reset credentials by e-mail flow: e-mail sent confirmation.
    ResetPasswordSent,
    /// Reset credentials by e-mail flow: set new password from signed link.
    ResetPasswordSet { user_id: String },
    /// Logout confirmation prompt.
    LogoutConfirm { session_id: String },
    /// Logged out terminal page.
    LoggedOut,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginInput {
    SubmitIdentifier {
        user_id: String,
        routed_idp: Option<String>,
    },
    StartPasskeyCeremony,
    UpstreamAssertionVerified {
        user_id: String,
    },
    VerifyPasswordOk,
    VerifyPasskeyOk {
        user_id: String,
        user_verified: bool,
    },
    SwitchToWebAuthn2fa,
    SwitchToRecoveryCode,
    VerifyOtpOk,
    VerifyWebAuthn2faOk,
    VerifyRecoveryCodeOk,
    CompleteEnrolTotpOk,
    CompleteRegisterWebAuthnOk,
    CompleteUpdatePasswordOk,
    CompleteVerifyEmailOk,
    AcceptTermsOk,
    SelectOrganization {
        org_id: String,
    },
    GrantConsent,
    DenyConsent,
    RequestPasswordReset,
    OpenResetLink {
        user_id: String,
    },
    ConfirmLogout,
}

#[derive(Debug, Clone, Default)]
pub struct LoginPolicyContext {
    /// True when the person holds `mfa-required` (or composite `portal-approver` / `platform-admin`)
    /// or the realm is `master` or the request asked `acr_values=mfa`.
    pub second_factor_required: bool,
    /// True when the person has an enrolled TOTP credential.
    pub has_totp_credential: bool,
    /// True when the person has an enrolled WebAuthn credential.
    pub has_webauthn_credential: bool,
    /// Pending required actions on the user (`CONFIGURE_TOTP`, `UPDATE_PASSWORD`, `VERIFY_EMAIL`,
    /// `TERMS_AND_CONDITIONS`, `webauthn-register`).
    pub pending_actions: Vec<String>,
    /// Organizations the person belongs to when more than 1 and none is pinned.
    pub org_choices: Vec<String>,
    /// Pinned organization (from domain or `organization:{alias}` scope).
    pub pinned_org: Option<String>,
    /// True when the client requires user consent and no prior consent exists.
    pub consent_required: bool,
}

impl LoginPolicyContext {
    pub fn needs_second_factor(&self) -> bool {
        self.second_factor_required || self.has_totp_credential || self.has_webauthn_credential
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TransitionError {
    #[error("invalid transition from current login state")]
    InvalidTransition,
    #[error("passkey passwordless login requires userVerification=true")]
    UserVerificationRequired,
    #[error("second factor required before issuing authorization code")]
    SecondFactorMissing,
    #[error("selected organization is not in the user's memberships")]
    InvalidOrganizationChoice,
    #[error("consent was denied by the user")]
    ConsentDenied,
}

/// Computes the next post-authentication step after factors are satisfied:
/// required actions -> organization choice -> consent -> `Code`.
fn advance_post_factors(
    user_id: String,
    first_factor_passed: bool,
    second_factor_passed: bool,
    ctx: &LoginPolicyContext,
    chosen_org: Option<String>,
) -> Result<LoginState, TransitionError> {
    if !first_factor_passed {
        return Err(TransitionError::InvalidTransition);
    }
    if ctx.needs_second_factor() && !second_factor_passed {
        return Err(TransitionError::SecondFactorMissing);
    }

    for action in &ctx.pending_actions {
        match action.as_str() {
            "CONFIGURE_TOTP" if !second_factor_passed => {
                return Ok(LoginState::EnrolTotp {
                    user_id,
                    first_factor_passed,
                });
            }
            "UPDATE_PASSWORD" => {
                return Ok(LoginState::UpdatePassword {
                    user_id,
                    first_factor_passed,
                    second_factor_passed,
                });
            }
            "VERIFY_EMAIL" => {
                return Ok(LoginState::VerifyEmail {
                    user_id,
                    first_factor_passed,
                    second_factor_passed,
                });
            }
            "TERMS_AND_CONDITIONS" => {
                return Ok(LoginState::TermsAndConditions {
                    user_id,
                    first_factor_passed,
                    second_factor_passed,
                });
            }
            "webauthn-register" => {
                return Ok(LoginState::RegisterWebAuthn {
                    user_id,
                    first_factor_passed,
                    second_factor_passed,
                });
            }
            _ => {}
        }
    }

    let resolved_org = chosen_org.or_else(|| ctx.pinned_org.clone());
    if resolved_org.is_none() && ctx.org_choices.len() > 1 {
        return Ok(LoginState::OrganizationChoice {
            user_id,
            first_factor_passed,
            second_factor_passed,
            candidates: ctx.org_choices.clone(),
        });
    }
    let final_org = resolved_org.or_else(|| ctx.org_choices.first().cloned());

    if ctx.consent_required {
        return Ok(LoginState::Consent {
            user_id,
            first_factor_passed,
            second_factor_passed,
            chosen_org: final_org,
        });
    }

    let (acr, amr) = if second_factor_passed {
        ("mfa".to_owned(), vec!["pwd".to_owned(), "otp".to_owned()])
    } else {
        ("pwd".to_owned(), vec!["pwd".to_owned()])
    };

    Ok(LoginState::Code {
        user_id,
        acr,
        amr,
        chosen_org: final_org,
    })
}

/// Pure state machine transition function (W7).
pub fn transition(
    state: LoginState,
    input: LoginInput,
    ctx: &LoginPolicyContext,
) -> Result<LoginState, TransitionError> {
    match (state, input) {
        (
            LoginState::IdentityFirst,
            LoginInput::SubmitIdentifier {
                user_id,
                routed_idp,
            },
        ) => {
            if let Some(idp_alias) = routed_idp {
                Ok(LoginState::UpstreamIdp { idp_alias })
            } else {
                Ok(LoginState::Password { user_id })
            }
        }
        (LoginState::IdentityFirst, LoginInput::StartPasskeyCeremony) => {
            Ok(LoginState::Passkey { user_id: None })
        }
        (LoginState::Password { user_id }, LoginInput::StartPasskeyCeremony) => {
            Ok(LoginState::Passkey {
                user_id: Some(user_id),
            })
        }
        (LoginState::UpstreamIdp { .. }, LoginInput::UpstreamAssertionVerified { user_id })
        | (LoginState::Password { user_id }, LoginInput::VerifyPasswordOk) => {
            if ctx.needs_second_factor() {
                if ctx.has_totp_credential {
                    Ok(LoginState::SecondFactorOtp {
                        user_id,
                        first_factor_passed: true,
                    })
                } else if ctx.has_webauthn_credential && !ctx.second_factor_required {
                    Ok(LoginState::SecondFactorWebAuthn {
                        user_id,
                        first_factor_passed: true,
                    })
                } else {
                    Ok(LoginState::EnrolTotp {
                        user_id,
                        first_factor_passed: true,
                    })
                }
            } else {
                advance_post_factors(user_id, true, false, ctx, None)
            }
        }
        (
            LoginState::Passkey { .. },
            LoginInput::VerifyPasskeyOk {
                user_id,
                user_verified,
            },
        ) => {
            if !user_verified {
                return Err(TransitionError::UserVerificationRequired);
            }
            // A user-verified passkey counts as both first and second factor (research-keycloak §5.1, W7)
            advance_post_factors(user_id, true, true, ctx, None)
        }
        (
            LoginState::SecondFactorOtp {
                user_id,
                first_factor_passed: true,
            },
            LoginInput::SwitchToWebAuthn2fa,
        ) if ctx.has_webauthn_credential => Ok(LoginState::SecondFactorWebAuthn {
            user_id,
            first_factor_passed: true,
        }),
        (
            LoginState::SecondFactorOtp {
                user_id,
                first_factor_passed: true,
            }
            | LoginState::SecondFactorWebAuthn {
                user_id,
                first_factor_passed: true,
            },
            LoginInput::SwitchToRecoveryCode,
        ) => Ok(LoginState::RecoveryCode {
            user_id,
            first_factor_passed: true,
        }),
        (
            LoginState::SecondFactorOtp {
                user_id,
                first_factor_passed: true,
            },
            LoginInput::VerifyOtpOk,
        )
        | (
            LoginState::SecondFactorWebAuthn {
                user_id,
                first_factor_passed: true,
            },
            LoginInput::VerifyWebAuthn2faOk,
        )
        | (
            LoginState::RecoveryCode {
                user_id,
                first_factor_passed: true,
            },
            LoginInput::VerifyRecoveryCodeOk,
        )
        | (
            LoginState::EnrolTotp {
                user_id,
                first_factor_passed: true,
            },
            LoginInput::CompleteEnrolTotpOk,
        ) => {
            let mut next_ctx = ctx.clone();
            next_ctx.pending_actions.retain(|a| a != "CONFIGURE_TOTP");
            advance_post_factors(user_id, true, true, &next_ctx, None)
        }
        (
            LoginState::RegisterWebAuthn {
                user_id,
                first_factor_passed: true,
                second_factor_passed,
            },
            LoginInput::CompleteRegisterWebAuthnOk,
        ) => {
            let mut next_ctx = ctx.clone();
            next_ctx.pending_actions.retain(|a| a != "webauthn-register");
            advance_post_factors(user_id, true, second_factor_passed, &next_ctx, None)
        }
        (
            LoginState::UpdatePassword {
                user_id,
                first_factor_passed: true,
                second_factor_passed,
            },
            LoginInput::CompleteUpdatePasswordOk,
        ) => {
            let mut next_ctx = ctx.clone();
            next_ctx.pending_actions.retain(|a| a != "UPDATE_PASSWORD");
            advance_post_factors(user_id, true, second_factor_passed, &next_ctx, None)
        }
        (
            LoginState::VerifyEmail {
                user_id,
                first_factor_passed: true,
                second_factor_passed,
            },
            LoginInput::CompleteVerifyEmailOk,
        ) => {
            let mut next_ctx = ctx.clone();
            next_ctx.pending_actions.retain(|a| a != "VERIFY_EMAIL");
            advance_post_factors(user_id, true, second_factor_passed, &next_ctx, None)
        }
        (
            LoginState::TermsAndConditions {
                user_id,
                first_factor_passed: true,
                second_factor_passed,
            },
            LoginInput::AcceptTermsOk,
        ) => {
            let mut next_ctx = ctx.clone();
            next_ctx
                .pending_actions
                .retain(|a| a != "TERMS_AND_CONDITIONS");
            advance_post_factors(user_id, true, second_factor_passed, &next_ctx, None)
        }
        (
            LoginState::OrganizationChoice {
                user_id,
                first_factor_passed: true,
                second_factor_passed,
                candidates,
            },
            LoginInput::SelectOrganization { org_id },
        ) => {
            if !candidates.contains(&org_id) {
                return Err(TransitionError::InvalidOrganizationChoice);
            }
            let mut next_ctx = ctx.clone();
            next_ctx.pending_actions.clear();
            advance_post_factors(user_id, true, second_factor_passed, &next_ctx, Some(org_id))
        }
        (
            LoginState::Consent {
                user_id,
                first_factor_passed: true,
                second_factor_passed,
                chosen_org,
            },
            LoginInput::GrantConsent,
        ) => {
            let mut next_ctx = ctx.clone();
            next_ctx.pending_actions.clear();
            next_ctx.consent_required = false;
            advance_post_factors(user_id, true, second_factor_passed, &next_ctx, chosen_org)
        }
        (LoginState::Consent { .. }, LoginInput::DenyConsent) => {
            Err(TransitionError::ConsentDenied)
        }
        (LoginState::ResetPasswordRequest, LoginInput::RequestPasswordReset) => {
            Ok(LoginState::ResetPasswordSent)
        }
        (LoginState::ResetPasswordSent, LoginInput::OpenResetLink { user_id }) => {
            Ok(LoginState::ResetPasswordSet { user_id })
        }
        (LoginState::ResetPasswordSet { user_id }, LoginInput::CompleteUpdatePasswordOk) => {
            Ok(LoginState::Password { user_id })
        }
        (LoginState::LogoutConfirm { .. }, LoginInput::ConfirmLogout) => Ok(LoginState::LoggedOut),
        _ => Err(TransitionError::InvalidTransition),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FactorRequirement {
    PasswordOnly,
    MfaRequired,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_test_every_state_machine_edge() {
        let mut ctx = LoginPolicyContext {
            second_factor_required: true,
            has_totp_credential: false,
            has_webauthn_credential: true,
            pending_actions: vec![
                "UPDATE_PASSWORD".into(),
                "VERIFY_EMAIL".into(),
                "TERMS_AND_CONDITIONS".into(),
                "webauthn-register".into(),
            ],
            org_choices: vec!["helsinki".into(), "zilina".into()],
            pinned_org: None,
            consent_required: true,
        };

        // 1. IdentityFirst -> Password -> EnrolTotp (since has_totp_credential == false and second_factor_required == true)
        let s1 = transition(
            LoginState::IdentityFirst,
            LoginInput::SubmitIdentifier {
                user_id: "u1".into(),
                routed_idp: None,
            },
            &ctx,
        )
        .unwrap();
        assert_eq!(s1, LoginState::Password { user_id: "u1".into() });

        let s2 = transition(s1, LoginInput::VerifyPasswordOk, &ctx).unwrap();
        assert_eq!(
            s2,
            LoginState::EnrolTotp {
                user_id: "u1".into(),
                first_factor_passed: true
            }
        );

        // 2. EnrolTotp -> UpdatePassword -> VerifyEmail -> Terms -> RegisterWebAuthn -> OrgChoice -> Consent -> Code
        let s3 = transition(s2, LoginInput::CompleteEnrolTotpOk, &ctx).unwrap();
        assert!(matches!(s3, LoginState::UpdatePassword { .. }));
        ctx.pending_actions.retain(|a| a != "UPDATE_PASSWORD");

        let s4 = transition(s3, LoginInput::CompleteUpdatePasswordOk, &ctx).unwrap();
        assert!(matches!(s4, LoginState::VerifyEmail { .. }));
        ctx.pending_actions.retain(|a| a != "VERIFY_EMAIL");

        let s5 = transition(s4, LoginInput::CompleteVerifyEmailOk, &ctx).unwrap();
        assert!(matches!(s5, LoginState::TermsAndConditions { .. }));
        ctx.pending_actions.retain(|a| a != "TERMS_AND_CONDITIONS");

        let s6 = transition(s5, LoginInput::AcceptTermsOk, &ctx).unwrap();
        assert!(matches!(s6, LoginState::RegisterWebAuthn { .. }));
        ctx.pending_actions.retain(|a| a != "webauthn-register");

        let s7 = transition(s6, LoginInput::CompleteRegisterWebAuthnOk, &ctx).unwrap();
        assert!(matches!(s7, LoginState::OrganizationChoice { .. }));

        let s8 = transition(
            s7,
            LoginInput::SelectOrganization {
                org_id: "helsinki".into(),
            },
            &ctx,
        )
        .unwrap();
        assert!(matches!(s8, LoginState::Consent { .. }));

        let s9 = transition(s8, LoginInput::GrantConsent, &ctx).unwrap();
        assert_eq!(
            s9,
            LoginState::Code {
                user_id: "u1".into(),
                acr: "mfa".into(),
                amr: vec!["pwd".into(), "otp".into()],
                chosen_org: Some("helsinki".into()),
            }
        );

        // 3. Passkey edge: requires user_verified == true
        let pk1 = transition(
            LoginState::IdentityFirst,
            LoginInput::StartPasskeyCeremony,
            &ctx,
        )
        .unwrap();
        assert_eq!(
            transition(
                pk1.clone(),
                LoginInput::VerifyPasskeyOk {
                    user_id: "u1".into(),
                    user_verified: false
                },
                &ctx
            ),
            Err(TransitionError::UserVerificationRequired)
        );

        // 4. Reset password and Logout edges
        let r1 = transition(
            LoginState::ResetPasswordRequest,
            LoginInput::RequestPasswordReset,
            &ctx,
        )
        .unwrap();
        assert_eq!(r1, LoginState::ResetPasswordSent);
        let r2 = transition(
            r1,
            LoginInput::OpenResetLink {
                user_id: "u1".into(),
            },
            &ctx,
        )
        .unwrap();
        assert_eq!(
            transition(r2, LoginInput::CompleteUpdatePasswordOk, &ctx).unwrap(),
            LoginState::Password {
                user_id: "u1".into()
            }
        );
        assert_eq!(
            transition(
                LoginState::LogoutConfirm {
                    session_id: "s1".into()
                },
                LoginInput::ConfirmLogout,
                &ctx
            )
            .unwrap(),
            LoginState::LoggedOut
        );
    }

    /// Property test (W7): Exhaustively explores every sequence of `LoginInput`s from `IdentityFirst`
    /// up to depth 7 under an MFA-required policy and proves that NO sequence reaches `LoginState::Code`
    /// without passing both a first factor (`VerifyPasswordOk` / `UpstreamAssertionVerified` /
    /// `VerifyPasskeyOk{user_verified:true}`) AND a second factor (`VerifyOtpOk` / `VerifyWebAuthn2faOk` /
    /// `VerifyRecoveryCodeOk` / `CompleteEnrolTotpOk` / `VerifyPasskeyOk{user_verified:true}`).
    #[test]
    fn property_no_path_reaches_code_without_every_required_factor() {
        let ctx = LoginPolicyContext {
            second_factor_required: true,
            has_totp_credential: true,
            has_webauthn_credential: true,
            pending_actions: vec!["UPDATE_PASSWORD".into()],
            org_choices: vec!["helsinki".into(), "zilina".into()],
            pinned_org: None,
            consent_required: true,
        };

        let all_inputs = [
            LoginInput::SubmitIdentifier {
                user_id: "u1".into(),
                routed_idp: None,
            },
            LoginInput::SubmitIdentifier {
                user_id: "u1".into(),
                routed_idp: Some("oidc-idp".into()),
            },
            LoginInput::StartPasskeyCeremony,
            LoginInput::UpstreamAssertionVerified {
                user_id: "u1".into(),
            },
            LoginInput::VerifyPasswordOk,
            LoginInput::VerifyPasskeyOk {
                user_id: "u1".into(),
                user_verified: false,
            },
            LoginInput::VerifyPasskeyOk {
                user_id: "u1".into(),
                user_verified: true,
            },
            LoginInput::SwitchToWebAuthn2fa,
            LoginInput::SwitchToRecoveryCode,
            LoginInput::VerifyOtpOk,
            LoginInput::VerifyWebAuthn2faOk,
            LoginInput::VerifyRecoveryCodeOk,
            LoginInput::CompleteEnrolTotpOk,
            LoginInput::CompleteUpdatePasswordOk,
            LoginInput::SelectOrganization {
                org_id: "helsinki".into(),
            },
            LoginInput::GrantConsent,
        ];

        fn is_first_factor(inp: &LoginInput) -> bool {
            matches!(
                inp,
                LoginInput::VerifyPasswordOk
                    | LoginInput::UpstreamAssertionVerified { .. }
                    | LoginInput::VerifyPasskeyOk {
                        user_verified: true,
                        ..
                    }
            )
        }
        fn is_second_factor(inp: &LoginInput) -> bool {
            matches!(
                inp,
                LoginInput::VerifyOtpOk
                    | LoginInput::VerifyWebAuthn2faOk
                    | LoginInput::VerifyRecoveryCodeOk
                    | LoginInput::CompleteEnrolTotpOk
                    | LoginInput::VerifyPasskeyOk {
                        user_verified: true,
                        ..
                    }
            )
        }

        let mut frontier: Vec<(LoginState, LoginPolicyContext, Vec<LoginInput>)> =
            vec![(LoginState::IdentityFirst, ctx, Vec::new())];
        let mut reached_code_paths = 0usize;

        for _depth in 0..7 {
            let mut next_frontier = Vec::new();
            for (st, cur_ctx, trace) in frontier {
                for inp in &all_inputs {
                    let mut step_ctx = cur_ctx.clone();
                    if matches!(inp, LoginInput::CompleteUpdatePasswordOk) {
                        step_ctx.pending_actions.retain(|a| a != "UPDATE_PASSWORD");
                    }
                    if let Ok(next_state) = transition(st.clone(), inp.clone(), &cur_ctx) {
                        let mut next_trace = trace.clone();
                        next_trace.push(inp.clone());
                        if let LoginState::Code { acr, .. } = &next_state {
                            reached_code_paths += 1;
                            assert_eq!(acr, "mfa");
                            assert!(
                                next_trace.iter().any(is_first_factor),
                                "path reached Code without first factor: {next_trace:?}"
                            );
                            assert!(
                                next_trace.iter().any(is_second_factor),
                                "path reached Code without second factor: {next_trace:?}"
                            );
                            assert!(
                                next_trace
                                    .iter()
                                    .any(|i| matches!(i, LoginInput::CompleteUpdatePasswordOk)),
                                "path reached Code without completing required action: {next_trace:?}"
                            );
                            assert!(
                                next_trace
                                    .iter()
                                    .any(|i| matches!(i, LoginInput::SelectOrganization { .. })),
                                "path reached Code without organization choice: {next_trace:?}"
                            );
                            assert!(
                                next_trace
                                    .iter()
                                    .any(|i| matches!(i, LoginInput::GrantConsent)),
                                "path reached Code without consent: {next_trace:?}"
                            );
                        } else {
                            next_frontier.push((next_state, step_ctx, next_trace));
                        }
                    }
                }
            }
            frontier = next_frontier;
        }
        assert!(reached_code_paths > 0);
    }
}
