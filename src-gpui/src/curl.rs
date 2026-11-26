use serde_json::{Value, json};

pub struct Import {
    pub request: Value,
    pub skipped: Vec<String>,
}

pub fn export(request: &Value) -> String {
    let method = request["method"].as_str().unwrap_or("GET");
    let mut lines = vec![format!("curl --request {method}")];
    if request["settings"]["ssl_verify"] == false {
        lines.push("  --insecure".into());
    }
    lines.push(format!("  --url {}", quote(text(&request["url"]))));
    for header in rows(&request["headers"]) {
        lines.push(format!(
            "  --header {}",
            quote(&format!(
                "{}: {}",
                text(&header["key"]),
                text(&header["value"])
            ))
        ));
    }
    if let Some(fields) = request["form"].as_array().filter(|rows| !rows.is_empty()) {
        for field in fields {
            let prefix = if field["type"] == "file" { "@" } else { "" };
            lines.push(format!(
                "  --form {}",
                quote(&format!(
                    "{}={prefix}{}",
                    text(&field["key"]),
                    text(&field["value"])
                ))
            ));
        }
    } else if let Some(body) = request["body"].as_str().filter(|body| !body.is_empty()) {
        lines.push(format!("  --data-raw {}", quote(body)));
    }
    lines.join(" \\\n")
}

