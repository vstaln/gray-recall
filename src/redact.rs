// Copied from vstaln/gray-memory `src/redact.rs` with path stripping removed,
// so file paths stay searchable. That file is vendored from gray
// `crates/gray-core/src/redaction.rs`, itself vendored from Hmbown/CodeWhale
// (MIT) `crates/workflow/src/redaction.rs`.
// Copyright (c) 2024-2025 DeepSeek CLI Contributors. See THIRD_PARTY_NOTICES.md.
//! Secret redaction for everything gray-recall writes to its index.
//!
//! Secret-shaped tokens (provider keys, bearer tokens, `SOMETHING_KEY=value`
//! assignments, Discord bot tokens) become `<redacted>`. Paths are kept:
//! they are what `--file` searches.

use std::sync::LazyLock;

use regex::Regex;
use std::collections::BTreeSet;

pub const REDACTION_SECRET: &str = "secret";

/// Placeholder substituted for a removed secret-shaped token.
/// Placeholder substituted for a removed secret. Public so callers that mask a
/// known value verbatim (e.g. a credential read out of `auth.json`) produce the
/// same marker instead of inventing a second spelling.
pub const REDACTED: &str = "<redacted>";

/// Case-insensitive substrings that mark an identifier as secret-bearing.
const SECRET_NAME_MARKERS: &[&str] = &[
    "api_key",
    "apikey",
    "secret",
    "token",
    "password",
    "passwd",
    "credential",
    "authorization",
    "auth_token",
    "access_key",
    "private_key",
    "session_key",
];

/// Prefixes that are themselves a credential, whatever they are attached to.
///
/// Every entry here must be *unambiguously* a credential prefix. A prefix that
/// is also an ordinary English word — `bearer`, `asia` — belongs nowhere near
/// this list: it would redact prose and, worse, would report a `secret`
/// redaction kind on a receipt that removed nothing but a word. The AWS key ids
/// that motivated `asia`/`akia` are handled by
/// [`looks_like_aws_access_key`], which requires the full shape.
const SECRET_VALUE_PREFIXES: &[&str] = &[
    "sk-",
    "sk_",
    "ghp_",
    "gho_",
    "ghs_",
    "github_pat_",
    "xoxb-",
    "xoxp-",
    "xapp-",
];

/// HTTP authorization scheme keywords, in their canonical HTTP capitalization.
///
/// These are *never* the secret — the secret is the token that follows them.
/// Redacting the keyword and stopping there is the failure this list exists to
/// prevent: `Authorization: Bearer <token>` would keep the token and still
/// claim on the receipt that a secret had been removed.
///
/// The capitalization is stored, not normalized away, because it carries
/// evidence: `Bearer` is HTTP syntax and `bearer` is an English noun. See
/// [`is_canonical_auth_scheme`].
const AUTH_SCHEMES: &[&str] = &["Bearer", "Basic", "Digest", "Token"];

/// The one scheme keyword whose canonical capitalization is, by itself, enough
/// to treat the following token as a credential.
///
/// The asymmetry is deliberate and is the whole of the false-positive story.
/// `Basic`, `Digest`, and `Token` are ordinary capitalized English words —
/// "Basic auth is enabled", "Token holders vote", "Digest the results" — and
/// arming on them would redact the next word of perfectly ordinary prose. `Bearer`
/// capitalized is, in a technical corpus, the HTTP scheme essentially every
/// time. So `Bearer qqq` loses `qqq` even though a three-letter lowercase token
/// looks like nothing at all, while the other three need header context first.
///
/// The residual cost is stated plainly: `Bearer tokens are rotated weekly`
/// redacts `tokens`. That is a capitalized-`Bearer` sentence, which is header
/// syntax by shape; ordinary prose says "bearer", and lowercase never arms on
/// its own.
const SELF_EVIDENT_AUTH_SCHEME: &str = "Bearer";

/// AWS access-key-id prefixes. Matched case-sensitively and only against the
/// full 20-character shape, so the word `Asia` is prose and `ASIA…` is a key.
const AWS_KEY_ID_PREFIXES: &[&str] = &[
    "AKIA", "ASIA", "AGPA", "AIDA", "AROA", "ANPA", "ANVA", "ASCA", "ABIA", "ACCA",
];

/// Minimum length of an AWS access key id (`AKIA` + 16).
const AWS_KEY_ID_LEN: usize = 20;

