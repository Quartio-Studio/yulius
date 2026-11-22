//! Allow rules of an agent secret (SPEC-627, ADR-0458): the HTTP methods and path patterns of the
//! requests the egress proxy may put its value into. No rules: any request to a bound host.

use super::VaultError;

/// Rules one secret may have.
pub const MAX_RULES: usize = 32;
/// Methods one rule may list.
pub const MAX_RULE_METHODS: usize = 16;
/// Longest path pattern in bytes.
pub const MAX_PATTERN_BYTES: usize = 512;
/// Longest method name.
const MAX_METHOD_BYTES: usize = 32;

/// Headers some servers take over the request's method, so a `GET` rule would let a `DELETE`
/// through: a request that uses a secret with rules may not carry them (MPR-004).
pub const METHOD_OVERRIDE_HEADERS: &[&str] = &[
    "x-http-method-override",
    "x-http-method",
    "x-method-override",
];

/// One allow rule: a request whose method is one of `methods` and whose path matches `path`
/// (`*` matching any run of characters, `/` included).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretRule {
    methods: Vec<String>,
    path: String,
}

impl SecretRule {
    /// Parses a rule (methods upper-cased, duplicates removed).
    ///
    /// # Errors
    /// [`VaultError::Rule`] for no or too many methods, a method that is no HTTP token, or a
    /// path pattern that is not `/…` printable ASCII without spaces, `?`, `#`, `..`, `\` or a
    /// percent-encoded `.`, `\` or NUL.
    pub fn parse(methods: &[String], path: &str) -> Result<Self, VaultError> {
        let mut out: Vec<String> = Vec::new();
        for method in methods {
            let upper = method.trim().to_ascii_uppercase();
            if upper.is_empty()
                || upper.len() > MAX_METHOD_BYTES
                || !upper
                    .bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
            {
                return Err(VaultError::Rule(format!(
                    "invalid method {method:?}: an HTTP method such as GET or POST"
                )));
            }
            if !out.contains(&upper) {
                out.push(upper);
            }
        }
        if out.is_empty() || out.len() > MAX_RULE_METHODS {
            return Err(VaultError::Rule(format!(
                "a rule lists 1 to {MAX_RULE_METHODS} methods (for {path:?})"
            )));
        }
        let path = path.trim();
        let printable = path
            .bytes()
            .all(|b| b.is_ascii_graphic() && b != b'?' && b != b'#');
        if !path.starts_with('/')
            || path.len() > MAX_PATTERN_BYTES
            || !printable
            || unsafe_path(path).is_some()
        {
            return Err(VaultError::Rule(format!(
                "invalid path {path:?}: a path starting with / (at most {MAX_PATTERN_BYTES} \
                 characters, * for any characters, no query), without spaces, .., \\ or \
                 encoded dots"
            )));
        }
        Ok(Self {
            methods: out,
            path: path.to_owned(),
        })
    }

    /// Its methods, upper-case.
    #[must_use]
    pub fn methods(&self) -> &[String] {
        &self.methods
    }

    /// Its path pattern.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Whether a request with `method` (as sent: compared exactly) to `path` (without the query)
    /// is allowed.
    #[must_use]
    pub fn matches(&self, method: &str, path: &str) -> bool {
        self.methods.iter().any(|m| m == method) && glob(&self.path, path)
    }
}

impl std::fmt::Display for SecretRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.methods.join(", "), self.path)
    }
}

/// Parses a secret's rules: at most [`MAX_RULES`], duplicates removed. An empty list is no rules.
///
/// # Errors
/// [`VaultError::Rule`] for too many rules or a bad one.
pub fn parse_rules(rules: &[(Vec<String>, String)]) -> Result<Vec<SecretRule>, VaultError> {
    if rules.len() > MAX_RULES {
        return Err(VaultError::Rule(format!("at most {MAX_RULES} rules")));
    }
    let mut out: Vec<SecretRule> = Vec::new();
    for (methods, path) in rules {
        let rule = SecretRule::parse(methods, path)?;
        if !out.contains(&rule) {
            out.push(rule);
        }
    }
    Ok(out)
}

/// The path of a request's `path_and_query` (before `?`).
#[must_use]
pub fn request_path(path_and_query: &str) -> &str {
    path_and_query
        .split_once('?')
        .map_or(path_and_query, |(path, _)| path)
}

/// Whether `rules` let a request with `method` to `path` (without the query) use their secret:
/// always without rules.
#[must_use]
pub fn rules_allow(rules: &[SecretRule], method: &str, path: &str) -> bool {
    rules.is_empty() || rules.iter().any(|r| r.matches(method, path))
}

/// Why a request path could reach another resource than the one a rule matched once the server
/// normalises it (`..`, `\`, percent-encoded `.`, `\` or NUL; MPR-004), or `None`.
#[must_use]
pub fn unsafe_path(path: &str) -> Option<&'static str> {
    let lowered = path.to_ascii_lowercase();
    if lowered.contains("..") || lowered.contains("%2e") {
        Some("it holds a dot segment or an encoded dot")
    } else if lowered.contains('\\') || lowered.contains("%5c") {
        Some("it holds a backslash")
    } else if lowered.contains("%00") {
        Some("it holds an encoded NUL")
    } else {
        None
    }
}

