//! Пароли в CSV: импорт из других браузеров и менеджеров, экспорт в формате
//! Chrome.
//!
//! Столбцы у всех свои: Chrome — `name,url,username,password,note`, Firefox —
//! `"url","username","password",…`, Bitwarden — `login_uri,login_username,
//! login_password`. Поэтому разбор идёт по заголовку, а не по позиции.

use serde::Serialize;

#[derive(Debug, Clone, PartialEq)]
pub struct CsvLogin {
    pub origin: String,
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct CsvReport {
    pub imported: usize,
    /// Строки без адреса сайта, без пароля или с адресом не-веб (android://).
    pub skipped: usize,
}

const URL_COLUMNS: &[&str] = &["url", "login_uri", "website", "origin", "web site", "uri"];
const USER_COLUMNS: &[&str] = &["username", "login_username", "login", "user", "email"];
const PASSWORD_COLUMNS: &[&str] = &["password", "login_password", "pass"];

pub fn parse_logins(text: &str) -> anyhow::Result<(Vec<CsvLogin>, usize)> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut reader = csv::ReaderBuilder::new()
        .flexible(true)
        .trim(csv::Trim::Headers)
        .from_reader(text.as_bytes());

    let headers: Vec<String> = reader
        .headers()?
        .iter()
        .map(|header| header.trim().to_ascii_lowercase())
        .collect();
    let column = |names: &[&str]| headers.iter().position(|h| names.contains(&h.as_str()));

    let (Some(url), Some(password)) = (column(URL_COLUMNS), column(PASSWORD_COLUMNS)) else {
        anyhow::bail!("в файле нет столбцов с адресом сайта и паролем");
    };
    let username = column(USER_COLUMNS);

    let mut logins = Vec::new();
    let mut skipped = 0;
    for record in reader.records() {
        let Ok(record) = record else {
            skipped += 1;
            continue;
        };
        let origin = record.get(url).and_then(origin_of);
        let secret = record.get(password).unwrap_or_default();
        match origin {
            Some(origin) if !secret.is_empty() => logins.push(CsvLogin {
                origin,
                username: username
                    .and_then(|index| record.get(index))
                    .unwrap_or_default()
                    .to_string(),
                password: secret.to_string(),
            }),
            _ => skipped += 1,
        }
    }
    Ok((logins, skipped))
}

/// CSV в формате Chrome: его без настройки понимают Chrome, Edge, Firefox,
/// Bitwarden и 1Password.
pub fn render_logins(logins: &[CsvLogin]) -> anyhow::Result<String> {
    let mut writer = csv::Writer::from_writer(Vec::new());
    writer.write_record(["name", "url", "username", "password", "note"])?;
    for login in logins {
        let name = login
            .origin
            .split_once("://")
            .map_or(login.origin.as_str(), |(_, host)| host);
        writer.write_record([
            name,
            &format!("{}/", login.origin),
            &login.username,
            &login.password,
            "",
        ])?;
    }
    Ok(String::from_utf8(writer.into_inner()?)?)
}

/// `https://Habr.com:443/ru/login?x` → `https://habr.com`.
///
/// Пароль привязан к origin, а не к адресу страницы: форма входа живёт
/// то на `/login`, то на `/auth/…`, а учётка одна.
pub fn origin_of(url: &str) -> Option<String> {
    let (scheme, rest) = url.trim().split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit('@').next()?.to_ascii_lowercase();
    if host.is_empty() {
        return None;
    }
    let host = match (scheme.as_str(), host.rsplit_once(':')) {
        ("https", Some((name, "443"))) | ("http", Some((name, "80"))) => name.to_string(),
        _ => host,
    };
    Some(format!("{scheme}://{host}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_is_normalized() {
        assert_eq!(
            origin_of("https://Habr.com:443/ru/login?next=/").as_deref(),
            Some("https://habr.com")
        );
        assert_eq!(
            origin_of("http://user:pw@localhost:5173/").as_deref(),
            Some("http://localhost:5173")
        );
        assert_eq!(origin_of("android://hash@com.app/"), None);
        assert_eq!(origin_of("habr.com"), None);
    }

    #[test]
    fn chrome_and_firefox_exports_are_read() {
        let chrome = "\u{feff}name,url,username,password,note\n\
            habr.com,https://habr.com/ru/auth/,me@example.com,\"p,a\"\"ss\",\n\
            app,android://x@com.app/,me,secret,\n\
            empty,https://empty.example/,me,,\n";
        let (logins, skipped) = parse_logins(chrome).unwrap();
        assert_eq!(skipped, 2);
        assert_eq!(
            logins,
            vec![CsvLogin {
                origin: "https://habr.com".into(),
                username: "me@example.com".into(),
                password: "p,a\"ss".into(),
            }]
        );

        let firefox =
            "\"url\",\"username\",\"password\",\"httpRealm\",\"formActionOrigin\",\"guid\"\r\n\
            \"https://github.com\",\"octo\",\"hunter2\",,\"https://github.com\",\"{1}\"\r\n";
        let (logins, _) = parse_logins(firefox).unwrap();
        assert_eq!(logins[0].origin, "https://github.com");
        assert_eq!(logins[0].password, "hunter2");
    }

    #[test]
    fn export_is_readable_back() {
        let logins = vec![CsvLogin {
            origin: "https://habr.com".into(),
            username: "me".into(),
            password: "a,b\"c".into(),
        }];
        let text = render_logins(&logins).unwrap();
        assert_eq!(parse_logins(&text).unwrap().0, logins);
    }

    #[test]
    fn unrelated_csv_is_rejected() {
        assert!(parse_logins("a,b,c\n1,2,3\n").is_err());
    }
}