/// Minimum length before a bare token is treated as a credential value.
const CREDENTIAL_VALUE_MIN_LEN: usize = 16;

/// The result of redacting one string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redaction {
    text: String,
    kinds: BTreeSet<String>,
}

impl Redaction {
    /// The redacted text.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Consume the redaction, yielding the redacted text.
    #[must_use]
    pub fn into_text(self) -> String {
        self.text
    }

    /// Whether anything was removed.
    #[must_use]
    pub fn redacted(&self) -> bool {
        !self.kinds.is_empty()
    }

    /// Whether a secret was removed (as opposed to only paths).
    ///
    /// Durable tool/session surfaces gate on this: secret-bearing text is
    /// scrubbed aggressively (paths in the same unit go too), while
    /// secret-free tool data — absolute paths, fence markup like
    /// `</untrusted-output>` — stays verbatim. Scrubbing paths out of shell
    /// output would break the shell fence contract and the tool's utility;
    /// see `redact_bytes_for_log`.
    #[must_use]
    pub fn has_secret(&self) -> bool {
        self.kinds.iter().any(|k| k == REDACTION_SECRET)
    }

    /// Which classes of content were removed — never the content itself.
    #[must_use]
    pub fn kinds(&self) -> Vec<String> {
        self.kinds.iter().cloned().collect()
    }
}

/// How strongly the preceding tokens claim that the *next* token is a
/// credential.
///
/// A credential is routinely written as two or three tokens
/// (`Authorization: Bearer <token>`), so the decision cannot be made per token.
/// But the evidence for "a credential follows" is not uniform, and collapsing it
/// to a boolean is what produces either a leak or a mangled sentence:
///
/// - `Authorization: Bearer qqq` — the credential is `qqq`, three lowercase
///   letters, indistinguishable in shape from a word. Only the *context* says
///   it is a secret, and the context says so unambiguously.
/// - `authorization: bearer shares responsibility` — the same context markers,
///   lowercase, and the next token is an English verb. Redacting `shares` would
///   destroy a sentence and put a false `secret` kind on a receipt.
///
/// So the arming carries its own strength, and the shape test is applied only
/// where the context is weak enough to need it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CredentialArm {
    /// Nothing is expected; the next token is judged on its own.
    None,
    /// The next token is the credential only if it also *looks* like one.
    /// Lowercase scheme words and bare `Authorization:` land here.
    IfShaped,
    /// The next token is the credential whatever it looks like. Reached by
    /// canonical HTTP capitalization — `Authorization: Bearer …`, or a bare
    /// `Bearer` — where the syntax alone settles it.
    Certain,
}

impl CredentialArm {
    const fn is_armed(self) -> bool {
        !matches!(self, Self::None)
    }
}

/// What one token resolved to, and whether it armed the *next* token.
struct TokenOutcome {
    /// `None` keeps the token verbatim.
    text: Option<String>,
    /// Whether — and how strongly — the following token carries the credential
    /// this one introduced. `Authorization:` and a bare `Bearer` reveal nothing
    /// themselves; the value after them is the whole secret.
    arm: CredentialArm,
}

impl TokenOutcome {
    const fn keep() -> Self {
        Self {
            text: None,
            arm: CredentialArm::None,
        }
    }

    const fn keep_and_arm(arm: CredentialArm) -> Self {
        Self { text: None, arm }
    }

    fn replace(text: String) -> Self {
        Self {
            text: Some(text),
            arm: CredentialArm::None,
        }
    }
}

