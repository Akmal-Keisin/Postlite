use std::{borrow::Cow, collections::HashMap, fmt::Write, sync::LazyLock};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use chrono::{SecondsFormat, Utc};
use regex::{Captures, Regex};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

static VARIABLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{\{([^{}]+?)\}\}").expect("literal variable regex"));
static URL_PARTS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:[A-Za-z][A-Za-z0-9+.-]*://)?[^/?#]*([^?#]*)(?:[?#][^\r\n\u{2028}\u{2029}]*)?$")
        .expect("literal URL regex")
});
static PATH_VARIABLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r":([A-Za-z0-9_]+)").expect("literal path variable regex"));
static REPEATED_SLASHES: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"/{2,}").expect("literal repeated slash regex"));

type Vars<'a> = HashMap<&'a str, Cow<'a, str>>;
type Pairs = Vec<(String, String)>;

/// Build the existing desktop transport payload from the saved request and its parent scopes.
/// OAuth credentials are supplied by the session just before sending. Scripts whose pre-send
/// effects cannot be reproduced yet still stop the request.
pub fn prepare(
    request: &Value,
    collection: Option<&Value>,
    ancestors: &[&Value],
    environment: Option<&Value>,
    settings: &Value,
    workspace_id: &str,
) -> Result<Value, String> {
    prepare_impl(
        request,
        collection,
        ancestors,
        environment,
        settings,
        workspace_id,
        false,
    )
}

pub fn preview(
    request: &Value,
    collection: Option<&Value>,
    ancestors: &[&Value],
    environment: Option<&Value>,
    settings: &Value,
    workspace_id: &str,
) -> Result<Value, String> {
    prepare_impl(
        request,
        collection,
        ancestors,
        environment,
        settings,
        workspace_id,
        true,
    )
}

fn auth_owner<'a>(
    request: &'a Value,
    collection: Option<&'a Value>,
    ancestors: &[&'a Value],
) -> Option<&'a Value> {
    if text(&request["auth"]["type"], "inherit") != "inherit" {
        Some(request)
    } else {
        collection
            .into_iter()
            .chain(ancestors.iter().copied())
            .rev()
            .find(|node| text(&node["auth"]["type"], "noauth") != "inherit")
    }
}

pub fn oauth_owner<'a>(
    request: &'a Value,
    collection: Option<&'a Value>,
    ancestors: &[&'a Value],
) -> Option<&'a Value> {
    auth_owner(request, collection, ancestors).filter(|owner| owner["auth"]["type"] == "oauth2")
}

pub fn scoped_owner(workspace: &str, node: &str) -> String {
    let mut digest = Sha256::new();
    digest.update((workspace.len() as u64).to_be_bytes());
    digest.update(workspace);
    digest.update(node);
    format!("{:x}", digest.finalize())
}

pub fn oauth_config(
    request: &Value,
    collection: Option<&Value>,
    ancestors: &[&Value],
    workspace: &str,
) -> Result<Option<crate::oauth::Config>, String> {
    oauth_owner(request, collection, ancestors)
        .map(|owner| {
            let id = owner["id"]
                .as_str()
                .filter(|id| !id.is_empty())
                .ok_or_else(|| "OAuth 2.0 needs an authentication owner ID".to_owned())?;
            crate::oauth::Config::from_auth(&scoped_owner(workspace, id), &owner["auth"]["oauth2"])
        })
        .transpose()
}

