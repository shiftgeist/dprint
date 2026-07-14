/// The maximum number of bytes to read from the start of an extensionless file
/// when looking for a shebang. A shebang line is short in practice, so this is a
/// generous upper bound that keeps the read cheap while still capturing the full
/// line (only the leading portion is needed for prefix matching anyway).
pub const SHEBANG_READ_BYTES: usize = 1024;

/// Normalizes a shebang line for matching by trimming surrounding whitespace and
/// collapsing runs of inter-token whitespace down to a single space.
///
/// This lets a configured key like `#!/usr/bin/env -S deno run` match a file
/// whose shebang uses different spacing between tokens.
pub fn normalize_shebang_line(line: &str) -> String {
  let mut result = String::with_capacity(line.len());
  for (i, token) in line.split_whitespace().enumerate() {
    if i > 0 {
      result.push(' ');
    }
    result.push_str(token);
  }
  result
}

/// Extracts and normalizes the shebang line from the start of a file's bytes.
///
/// Returns `None` when the content does not start with `#!`. Only the first line
/// is considered and it is normalized with [`normalize_shebang_line`].
pub fn parse_shebang_line(prefix: &[u8]) -> Option<String> {
  if !prefix.starts_with(b"#!") {
    return None;
  }
  let line_end = prefix.iter().position(|&b| b == b'\n').unwrap_or(prefix.len());
  let line = String::from_utf8_lossy(&prefix[..line_end]);
  Some(normalize_shebang_line(&line))
}

/// Returns whether a normalized shebang line starts with the given normalized
/// key at a whitespace boundary (or matches it exactly).
pub fn shebang_line_matches_key(normalized_line: &str, normalized_key: &str) -> bool {
  match normalized_line.strip_prefix(normalized_key) {
    Some(rest) => rest.is_empty() || rest.starts_with(' '),
    None => false,
  }
}

#[cfg(test)]
mod test {
  use super::*;

  #[test]
  fn normalizes_whitespace() {
    assert_eq!(normalize_shebang_line("  #!/usr/bin/env   -S  deno run  "), "#!/usr/bin/env -S deno run");
    assert_eq!(normalize_shebang_line("#!/bin/sh"), "#!/bin/sh");
  }

  #[test]
  fn parses_shebang_line() {
    assert_eq!(parse_shebang_line(b"#!/bin/sh\necho hi\n").unwrap(), "#!/bin/sh");
    assert_eq!(parse_shebang_line(b"#!/usr/bin/env  bash").unwrap(), "#!/usr/bin/env bash");
    // handles a trailing carriage return
    assert_eq!(parse_shebang_line(b"#!/bin/sh\r\n").unwrap(), "#!/bin/sh");
    assert!(parse_shebang_line(b"not a shebang").is_none());
    assert!(parse_shebang_line(b"").is_none());
  }

  #[test]
  fn matches_key_at_boundary() {
    assert!(shebang_line_matches_key("#!/usr/bin/env -S deno run --allow-net", "#!/usr/bin/env -S deno run"));
    assert!(shebang_line_matches_key("#!/bin/sh", "#!/bin/sh"));
    // must be a whitespace boundary, not a partial token
    assert!(!shebang_line_matches_key("#!/usr/bin/env node-thing", "#!/usr/bin/env node"));
    assert!(!shebang_line_matches_key("#!/bin/sh", "#!/bin/bash"));
  }
}
