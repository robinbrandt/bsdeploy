/// Shell command building utilities for safe command construction.
///
/// This module provides utilities to safely construct shell commands
/// by properly escaping user-controlled input to prevent command injection.

/// Escape a string for safe use in a POSIX shell command.
///
/// Uses single-quote escaping: wraps the string in single quotes and
/// escapes any embedded single quotes using the `'\''` pattern.
///
/// # Examples
/// ```
/// use bsdeploy::shell::escape;
/// assert_eq!(escape("hello"), "'hello'");
/// assert_eq!(escape("it's"), "'it'\\''s'");
/// assert_eq!(escape(""), "''");
/// ```
pub fn escape(s: &str) -> String {
    // If empty, return empty quotes
    if s.is_empty() {
        return "''".to_string();
    }

    // Check if the string needs escaping at all
    // Safe characters that don't need quoting: alphanumeric, underscore, hyphen, dot, slash, colon
    let needs_escaping = s.chars().any(|c| {
        !c.is_ascii_alphanumeric() && c != '_' && c != '-' && c != '.' && c != '/' && c != ':'
    });

    if !needs_escaping {
        return s.to_string();
    }

    // Use single-quote escaping
    let mut result = String::with_capacity(s.len() + 10);
    result.push('\'');
    for c in s.chars() {
        if c == '\'' {
            // End single quote, add escaped single quote, start new single quote
            result.push_str("'\\''");
        } else {
            result.push(c);
        }
    }
    result.push('\'');
    result
}

/// Escape a string for use in an environment variable export statement.
///
/// This is specifically for values used in: export VAR='value'
/// The value is already expected to be placed inside single quotes by the caller,
/// so we only need to escape single quotes within the value.
pub fn escape_env_value(s: &str) -> String {
    s.replace('\'', "'\\''")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_escape_simple() {
        assert_eq!(escape("hello"), "hello");
        assert_eq!(escape("world123"), "world123");
    }

    #[test]
    fn test_escape_with_special_chars() {
        assert_eq!(escape("hello world"), "'hello world'");
        assert_eq!(escape("test@example"), "'test@example'");
        assert_eq!(escape("$HOME"), "'$HOME'");
    }

    #[test]
    fn test_escape_with_single_quote() {
        assert_eq!(escape("it's"), "'it'\\''s'");
        assert_eq!(escape("'quoted'"), "''\\''quoted'\\'''");
    }

    #[test]
    fn test_escape_empty() {
        assert_eq!(escape(""), "''");
    }

    #[test]
    fn test_escape_path_like() {
        assert_eq!(escape("/var/lib/app"), "/var/lib/app");
        assert_eq!(escape("file.txt"), "file.txt");
        assert_eq!(escape("my-app_v1.0"), "my-app_v1.0");
    }

    #[test]
    fn test_escape_dangerous_input() {
        // These should all be safely escaped
        assert_eq!(escape("; rm -rf /"), "'; rm -rf /'");
        assert_eq!(escape("$(whoami)"), "'$(whoami)'");
        assert_eq!(escape("`id`"), "'`id`'");
        assert_eq!(escape("foo && bar"), "'foo && bar'");
        assert_eq!(escape("foo | bar"), "'foo | bar'");
        assert_eq!(escape("foo > /etc/passwd"), "'foo > /etc/passwd'");
    }

    #[test]
    fn test_escape_env_value() {
        assert_eq!(escape_env_value("simple"), "simple");
        assert_eq!(escape_env_value("it's"), "it'\\''s");
        assert_eq!(escape_env_value("no'quotes'here"), "no'\\''quotes'\\''here");
    }

    #[test]
    fn test_escape_unicode() {
        // Unicode characters should be quoted
        assert_eq!(escape("héllo"), "'héllo'");
        assert_eq!(escape("日本語"), "'日本語'");
    }

    #[test]
    fn test_escape_newlines_and_tabs() {
        assert_eq!(escape("line1\nline2"), "'line1\nline2'");
        assert_eq!(escape("col1\tcol2"), "'col1\tcol2'");
    }
}