fn prepare_impl(
    request: &Value,
    collection: Option<&Value>,
    ancestors: &[&Value],
    environment: Option<&Value>,
    settings: &Value,
    workspace_id: &str,
    preview: bool,
) -> Result<Value, String> {
    let url = text(&request["url"], "");
    if trim(url).is_empty() {
        return Err("URL is required".into());
    }

    let parents = collection.into_iter().chain(ancestors.iter().copied());
    let auth = auth_owner(request, collection, ancestors)
        .map(|owner| &owner["auth"])
        .unwrap_or(&Value::Null);
    if !preview
        && parents
            .chain([request])
            .any(|node| !trim(text(&node["scripts"]["pre"], "")).is_empty())
    {
        return Err("Pre-request scripts are not yet available in the GPUI build".into());
    }

    let mut vars = collection
        .into_iter()
        .chain(environment.filter(|_| collection.is_some()))
        .chain(ancestors.iter().copied())
        .flat_map(|node| enabled_rows(&node["variables"]))
        .map(|row| (trim(text(&row["key"], "")), value_string(&row["value"])))
        .collect::<Vars<'_>>();
    if preview {
        for (key, literal) in [
            ("$guid", "{{$guid}}"),
            ("$timestamp", "{{$timestamp}}"),
            ("$isoTimestamp", "{{$isoTimestamp}}"),
            ("$randomInt", "{{$randomInt}}"),
        ] {
            vars.entry(key).or_insert(Cow::Borrowed(literal));
        }
    }

    let body = &request["body"];
    let body_mode = match text(&body["mode"], "none") {
        "none" => "none",
        "urlencoded" => "urlencoded",
        "form-data" => "form-data",
        _ => "raw",
    };
    let (headers, auth_query) = prepare_headers(request, auth, body_mode, &vars);
    let mut prepared_url = prepare_url(url, &request["pathVars"], &vars);
    let params = enabled_rows(&request["params"]).map(|row| interpolate_pair(row, &vars));
    append_query(&mut prepared_url, params.chain(auth_query));

    let form_fields = enabled_rows(&body["form"])
        .filter(|row| row["type"] != "file" || !trim(&value_string(&row["value"])).is_empty());
    let (body, form) = match body_mode {
        "urlencoded" => {
            let mut encoder = url::form_urlencoded::Serializer::new(String::new());
            encoder.extend_pairs(form_fields.map(|row| interpolate_pair(row, &vars)));
            (encoder.finish(), Vec::new())
        }
        "form-data" => {
            let form = form_fields
                .map(|row| {
                    let (key, value) = interpolate_pair(row, &vars);
                    let kind = if row["type"] == "file" {
                        "file"
                    } else {
                        "text"
                    };
                    json!({"key": key, "value": value, "type": kind})
                })
                .collect();
            (String::new(), form)
        }
        "raw" => (interpolate(&value_string(&body["raw"]), &vars), Vec::new()),
        _ => (String::new(), Vec::new()),
    };

    let default_method = text(&settings["request"]["defaultMethod"], "GET");
    let default_method = match default_method {
        "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD" | "OPTIONS" | "QUERY" => {
            default_method
        }
        _ => "GET",
    };
    let headers = headers
        .into_iter()
        .map(|(key, value)| json!({"key": key, "value": value}))
        .collect::<Vec<_>>();
    Ok(json!({
        "method": text(&request["method"], default_method), "url": prepared_url,
        "headers": headers, "body": body, "form": form, "settings": request_settings(settings, workspace_id)
    }))
}

fn trim(value: &str) -> &str {
    // ECMAScript trim includes BOM and excludes NEXT LINE.
    value.trim_matches(|c: char| (c.is_whitespace() && c != '\u{0085}') || c == '\u{feff}')
}

fn text<'a>(value: &'a Value, fallback: &'a str) -> &'a str {
    value.as_str().unwrap_or(fallback)
}

fn value_string(value: &Value) -> Cow<'_, str> {
    match value {
        Value::String(s) => Cow::Borrowed(s),
        Value::Null => Cow::Borrowed(""),
        Value::Object(_) => Cow::Borrowed("[object Object]"),
        Value::Array(items) => {
            Cow::Owned(items.iter().map(value_string).collect::<Vec<_>>().join(","))
        }
        Value::Number(number) => {
            let encoded = number.to_string();
            Cow::Owned(
                encoded
                    .strip_suffix(".0")
                    .map(str::to_owned)
                    .unwrap_or(encoded),
            )
        }
        Value::Bool(true) => Cow::Borrowed("true"),
        Value::Bool(false) => Cow::Borrowed("false"),
    }
}

