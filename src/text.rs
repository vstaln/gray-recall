//! Small string helpers shared by extraction, indexing and rendering.

/// Longest prefix of `s` that is at most `max` bytes and ends on a char boundary.
pub fn cap(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Longest suffix of `s` that is at most `max` bytes and starts on a char boundary.
pub fn cap_tail(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut start = s.len() - max;
    while !s.is_char_boundary(start) {
        start += 1;
    }
    &s[start..]
}

/// `s` if it fits in `head + tail` bytes, else its first `head` and last
/// `tail` bytes joined by ` … `.
pub fn head_tail(s: &str, head: usize, tail: usize) -> String {
    if s.len() <= head + tail {
        return s.to_string();
    }
    format!("{} … {}", cap(s, head), cap_tail(s, tail))
}

/// The first line of `s` that is not blank, trimmed.
pub fn first_line(s: &str) -> &str {
    s.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
}

/// Whitespace runs collapsed to one space, cut to `max` chars with a trailing `…`.
pub fn clip(s: &str, max: usize) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let mut out: String = flat.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// FNV-1a, 64-bit: the change-detection hash for catch-up's byte windows.
pub fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3)
    })
}

#[cfg(test)]
#[path = "text_tests.rs"]
mod text_tests;