/// Whether `text` matches `pattern` whole, `*` matching any run of characters (also none and
/// `/`). Linear in `text` for each literal piece: the first piece is a prefix, the last a suffix,
/// and the middle ones are found leftmost in order, which is enough for `*` alone.
fn glob(pattern: &str, text: &str) -> bool {
    let pieces: Vec<&str> = pattern.split('*').collect();
    let (first, last) = match pieces.as_slice() {
        [only] => return *only == text,
        [first, .., last] => (*first, *last),
        [] => return text.is_empty(),
    };
    if text.len() < first.len() + last.len() || !text.starts_with(first) || !text.ends_with(last) {
        return false;
    }
    let mut rest = &text[first.len()..text.len() - last.len()];
    for piece in &pieces[1..pieces.len() - 1] {
        match rest.find(piece) {
            Some(at) => rest = &rest[at + piece.len()..],
            None => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(methods: &[&str], path: &str) -> Result<SecretRule, VaultError> {
        let methods: Vec<String> = methods.iter().map(|m| (*m).to_owned()).collect();
        SecretRule::parse(&methods, path)
    }

    #[test]
    fn a_star_matches_across_slashes_and_the_rest_is_exact() -> Result<(), VaultError> {
        let read = rule(&["get", "HEAD", "GET"], "/repos/*")?;
        assert_eq!(read.methods(), ["GET", "HEAD"]);
        assert_eq!(read.to_string(), "GET, HEAD /repos/*");
        assert!(read.matches("GET", "/repos/acme/app/issues"));
        assert!(read.matches("HEAD", "/repos/"));
        assert!(!read.matches("POST", "/repos/acme/app/issues"));
        assert!(
            !read.matches("get", "/repos/acme"),
            "methods are compared as sent"
        );
        assert!(
            !read.matches("GET", "/repos"),
            "the slash is part of the pattern"
        );
        assert!(
            !read.matches("GET", "/user/repos/x"),
            "a pattern matches the whole path"
        );
        assert!(!read.matches("GET", "/Repos/x"), "case-sensitive");

        let middle = rule(&["POST"], "/v1/*/messages/*.json")?;
        assert!(middle.matches("POST", "/v1/a/b/messages/c/d.json"));
        assert!(middle.matches("POST", "/v1//messages/.json"));
        assert!(!middle.matches("POST", "/v1/a/messages/c.jsonx"));
        assert!(
            !middle.matches("POST", "/v1/messages/c.json"),
            "both literal slashes needed"
        );

        let exact = rule(&["GET"], "/user")?;
        assert!(exact.matches("GET", "/user"));
        assert!(!exact.matches("GET", "/user/emails"));
        let stars = rule(&["GET"], "/a**b")?;
        assert!(stars.matches("GET", "/ab") && stars.matches("GET", "/a/x/b"));
        // The first and the last literal piece may not overlap.
        let overlap = rule(&["GET"], "/ab*ba")?;
        assert!(!overlap.matches("GET", "/aba"));
        assert!(overlap.matches("GET", "/abba"));
        Ok(())
    }

    #[test]
    fn no_rules_allow_everything_and_rules_allow_only_their_requests() -> Result<(), VaultError> {
        assert!(rules_allow(&[], "DELETE", "/anything"));
        let rules = parse_rules(&[
            (vec!["GET".into()], "/v1/*".into()),
            (vec!["POST".into()], "/v1/search".into()),
            (vec!["get".into()], "/v1/*".into()),
        ])?;
        assert_eq!(rules.len(), 2, "duplicates removed");
        assert!(rules_allow(&rules, "GET", "/v1/items"));
        assert!(rules_allow(&rules, "POST", "/v1/search"));
        assert!(!rules_allow(&rules, "POST", "/v1/items"));
        assert!(!rules_allow(&rules, "GET", "/v2/items"));
        assert_eq!(request_path("/v1/items?page=2&q=a?b"), "/v1/items");
        assert_eq!(request_path("/v1/items"), "/v1/items");
        Ok(())
    }

    #[test]
    fn refuses_malformed_rules() {
        for (methods, path) in [
            (&[][..], "/v1/*"),
            (&[""][..], "/v1/*"),
            (&["GE T"][..], "/v1/*"),
            (&["*"][..], "/v1/*"),
            (&["GET"][..], "v1/*"),
            (&["GET"][..], ""),
            (&["GET"][..], "/v1/*?x=1"),
            (&["GET"][..], "/v1/#x"),
            (&["GET"][..], "/v1 /x"),
            (&["GET"][..], "/v1/../admin"),
            (&["GET"][..], "/v1/%2E%2e/admin"),
            (&["GET"][..], "/v1\\admin"),
            (&["GET"][..], "/v1/%5Cadmin"),
            (&["GET"][..], "/v1/\u{e9}"),
        ] {
            assert!(rule(methods, path).is_err(), "{methods:?} {path:?}");
        }
        let long = format!("/{}", "a".repeat(MAX_PATTERN_BYTES));
        assert!(rule(&["GET"], &long).is_err());
        let many: Vec<String> = (0..=MAX_RULE_METHODS).map(|i| format!("M{i}")).collect();
        assert!(SecretRule::parse(&many, "/").is_err());
        let rules: Vec<(Vec<String>, String)> = (0..=MAX_RULES)
            .map(|i| (vec!["GET".into()], format!("/r{i}")))
            .collect();
        assert!(parse_rules(&rules).is_err());
    }

    #[test]
    fn paths_a_server_could_normalise_elsewhere_are_flagged() {
        for bad in [
            "/v1/../admin",
            "/v1/..",
            "/v1/%2e%2e/admin",
            "/v1/.%2E/admin",
            "/v1/x%2Eadmin",
            "/v1\\..\\admin",
            "/v1/%5c",
            "/v1/admin%00/read",
        ] {
            assert!(unsafe_path(bad).is_some(), "{bad}");
        }
        for fine in [
            "/v1/a.b/c",
            "/api/v4/projects/group%2Fproject",
            "/v1/./x",
            "/",
        ] {
            assert!(unsafe_path(fine).is_none(), "{fine}");
        }
    }
}