fn rows(value: &Value) -> impl Iterator<Item = &Value> {
    value.as_array().into_iter().flatten()
}

fn enabled_rows(value: &Value) -> impl Iterator<Item = &Value> {
    rows(value).filter(|row| row["enabled"] != false && !trim(text(&row["key"], "")).is_empty())
}

fn interpolate(source: &str, vars: &Vars<'_>) -> String {
    VARIABLE
        .replace_all(source, |captures: &Captures<'_>| {
            let name = trim(&captures[1]);
            if let Some(value) = vars.get(name) {
                return value.to_string();
            }
            match name {
                "$guid" => uuid::Uuid::new_v4().to_string(),
                "$timestamp" => Utc::now().timestamp().to_string(),
                "$isoTimestamp" => Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
                "$randomInt" => rand::random_range(0..=1000).to_string(),
                _ => captures[0].to_owned(),
            }
        })
        .into_owned()
}

fn interpolate_pair(row: &Value, vars: &Vars<'_>) -> (String, String) {
    (
        interpolate(text(&row["key"], ""), vars),
        interpolate(&value_string(&row["value"]), vars),
    )
}

fn prepare_headers(
    request: &Value,
    auth: &Value,
    body_mode: &str,
    vars: &Vars<'_>,
) -> (Pairs, Option<(String, String)>) {
    let mut headers = if request.get("headers").is_none() {
        vec![("Accept".into(), "*/*".into())]
    } else {
        enabled_rows(&request["headers"])
            .map(|row| {
                (
                    trim(text(&row["key"], "")).to_owned(),
                    value_string(&row["value"]).into_owned(),
                )
            })
            .collect()
    };
    let mut has_content_type = headers
        .iter()
        .any(|(key, _)| key.eq_ignore_ascii_case("content-type"));
    let content_type = match body_mode {
        "urlencoded" => Some("application/x-www-form-urlencoded"),
        "raw" => Some(text(&request["body"]["contentType"], "application/json")),
        _ => None,
    };
    if let Some(content_type) = content_type.filter(|value| !value.is_empty()) {
        if !has_content_type {
            headers.push(("Content-Type".into(), content_type.into()));
            has_content_type = true;
        } else if body_mode == "urlencoded" {
            for (_, value) in headers
                .iter_mut()
                .filter(|(key, _)| key.eq_ignore_ascii_case("content-type"))
            {
                *value = content_type.into();
            }
        }
    }

    let mut auth_query = None;
    let auth_header = match text(&auth["type"], "noauth") {
        "basic" => {
            let username = interpolate(&value_string(&auth["basic"]["username"]), vars);
            let password = interpolate(&value_string(&auth["basic"]["password"]), vars);
            Some((
                "Authorization".into(),
                format!(
                    "Basic {}",
                    STANDARD.encode(format!("{username}:{password}"))
                ),
            ))
        }
        "bearer" => {
            let token = interpolate(&value_string(&auth["bearer"]["token"]), vars);
            (!token.is_empty()).then(|| ("Authorization".into(), format!("Bearer {token}")))
        }
        "apikey" => {
            let key = interpolate(&value_string(&auth["apikey"]["key"]), vars);
            let pair = (!key.is_empty()).then(|| {
                (
                    key,
                    interpolate(&value_string(&auth["apikey"]["value"]), vars),
                )
            });
            match text(&auth["apikey"]["addTo"], "header") {
                "query" => {
                    auth_query = pair;
                    None
                }
                "header" => pair,
                _ => None,
            }
        }
        _ => None,
    };
    if let Some(header) = auth_header
        .filter(|(key, _)| !has_content_type || !key.eq_ignore_ascii_case("content-type"))
    {
        headers.push(header);
    }

    for (key, value) in &mut headers {
        *key = interpolate(key, vars);
        *value = interpolate(value, vars);
    }
    (headers, auth_query)
}

