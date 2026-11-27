//! Heuristics that spot files and lines that look like credentials before they are committed.

/// A file name that usually holds secrets.
pub fn suspicious_file(path: &str) -> Option<&'static str> {
    let name = path.rsplit('/').next().unwrap_or(path).to_lowercase();
    if name == ".env"
        || (name.starts_with(".env.") && !name.ends_with(".example") && !name.ends_with(".sample"))
    {
        return Some("environment file with secrets");
    }
    if name == "id_rsa" || name == "id_ed25519" || name == "id_ecdsa" || name == "id_dsa" {
        return Some("private SSH key");
    }
    if [".pem", ".pfx", ".p12", ".key", ".keystore", ".jks"]
        .iter()
        .any(|e| name.ends_with(e))
    {
        return Some("key or certificate file");
    }
    if name == "credentials.json" || name == "service-account.json" || name == ".npmrc" || name == ".pgpass" {
        return Some("credentials file");
    }
    None
}

fn is_placeholder(v: &str) -> bool {
    let l = v.to_lowercase();
    [
        "env(",
        "config(",
        "$",
        "{{",
        "<",
        "your",
        "xxx",
        "example",
        "changeme",
        "null",
        "none",
        "true",
        "false",
        "*",
        "process.env",
        "(",
        ")",
    ]
    .iter()
    .any(|p| l.contains(p))
}

/// Why this (added) line looks like a secret, if it does.
pub fn scan_line(line: &str) -> Option<&'static str> {
    if line.contains("-----BEGIN") && line.contains("PRIVATE KEY") {
        return Some("private key");
    }
    if let Some(i) = line.find("AKIA") {
        let tail: Vec<char> = line[i + 4..].chars().take(16).collect();
        if tail.len() == 16 && tail.iter().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()) {
            return Some("AWS access key");
        }
    }
    if let Some(i) = line.find("ghp_")
        && line[i + 4..]
            .chars()
            .take_while(char::is_ascii_alphanumeric)
            .count()
            >= 30
    {
        return Some("GitHub token");
    }
    if line.contains("sk_live_") {
        return Some("Stripe live key");
    }
    let lower = line.to_lowercase();
    for kw in [
        "password",
        "passwd",
        "secret",
        "api_key",
        "apikey",
        "access_token",
        "auth_token",
        "private_key",
    ] {
        let Some(pos) = lower.find(kw) else { continue };
        let rest = &line[pos + kw.len()..];
        let Some(eq) = rest.find(['=', ':']) else { continue };
        if eq > 6 {
            continue;
        }
        let value: String = rest[eq + 1..]
            .trim_start_matches(['=', '>', ' ', '\'', '"'])
            .chars()
            .take_while(|c| !c.is_whitespace() && !matches!(c, '\'' | '"' | ',' | ';'))
            .collect();
        if value.len() >= 8 && !is_placeholder(&value) {
            return Some("hard-coded secret");
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_real_secrets_but_not_code() {
        assert_eq!(scan_line("DB_PASSWORD=supersecret123"), Some("hard-coded secret"));
        assert_eq!(
            scan_line("'password' => 'hunter2hunter2',"),
            Some("hard-coded secret")
        );
        assert_eq!(scan_line("key = AKIAIOSFODNN7EXAMPLE"), Some("AWS access key"));
        assert_eq!(scan_line("-----BEGIN RSA PRIVATE KEY-----"), Some("private key"));
        assert_eq!(scan_line("$password = $request->input('password');"), None);
        assert_eq!(scan_line("'password' => Hash::make($value),"), None);
        assert_eq!(scan_line("password: 'abc'"), None);
        assert_eq!(scan_line("APP_KEY=${APP_KEY_VALUE}"), None);
    }

    #[test]
    fn flags_secret_files() {
        assert!(suspicious_file("config/.env").is_some());
        assert!(suspicious_file("keys/id_rsa").is_some());
        assert!(suspicious_file(".env.example").is_none());
        assert!(suspicious_file("src/main.rs").is_none());
    }
}