/// Redact secret-shaped tokens from `input`; paths are kept.
///
/// Operates token-by-token over whitespace-free runs, which is enough for the
/// bounded, already whitespace-collapsed text this crate transmits and keeps
/// the rule set auditable without a regex dependency.
///
/// One piece of state crosses token boundaries, and it has to: a credential is
/// routinely written as *two* tokens (`Authorization: Bearer <token>`), so a
/// purely per-token rule either leaks the value or redacts the English word
/// `bearer`. Carrying "the next token is the credential" forward — and *how
/// certainly*, see `CredentialArm` — is what lets this do neither.
#[must_use]
pub fn redact_secret_tokens(input: &str) -> Redaction {
    // The tokenizer below splits on `' '` only, so `'\n'` glues neighbouring
    // lines into one token: a secret on a later line escaped, and one that
    // fired armed the *next* glued token, deleting every line after it. Tool
    // output is multi-line by default, so redact line by line.
    if input.contains('\n') {
        let mut kinds = BTreeSet::new();
        let text = input
            .split('\n')
            .map(|line| {
                let redaction = redact_secret_tokens(line);
                kinds.extend(redaction.kinds.iter().cloned());
                redaction.into_text()
            })
            .collect::<Vec<_>>()
            .join("\n");
        return Redaction { text, kinds };
    }
    let mut kinds = BTreeSet::new();
    let tokens: Vec<&str> = input.split(' ').collect();
    let mut out: Vec<String> = Vec::with_capacity(tokens.len());
    let mut arm = CredentialArm::None;

    for (index, token) in tokens.iter().enumerate() {
        if token.is_empty() {
            out.push(String::new());
            continue;
        }
        if arm.is_armed() {
            // `Authorization: Bearer <token>` — the scheme keyword is not the
            // secret, so it survives and the arming carries past it. A
            // canonically capitalized keyword also *upgrades* the arming: the
            // header name alone left the shape in doubt, and `Bearer` settles
            // it.
            if is_auth_scheme(token) {
                if is_canonical_auth_scheme(token) {
                    arm = CredentialArm::Certain;
                }
                out.push((*token).to_string());
                continue;
            }
            // Weak arming still defers to shape, so an `authorization:` that
            // introduces a sentence rather than a secret leaves the sentence
            // intact — and leaves the receipt honest about having removed
            // nothing.
            if arm == CredentialArm::Certain || looks_like_credential_value(token) {
                kinds.insert(REDACTION_SECRET.to_string());
                out.push(REDACTED.to_string());
                arm = CredentialArm::None;
                continue;
            }
            // Fall through: the token is judged on its own merits, and the
            // arming state is replaced wholesale by `redact_token` below.
        }

        let next = tokens[index + 1..]
            .iter()
            .copied()
            .find(|candidate| !candidate.is_empty());
        let outcome = redact_token(token, next, &mut kinds);
        arm = outcome.arm;
        out.push(outcome.text.unwrap_or_else(|| (*token).to_string()));
    }

    Redaction {
        text: out.join(" "),
        kinds,
    }
}

/// Redact one whitespace-free token, recording what was removed.
///
/// `next` is the following non-empty token, used only to decide whether a bare
/// authorization scheme keyword is introducing a credential or is just a word.
fn redact_token(token: &str, next: Option<&str>, kinds: &mut BTreeSet<String>) -> TokenOutcome {
    // `NAME=value` / `NAME:value` — a secret-bearing name redacts its value and
    // keeps the name, which is the useful half.
    for separator in ['=', ':'] {
        let Some((name, value)) = token.split_once(separator) else {
            continue;
        };
        let lowered = name.to_ascii_lowercase();
        let secret_name = SECRET_NAME_MARKERS
            .iter()
            .any(|marker| lowered.contains(marker));
        if secret_name {
            // `Authorization:Bearer` carries no secret of its own; the
            // credential is the next token. Canonical capitalization on the
            // scheme makes that certain; `authorization:bearer` does not.
            if is_auth_scheme(value) {
                return TokenOutcome::keep_and_arm(if is_canonical_auth_scheme(value) {
                    CredentialArm::Certain
                } else {
                    CredentialArm::IfShaped
                });
            }
            // A bare `Authorization:` only introduces a credential when one
            // actually follows. `authorization: needed before merge` is a
            // sentence, and redacting `needed` would report a secret that was
            // never there. The arming stays weak here even when a scheme word
            // follows — the scheme token itself decides, on the next pass,
            // whether its capitalization upgrades it.
            if value.is_empty() {
                return if next
                    .is_some_and(|next| is_auth_scheme(next) || looks_like_credential_value(next))
                {
                    TokenOutcome::keep_and_arm(CredentialArm::IfShaped)
                } else {
                    TokenOutcome::keep()
                };
            }
            kinds.insert(REDACTION_SECRET.to_string());
            return TokenOutcome::replace(format!("{name}{separator}{REDACTED}"));
        }
    }

    // A bare `Bearer` in canonical HTTP capitalization introduces a credential
    // on its own — that is what makes `Bearer qqq` lose `qqq`, which no shape
    // test could ever do. Any other scheme keyword, and any other
    // capitalization, needs something credential-shaped to actually follow:
    // `bearer of bad news` and `Token holders vote` are prose.
    if unwrap_token(token) == SELF_EVIDENT_AUTH_SCHEME {
        return TokenOutcome::keep_and_arm(CredentialArm::Certain);
    }
    if is_auth_scheme(token) && next.is_some_and(looks_like_credential_value) {
        return TokenOutcome::keep_and_arm(CredentialArm::IfShaped);
    }

    let lowered = token.to_ascii_lowercase();
    if SECRET_VALUE_PREFIXES
        .iter()
        .any(|prefix| lowered.starts_with(prefix))
        || looks_like_aws_access_key(token)
    {
        kinds.insert(REDACTION_SECRET.to_string());
        return TokenOutcome::replace(REDACTED.to_string());
    }

    TokenOutcome::keep()
}