fn prepare_url(authored: &str, path_vars: &Value, vars: &Vars<'_>) -> String {
    let resolved = interpolate(authored, vars);
    let Some(path) = URL_PARTS.captures(&resolved).and_then(|parts| parts.get(1)) else {
        return resolved;
    };
    let authored_path = URL_PARTS
        .captures(authored)
        .and_then(|parts| parts.get(1))
        .map_or("", |path| path.as_str());
    let authored_keys = PATH_VARIABLE
        .captures_iter(authored_path)
        .filter_map(|part| part.get(1).map(|key| key.as_str()))
        .collect::<std::collections::HashSet<_>>();
    let values = rows(path_vars)
        .filter_map(|row| {
            let key = row["key"].as_str()?;
            let value = value_string(&row["value"]);
            (authored_keys.contains(key) && !trim(&value).is_empty())
                .then(|| (key, encode_component(&interpolate(&value, vars))))
        })
        .collect::<HashMap<_, _>>();
    let normalized = REPEATED_SLASHES.replace_all(path.as_str(), "/");
    let replaced = PATH_VARIABLE.replace_all(&normalized, |capture: &Captures<'_>| {
        values
            .get(&capture[1])
            .map_or(&capture[0], String::as_str)
            .to_owned()
    });
    format!(
        "{}{replaced}{}",
        &resolved[..path.start()],
        &resolved[path.end()..]
    )
}

fn append_query(url: &mut String, pairs: impl Iterator<Item = (String, String)>) {
    let mut separator = if url.contains('?') { '&' } else { '?' };
    for (key, value) in pairs.filter(|(key, _)| !trim(key).is_empty()) {
        url.push(separator);
        url.push_str(&encode_component(&key));
        url.push('=');
        url.push_str(&encode_component(&value));
        separator = '&';
    }
}

