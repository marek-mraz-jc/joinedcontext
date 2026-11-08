#![forbid(unsafe_code)]
//! `kaisatsu-login` (W7):
//! - Fixed login state machines (enums with exhaustive match transitions, unit test per edge,
//!   and property test proving no path reaches `Code` without every required factor).
//! - Brute-force protector matching RI (10 failures in 12 h, 60 s step to 15 min, quick login
//!   1000 ms -> 60 s wait, never permanent, counted per person and per source IP, same answer
//!   and timing for wrong password and unknown person).
//! - Server-rendered Askama pages (`templates/login_page.html`) with per-response CSP nonce,
//!   `frame-ancestors 'none'`, CSRF token bound to `auth_session`, WCAG AA contrast, and
//!   i18n bundles for `en`, `sk`, `cs`, `de`, and `hu` (no literal user-facing strings in templates).
//! - Outbound e-mail sender via `lettre` over TLS with signed single-use 15-min action links.

pub mod email;
pub mod handlers;
pub mod i18n;
pub mod machine;
pub mod outbox;
pub mod passkey;

pub use email::{EmailSender, SignedActionToken};
pub use handlers::login_router;
pub use i18n::{Bundle, SUPPORTED_LOCALES, negotiate_locale};
/// The protector lives in `kaisatsu-oidc` so the password grant shares it with the login pages.
pub use kaisatsu_oidc::brute_force;
pub use kaisatsu_oidc::brute_force::BruteForceProtector;
pub use machine::{
    FactorRequirement, LoginInput, LoginPolicyContext, LoginState, TransitionError, transition,
};
pub use outbox::{MailSender, OutboxDispatcher, RealmSmtpSender, RecordedMail, RecordingSender};
pub use passkey::{
    allowed_origins_for_realm, build_creation_options, complete_passkey_authentication,
    complete_passkey_registration, effective_rp_id, ensure_user_handle,
};