/// Strip the punctuation a token picks up from surrounding prose.
fn unwrap_token(token: &str) -> &str {
    token
        .trim_start_matches(['(', '[', '"', '\'', '<'])
        .trim_end_matches([',', '.', ';', ':', ')', ']', '"', '\'', '>', '!', '?'])
}

/// Whether a token is an HTTP authorization scheme keyword (and nothing else),
/// in any capitalization.
fn is_auth_scheme(token: &str) -> bool {
    let word = unwrap_token(token);
    !word.is_empty()
        && word.chars().all(|ch| ch.is_ascii_alphabetic())
        && AUTH_SCHEMES
            .iter()
            .any(|scheme| scheme.eq_ignore_ascii_case(word))
}

/// Whether a token is a scheme keyword spelled the way HTTP spells it —
/// `Bearer`, not `bearer` or `BEARER`.
///
/// This is the evidence that separates syntax from prose. It is a weak signal
/// read honestly: capitalization is *suggestive*, so it upgrades an arming that
/// header context already established, and stands alone only for
/// [`SELF_EVIDENT_AUTH_SCHEME`].
fn is_canonical_auth_scheme(token: &str) -> bool {
    AUTH_SCHEMES.contains(&unwrap_token(token))
}

/// Whether a token has the shape of an opaque credential value.
///
/// Deliberately conservative: long, punctuation-free-ish, and mixing letters
/// with digits. Ordinary words — however long — never qualify, which is what
/// keeps `bearer of responsibility` out of the redactor.
fn looks_like_credential_value(token: &str) -> bool {
    let value = unwrap_token(token);
    if value.chars().count() < CREDENTIAL_VALUE_MIN_LEN {
        return false;
    }
    let mut has_digit = false;
    let mut has_alpha = false;
    for ch in value.chars() {
        if ch.is_ascii_digit() {
            has_digit = true;
        } else if ch.is_ascii_alphabetic() {
            has_alpha = true;
        } else if !matches!(ch, '-' | '_' | '.' | '=' | '+' | '/' | '~') {
            return false;
        }
    }
    has_digit && has_alpha
}

/// Whether a token is an AWS access key id.
///
/// Requires the exact uppercase prefix *and* the full length, so `Asia` and
/// `ASIA` (the continent, in prose or in a shouted heading) are left alone
/// while `ASIA` + 16 key characters is removed.
fn looks_like_aws_access_key(token: &str) -> bool {
    let value = unwrap_token(token);
    if value.len() < AWS_KEY_ID_LEN {
        return false;
    }
    if !AWS_KEY_ID_PREFIXES
        .iter()
        .any(|prefix| value.starts_with(prefix))
    {
        return false;
    }
    value
        .chars()
        .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit())
}

/// A Discord bot token: three base64url segments joined by dots.
static DISCORD_TOKEN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b[MNO][A-Za-z0-9_-]{23,27}\.[A-Za-z0-9_-]{6}\.[A-Za-z0-9_-]{27,40}\b")
        .expect("valid regex")
});

/// `input` with every secret replaced by [`REDACTED`]; everything else,
/// paths included, is kept verbatim.
#[must_use]
pub fn redact_secrets(input: &str) -> String {
    let text = redact_secret_tokens(input).into_text();
    DISCORD_TOKEN.replace_all(&text, REDACTED).into_owned()
}

#[cfg(test)]
#[path = "redact_tests.rs"]
mod redact_tests;