pub(crate) fn encode_component(source: &str) -> String {
    let mut encoded = String::with_capacity(source.len());
    for byte in source.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

fn ranged_integer(value: &Value, min: i64, max: i64, fallback: i64) -> i64 {
    value
        .as_f64()
        .filter(|n| n.fract() == 0.0 && (min as f64..=max as f64).contains(n))
        .map_or(fallback, |n| n as i64)
}

fn request_settings(settings: &Value, workspace_id: &str) -> Value {
    let request = &settings["request"];
    let proxy = &request["proxy"];
    let proxy = match text(&proxy["mode"], "direct") {
        mode @ ("manual" | "environment") => json!({
            "mode": mode, "url": text(&proxy["url"], ""), "bypass": text(&proxy["bypass"], "")
        }),
        _ => Value::Null,
    };
    json!({
        "request_timeout_ms": ranged_integer(&request["requestTimeoutMs"], 1, 3_600_000, 60_000),
        "follow_redirects": request["followRedirects"].as_bool().unwrap_or(true),
        "max_redirects": ranged_integer(&request["maxRedirects"], 0, 100, 10),
        "ssl_verify": request["sslVerify"].as_bool().unwrap_or(true),
        "disable_cache": request["disableCache"].as_bool().unwrap_or(false),
        "max_response_size_kb": ranged_integer(&settings["response"]["maxResponseSizeKB"], 1, 102_400, 2_000),
        "workspace_id": workspace_id, "proxy": proxy,
    })
}

#[cfg(test)]
mod tests {
    use super::{oauth_config, prepare};
    use serde_json::{Value, json};

    // Expected IPC payloads recorded from ui/app.js's executeRequest with a mock invoke.
    #[test]
    fn resolves_scope_path_query_body_and_auth() {
        let request = json!({
            "id": "r", "url": "{{host}}//items/:id?already=1", "method": "PATCH",
            "pathVars": [{"key": "id", "value": "{{slug}}"}],
            "params": [{"key": "q", "value": "x/y !"}, {"key": "empty", "value": ""},
                {"key": "skip", "value": "skip", "enabled": false}],
            "headers": [{"key": " X-{{name}} ", "value": "{{outer}}|{{name}}|{{undeclared}}"}],
            "body": {"mode": "raw", "raw": "{{ name }}", "contentType": "text/plain"}
        });
        let collection = json!({
            "variables": [{"key": "host", "value": "https://old.test/"},
                {"key": "name", "value": "collection"}],
            "auth": {"type": "bearer", "bearer": {"token": "fallback"}}
        });
        let outer = json!({
            "variables": [{"key": "outer", "value": "folder"}, {"key": "name", "value": "outer"}],
            "auth": {"type": "basic", "basic": {"username": "żółć", "password": "{{name}}"}}
        });
        let inner = json!({
            "variables": [{"key": "name", "value": "inner"},
                {"key": "name", "value": "ignored", "enabled": false}],
            "auth": {"type": "inherit"}
        });
        let environment = json!({"variables": [
            {"key": "host", "value": "https://example.test:8081/"},
            {"key": "name", "value": "env"}, {"key": "slug", "value": "a /😊"}
        ]});
        let settings = json!({"request": {
            "requestTimeoutMs": 6500, "followRedirects": false, "maxRedirects": 3,
            "sslVerify": false, "disableCache": true,
            "proxy": {"mode": "manual", "url": "http://proxy.test", "bypass": "local"}
        }, "response": {"maxResponseSizeKB": 12}});

        let prepared = prepare(
            &request,
            Some(&collection),
            &[&outer, &inner],
            Some(&environment),
            &settings,
            "workspace-1",
        )
        .unwrap();

        assert_eq!(
            prepared,
            json!({
                "method": "PATCH",
                "url": "https://example.test:8081/items/a%20%2F%F0%9F%98%8A?already=1&q=x%2Fy%20!&empty=",
                "headers": [
                    {"key": "X-inner", "value": "folder|inner|{{undeclared}}"},
                    {"key": "Content-Type", "value": "text/plain"},
                    {"key": "Authorization", "value": "Basic xbzDs8WCxIc6aW5uZXI="}
                ],
                "body": "inner", "form": [],
                "settings": {"request_timeout_ms": 6500, "follow_redirects": false, "max_redirects": 3,
                    "ssl_verify": false, "disable_cache": true, "max_response_size_kb": 12,
                    "workspace_id": "workspace-1", "proxy": {"mode": "manual", "url": "http://proxy.test", "bypass": "local"}}
            })
        );
    }

    #[test]
    fn query_and_form_use_different_encodings() {
        let request = json!({
            "url": "https://example.test/data", "method": "POST", "params": [{"key": "p*", "value": "test ~"}],
            "auth": {"type": "apikey", "apikey": {"addTo": "query", "key": "api key", "value": "a! ~"}},
            "headers": [{"key": "Content-Type", "value": "wrong"}, {"key": "CONTENT-TYPE", "value": "wrong"}],
            "body": {"mode": "urlencoded", "form": [
                {"key": "a b", "value": "x+y~!/*'()"}, {"key": "empty", "value": ""},
                {"key": "badFile", "value": " ", "type": "file"}, {"key": "disabled", "enabled": false, "value": "bad"}
            ]}
        });
        let prepared = prepare(&request, None, &[], None, &Value::Null, "workspace-1").unwrap();
        assert_eq!(
            prepared["url"],
            "https://example.test/data?p*=test%20~&api%20key=a!%20~"
        );
        assert_eq!(prepared["body"], "a+b=x%2By%7E%21%2F*%27%28%29&empty=");
        assert_eq!(prepared["form"], json!([]));
        assert_eq!(
            prepared["headers"],
            json!([
                {"key": "Content-Type", "value": "application/x-www-form-urlencoded"},
                {"key": "CONTENT-TYPE", "value": "application/x-www-form-urlencoded"}
            ])
        );
        assert_eq!(prepared["settings"]["max_response_size_kb"], 2000);
    }

    #[test]
    fn multipart_filters_blank_files_and_preserves_header_order() {
        let request = json!({
            "url": "https://example.test/form", "method": "POST",
            "auth": {"type": "apikey", "apikey": {"key": "X-API-KEY", "value": "secret"}},
            "headers": [{"key": "Authorization", "value": "manual"}],
            "body": {"mode": "form-data", "form": [{"key": "title", "value": "hello"},
                {"key": "upload", "value": "/tmp/synthetic.txt", "type": "file"},
                {"key": "skip", "value": " ", "type": "file"}, {"key": "disabled", "enabled": false, "value": "bad"}]}
        });
        let prepared = prepare(&request, None, &[], None, &Value::Null, "workspace-1").unwrap();
        assert_eq!(prepared["body"], "");
        assert_eq!(
            prepared["form"],
            json!([
                {"key": "title", "value": "hello", "type": "text"},
                {"key": "upload", "value": "/tmp/synthetic.txt", "type": "file"}
            ])
        );
        assert_eq!(
            prepared["headers"],
            json!([
                {"key": "Authorization", "value": "manual"}, {"key": "X-API-KEY", "value": "secret"}
            ])
        );
    }

    #[test]
    fn retains_auth_header_duplicates_like_the_desktop() {
        let request = json!({"url": "https://example.test/auth",
            "headers": [{"key": "Authorization", "value": "manual"}],
            "auth": {"type": "bearer", "bearer": {"token": "token"}}
        });
        let headers = prepare(&request, None, &[], None, &Value::Null, "workspace-1").unwrap();
        assert_eq!(
            headers["headers"],
            json!([
                {"key": "Authorization", "value": "manual"}, {"key": "Authorization", "value": "Bearer token"}
            ])
        );
    }

    #[test]
    fn resolves_dynamic_tokens_and_allows_variable_overrides() {
        let request = json!({"url": "https://example.test", "body": {
            "mode": "raw", "raw": "{{$guid}}|{{$timestamp}}|{{$isoTimestamp}}|{{$randomInt}}"
        }});
        let collection = json!({"variables": [{"key": "$timestamp", "value": "override"}]});

        let prepared = prepare(&request, Some(&collection), &[], None, &Value::Null, "w").unwrap();
        let raw = prepared["body"].as_str().unwrap();
        let parts = raw.split('|').collect::<Vec<_>>();
        assert_eq!(parts.len(), 4);
        assert_eq!(
            uuid::Uuid::parse_str(parts[0]).unwrap().get_version_num(),
            4
        );
        assert_eq!(parts[1], "override");
        assert!(chrono::DateTime::parse_from_rfc3339(parts[2]).is_ok());
        assert!((0..=1000).contains(&parts[3].parse::<u16>().unwrap()));
    }

    #[test]
    fn preserves_defaults_and_blocks_missing_presend_behavior() {
        let request = json!({"url": "https://example.test"});
        let collection = json!({"auth": {"type": "oauth2"}});
        assert!(
            oauth_config(&request, Some(&collection), &[], "w")
                .err()
                .unwrap()
                .contains("OAuth 2.0")
        );
        let folder = json!({"scripts": {"pre": "  pm.request.method = 'POST';  "}});
        assert!(
            prepare(&request, None, &[&folder], None, &Value::Null, "w")
                .unwrap_err()
                .contains("Pre-request scripts")
        );

        let settings = json!({"request": {"defaultMethod": "QUERY", "requestTimeoutMs": 0,
            "maxRedirects": 7.0, "proxy": {"mode": "unknown"}}, "response": {"maxResponseSizeKB": 200_000}});
        let prepared = prepare(
            &request,
            Some(&collection),
            &[&Value::Null],
            None,
            &settings,
            "w",
        )
        .unwrap();
        assert_eq!(prepared["method"], "QUERY");
        assert_eq!(
            prepared["headers"],
            json!([{"key": "Accept", "value": "*/*"}])
        );
        assert_eq!(prepared["settings"]["request_timeout_ms"], 60_000);
        assert_eq!(prepared["settings"]["max_redirects"], 7);
        assert_eq!(prepared["settings"]["max_response_size_kb"], 2_000);
        assert_eq!(prepared["settings"]["proxy"], Value::Null);
    }
}