pub fn import(source: &str) -> Result<Import, String> {
    let clean = source
        .replace("\r\n", "\n")
        .lines()
        .filter(|line| !line.trim().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");
    let mut tokens = tokenize(&clean)?.into_iter();
    let executable = tokens.next().unwrap_or_default().to_ascii_lowercase();
    if !(executable.ends_with("curl") || executable.ends_with("curl.exe")) {
        return Err("The command has to start with curl.".into());
    }
    let mut found = Found::default();
    while let Some(token) = tokens.next() {
        if token.is_empty() {
            continue;
        }
        if !token.starts_with('-') || token == "-" {
            if found.url.is_empty() {
                found.url = token;
            } else {
                remember(&mut found.ignored, &token);
            }
            continue;
        }
        for (key, inline) in options(&token) {
            let Some((role, takes_value)) = flag(&key) else {
                remember(&mut found.ignored, &key);
                continue;
            };
            if !takes_value && inline.is_some() {
                remember(&mut found.ignored, &key);
                continue;
            }
            let value = if takes_value {
                inline.or_else(|| tokens.next()).unwrap_or_default()
            } else {
                String::new()
            };
            match key.as_str() {
                "--data-urlencode" => remember(&mut found.notes, "--data-urlencode sent as typed"),
                "-c" | "--cookie-jar" => {
                    remember(&mut found.notes, "cookie-jar flags are not applied")
                }
                "-x" | "--proxy" => remember(&mut found.notes, "proxy flags are not applied"),
                _ => {}
            }
            match role {
                Role::Method => {
                    let value = value.to_uppercase();
                    found.method = Some(value.strip_prefix("-X").unwrap_or(&value).to_owned());
                }
                Role::Url => found.url = value,
                Role::Header => found.headers.push(value),
                Role::Data => found.data.push(value),
                Role::Form => found.form.push(value),
                Role::User => found.user = value,
                Role::Cookie if value.contains('=') => found.cookie = value,
                Role::Cookie => remember(&mut found.notes, "cookie files are not imported"),
                Role::UserAgent => found.user_agent = value,
                Role::Referer => found.referer = value,
                Role::Get => found.get = true,
                Role::Ignore => remember(&mut found.ignored, &key),
            }
        }
    }
    found.finish()
}

#[derive(Default)]
struct Found {
    method: Option<String>,
    url: String,
    headers: Vec<String>,
    data: Vec<String>,
    form: Vec<String>,
    user: String,
    cookie: String,
    user_agent: String,
    referer: String,
    get: bool,
    ignored: Vec<String>,
    notes: Vec<String>,
}

impl Found {
    fn finish(mut self) -> Result<Import, String> {
        if self.url.is_empty() {
            return Err("The cURL command has no URL.".into());
        }
        for (name, value) in [
            ("Cookie", &self.cookie),
            ("User-Agent", &self.user_agent),
            ("Referer", &self.referer),
        ] {
            if !value.is_empty() && !has_header(&self.headers, name) {
                self.headers.push(format!("{name}: {value}"));
            }
        }
        let data = self.data.join("&");
        if self.get && !data.is_empty() {
            self.url
                .push(if self.url.contains('?') { '&' } else { '?' });
            self.url.push_str(&data);
        }
        let method = self.method.unwrap_or_else(|| {
            if self.get || data.is_empty() && self.form.is_empty() {
                "GET"
            } else {
                "POST"
            }
            .into()
        });
        let headers: Vec<_> = self
            .headers
            .iter()
            .map(|header| {
                let (key, value) = header.split_once(':').unwrap_or((header, ""));
                json!({"key":key.trim(),"value":value.trim(),"enabled":true})
            })
            .collect();
        let (username, password) = self.user.split_once(':').unwrap_or((&self.user, ""));
        let auth = if self.user.is_empty() {
            json!({"type":"noauth"})
        } else {
            json!({"type":"basic","basic":{"username":username,"password":password}})
        };
        let body = if !self.form.is_empty() {
            let fields: Vec<_> = self
                .form
                .iter()
                .map(|field| {
                    let (key, value) = field.split_once('=').unwrap_or((field, ""));
                    if let Some(path) = value.strip_prefix('@') {
                        json!({"key":key,"value":path,"type":"file","enabled":true})
                    } else {
                        json!({"key":key,"value":value,"type":"text","enabled":true})
                    }
                })
                .collect();
            json!({"mode":"form-data","form":fields})
        } else if !self.get && !data.is_empty() {
            let mime = if has_header(&self.headers, "content-type") {
                ""
            } else {
                "application/x-www-form-urlencoded"
            };
            json!({"mode":"raw","raw":data,"contentType":mime})
        } else {
            json!({"mode":"none"})
        };
        let mut url = self.url.split('?');
        let base = url.next().unwrap_or_default();
        let params: Vec<_> = url
            .next()
            .unwrap_or_default()
            .split('&')
            .filter(|pair| !pair.is_empty())
            .map(|pair| {
                let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
                json!({"key":crate::postman::decode_uri_component(key),
                "value":crate::postman::decode_uri_component(value),"enabled":true})
            })
            .collect();
        self.ignored.extend(self.notes);
        Ok(Import {
            request: json!({
                "id":uuid::Uuid::new_v4().to_string(),"kind":"request","name":"Untitled Request",
                "method":method,"url":base,"params":params,"headers":headers,"body":body,"auth":auth,
            }),
            skipped: self.ignored,
        })
    }
}

#[derive(Clone, Copy)]
enum Role {
    Method,
    Url,
    Header,
    Data,
    Form,
    User,
    Cookie,
    UserAgent,
    Referer,
    Get,
    Ignore,
}

fn flag(name: &str) -> Option<(Role, bool)> {
    use Role::*;
    Some(match name {
        "-X" | "--request" => (Method, true),
        "--url" => (Url, true),
        "-H" | "--header" => (Header, true),
        "-d" | "--data" | "--data-raw" | "--data-binary" | "--data-ascii" | "--data-urlencode" => {
            (Data, true)
        }
        "-F" | "--form" => (Form, true),
        "-u" | "--user" => (User, true),
        "-b" | "--cookie" => (Cookie, true),
        "-A" | "--user-agent" => (UserAgent, true),
        "-e" | "--referer" => (Referer, true),
        "-G" | "--get" => (Get, false),
        "-g" | "--globoff" | "-s" | "--silent" | "-S" | "--show-error" | "-L" | "--location"
        | "-k" | "--insecure" | "-i" | "--include" | "-I" | "--head" | "-v" | "--verbose"
        | "-f" | "--fail" | "--compressed" | "--location-trusted" => (Ignore, false),
        "-o" | "--output" | "-m" | "--max-time" | "--connect-timeout" | "-c" | "--cookie-jar"
        | "-x" | "--proxy" | "--cacert" | "--cert" | "--key" | "--resolve" | "--retry" => {
            (Ignore, true)
        }
        _ => return None,
    })
}

fn options(token: &str) -> Vec<(String, Option<String>)> {
    if token.starts_with("--") {
        return vec![
            token
                .split_once(['=', ':'])
                .map_or((token.into(), None), |(name, value)| {
                    (name.into(), Some(value.into()))
                }),
        ];
    }
    let Some(prefix) = token.get(..2) else {
        return vec![(token.into(), None)];
    };
    match flag(prefix) {
        None => vec![(token.into(), None)],
        Some((_, true)) => vec![(
            prefix.into(),
            token
                .get(2..)
                .filter(|tail| !tail.is_empty())
                .map(str::to_owned),
        )],
        Some((_, false)) => {
            let flags: Vec<_> = token[1..].chars().map(|char| format!("-{char}")).collect();
            if flags
                .iter()
                .all(|flag_name| matches!(flag(flag_name), Some((_, false))))
            {
                flags.into_iter().map(|flag| (flag, None)).collect()
            } else {
                vec![(token.into(), None)]
            }
        }
    }
}

fn tokenize(source: &str) -> Result<Vec<String>, String> {
    let mut tokens = Vec::new();
    let mut chars = source.chars().peekable();
    let mut token = String::new();
    let mut started = false;
    while let Some(character) = chars.next() {
        match character {
            ' ' | '\t' | '\n' | '\r' => {
                if started {
                    tokens.push(std::mem::take(&mut token));
                    started = false;
                }
            }
            '\\' if chars.peek().is_some() => {
                let follow = chars
                    .clone()
                    .take_while(|c| c.is_whitespace())
                    .collect::<String>();
                if follow.contains('\n') {
                    for _ in follow.chars() {
                        chars.next();
                    }
                    if started {
                        tokens.push(std::mem::take(&mut token));
                        started = false;
                    }
                } else if let Some(next) = chars.next() {
                    token.push(next);
                    started = true;
                }
            }
            '\'' | '"' => {
                started = true;
                loop {
                    let Some(current) = chars.next() else {
                        return Err(format!(
                            "Unclosed {} quote in the cURL command.",
                            if character == '\'' {
                                "single"
                            } else {
                                "double"
                            }
                        ));
                    };
                    if current == character {
                        break;
                    }
                    if character == '"'
                        && current == '\\'
                        && matches!(chars.peek(), Some('"' | '\\' | '$' | '\u{60}'))
                    {
                        if let Some(escaped) = chars.next() {
                            token.push(escaped);
                        }
                    } else {
                        token.push(current);
                    }
                }
            }
            _ => {
                started = true;
                token.push(character);
            }
        }
    }
    if started {
        tokens.push(token);
    }
    Ok(tokens)
}

fn has_header(headers: &[String], target: &str) -> bool {
    headers.iter().any(|line| {
        line.split_once(':')
            .map_or(line.as_str(), |(key, _)| key)
            .trim()
            .eq_ignore_ascii_case(target)
    })
}
fn remember(list: &mut Vec<String>, value: &str) {
    if !value.is_empty() && !list.iter().any(|item| item == value) {
        list.push(value.to_owned());
    }
}
fn rows(value: &Value) -> impl Iterator<Item = &Value> {
    value.as_array().into_iter().flatten()
}
fn text(value: &Value) -> &str {
    value.as_str().unwrap_or_default()
}
fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting_and_file_forms_round_trip() {
        let request = json!({"method":"PATCH","url":"https://example.test/path?a=b","headers":[{"key":"X","value":"it's \"valid\""}],
            "form":[{"key":"a","value":"hello world","type":"text"},{"key":"doc","value":"/tmp/file name.txt","type":"file"}]});
        let exported = export(&request);
        let parsed = import(&exported).unwrap().request;
        assert_eq!(parsed["method"], "PATCH");
        assert_eq!(parsed["params"][0]["value"], "b");
        assert_eq!(parsed["headers"][0]["value"], "it's \"valid\"");
        assert_eq!(parsed["body"]["form"][1]["type"], "file");
        assert_eq!(parsed["body"]["form"][1]["value"], "/tmp/file name.txt");
    }

    #[test]
    fn interprets_get_and_auth_and_reports_ignored_flags() {
        let parsed = import("curl -sL --compressed -x localhost -G -d 'a=1' -u user:pass:word -b sid=7 https://example.test").unwrap();
        assert_eq!(parsed.request["method"], "GET");
        assert_eq!(parsed.request["params"][0]["value"], "1");
        assert_eq!(parsed.request["headers"][0]["key"], "Cookie");
        assert_eq!(parsed.request["auth"]["basic"]["password"], "pass:word");
        assert_eq!(
            parsed.skipped,
            [
                "-s",
                "-L",
                "--compressed",
                "-x",
                "proxy flags are not applied"
            ]
        );
    }
}
