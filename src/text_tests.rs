use super::*;

#[test]
fn cap_keeps_char_boundaries() {
    assert_eq!(cap("héllo", 2), "h");
    assert_eq!(cap("héllo", 3), "hé");
    assert_eq!(cap("abc", 10), "abc");
}

#[test]
fn cap_tail_keeps_char_boundaries() {
    assert_eq!(cap_tail("abcé", 1), "");
    assert_eq!(cap_tail("abcé", 2), "é");
    assert_eq!(cap_tail("abc", 10), "abc");
}

#[test]
fn head_tail_joins_the_ends_of_long_text() {
    assert_eq!(head_tail("short", 3, 3), "short");
    assert_eq!(head_tail("abcdefghij", 3, 2), "abc … ij");
}

#[test]
fn first_line_skips_blank_lines() {
    assert_eq!(
        first_line("\n  \n  Done: fixed it  \nmore"),
        "Done: fixed it"
    );
    assert_eq!(first_line(""), "");
}

#[test]
fn clip_collapses_whitespace_and_marks_cuts() {
    assert_eq!(clip("a\n  b\tc", 10), "a b c");
    assert_eq!(clip("abcdefgh", 5), "abcd…");
    assert_eq!(clip("abcdefgh", 5).chars().count(), 5);
}

#[test]
fn fnv1a_matches_reference_values() {
    assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
    assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
}
