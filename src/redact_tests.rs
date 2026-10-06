use super::*;

fn fake(prefix: &str) -> String {
    format!("{prefix}{}", "Zq3xK9".repeat(6))
}

fn discord_token() -> String {
    let first = "Tk4Lm".repeat(5);
    format!("M{}.{}.{}", &first[..23], "Gh7k2a", "Q9w".repeat(10))
}

#[test]
fn provider_keys_are_redacted() {
    for prefix in ["sk-", "ghp_", "xoxb-"] {
        let key = fake(prefix);
        let out = redact_secrets(&format!("use {key} here"));
        assert!(!out.contains(&key), "{prefix} leaked: {out}");
        assert!(out.contains(REDACTED), "{out}");
        assert!(out.starts_with("use ") && out.ends_with(" here"), "{out}");
    }
}

#[test]
fn named_assignments_are_redacted() {
    let value = "Zq3xK9Zq3xK9Zq3xK9";
    let out = redact_secrets(&format!("export DISCORD_BOT_TOKEN={value}"));
    assert!(!out.contains(value), "{out}");
}

#[test]
fn bare_discord_tokens_are_redacted() {
    let token = discord_token();
    let out = redact_secrets(&format!("the bot uses {token} now"));
    assert!(!out.contains(&token), "{out}");
    assert!(out.starts_with("the bot uses "), "{out}");
}

#[test]
fn bearer_values_are_redacted() {
    let out = redact_secrets("Authorization: Bearer qqq");
    assert!(!out.contains("qqq"), "{out}");
}

#[test]
fn paths_and_prose_survive() {
    let s = "edited crates/gray/src/lib.rs and /home/x/notes.md; basic auth is fine";
    assert_eq!(redact_secrets(s), s);
}

#[test]
fn every_line_of_multiline_input_is_checked() {
    let key = fake("sk-");
    let out = redact_secrets(&format!("line one\nkey {key}\nline three"));
    assert!(!out.contains(&key), "{out}");
    assert!(
        out.starts_with("line one\n") && out.ends_with("\nline three"),
        "{out}"
    );
}
