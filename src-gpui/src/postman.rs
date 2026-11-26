use std::{borrow::Cow, collections::HashSet, sync::LazyLock};

use chrono::{DateTime, Local, NaiveDate, NaiveDateTime, SecondsFormat, TimeZone, Utc};
use regex::Regex;
use serde_json::{Value, json};

const FORM_CONTENT_TYPE: &str = "application/x-www-form-urlencoded";
const COLLECTION_SCHEMA: &str =
    "https://schema.getpostman.com/json/collection/v2.1.0/collection.json";

static URL_PATH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:[A-Za-z][A-Za-z0-9+.-]*://)?[^/?#]*([^?#]*)").expect("literal URL path regex")
});
static PATH_VARIABLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r":([A-Za-z0-9_]+)").expect("literal path variable regex"));

pub struct CollectionImport {
    pub collection: Value,
    pub skipped_auth: usize,
}

pub fn is_collection(document: &Value) -> bool {
    document["item"].is_array()
}

pub fn is_environment(document: &Value) -> bool {
    document.is_object() && environment_rows(document).is_array()
}

pub fn import_collection(document: &Value) -> Result<CollectionImport, String> {
    let items = document["item"]
        .as_array()
        .ok_or("not a Postman collection export")?;
    let auth = &document["auth"];
    let mut skipped_auth = 0;
    let collection = json!({
        "id": new_id(), "name": field(&document["info"], "name", "Imported collection"),
        "items": import_items(items, &mut skipped_auth, unsupported_oauth(auth)),
        "auth": import_auth(auth, "noauth"), "variables": import_variables(&document["variable"]),
        "scripts": import_events(&document["event"]), "environmentId": null, "favorite": false
    });
    Ok(CollectionImport {
        collection,
        skipped_auth,
    })
}

pub fn import_environment(document: &Value) -> Result<Value, String> {
    if !is_environment(document) {
        return Err("not a Postman environment export".into());
    }
    Ok(json!({
        "id": new_id(), "name": field(document, "name", "Imported environment"),
        "variables": import_variables(environment_rows(document)),
    }))
}

pub fn export_collection(collection: &Value, environment: Option<&Value>) -> Value {
    let vars: Vec<_> = rows(&collection["variables"])
        .chain(
            environment
                .into_iter()
                .flat_map(|env| rows(&env["variables"]))
                .filter(|row| row["enabled"] != false),
        )
        .map(export_variable)
        .collect();
    let mut document = json!({
        "info": {"name": collection["name"], "schema": COLLECTION_SCHEMA},
        "variable": vars, "event": export_events(&collection["scripts"]),
        "item": rows(&collection["items"]).map(export_item).collect::<Vec<_>>(),
    });
    if let Some(auth) = export_auth(&collection["auth"]) {
        document["auth"] = auth;
    }
    document
}

pub fn export_environment(environment: &Value) -> Value {
    let values: Vec<_> = rows(&environment["variables"])
        .map(|row| {
            json!({
                "key": row["key"], "value": row["value"], "enabled": row["enabled"] != false,
                "type": if row["secret"] == true { "secret" } else { "default" },
            })
        })
        .collect();
    json!({
        "id": environment["id"], "name": environment["name"], "values": values,
        "_postman_id": environment["id"], "_postman_exported_using":"postlite",
    })
}

fn export_auth(auth: &Value) -> Option<Value> {
    let kind = auth["type"].as_str()?;
    let fields = match kind {
        "inherit" => return None,
        "noauth" => return Some(json!({"type":"noauth"})),
        "apikey" => {
            json!({"key":auth[kind]["key"],"value":auth[kind]["value"],"in":auth[kind]["addTo"]})
        }
        "oauth2" => {
            let oauth = &auth[kind];
            let mut fields = json!({
                "grant_type": oauth["grantType"], "accessTokenUrl": oauth["tokenUrl"],
                "authUrl": oauth["authorizeUrl"], "clientId": oauth["clientId"], "scope": oauth["scope"],
                "client_authentication": if oauth["clientAuthIn"] == "basic" { "header" } else { "body" },
            });
            let port = numeric(&oauth["redirectPort"]);
            if port > 0. {
                fields["redirect_uri"] = json!(format!("http://127.0.0.1:{port}/callback"));
            }
            fields
        }
        "basic" | "bearer" => auth[kind].clone(),
        _ => return None,
    };
    let rows: Vec<_> = fields
        .as_object()
        .into_iter()
        .flatten()
        .map(|(key, value)| json!({"key":key,"value":value,"type":"string"}))
        .collect();
    let mut result = json!({"type":kind});
    result[kind] = json!(rows);
    Some(result)
}

fn export_events(scripts: &Value) -> Vec<Value> {
    [("prerequest", "pre"), ("test", "test")].into_iter().filter_map(|(event, key)| {
        let code = scripts[key].as_str().filter(|code| !code.is_empty())?;
        Some(json!({"listen":event,"script":{"type":"text/javascript","exec":code.split('\n').collect::<Vec<_>>()}}))
    }).collect()
}

fn export_variable(row: &Value) -> Value {
    json!({"key": row["key"], "value": row["value"], "type":"string",
        "disabled": row["enabled"] == false, "secret": row["secret"] == true})
}

fn export_row(row: &Value, description: bool) -> Value {
    let mut result =
        json!({"key":row["key"],"value":row["value"],"disabled":row["enabled"] == false});
    if description && truthy(&row["description"]) {
        result["description"] = row["description"].clone();
    }
    result
}

fn export_item(node: &Value) -> Value {
    let mut item = json!({"name":node["name"],"event":export_events(&node["scripts"])});
    let auth = export_auth(&node["auth"]);
    if node["kind"] == "folder" {
        if let Some(auth) = auth {
            item["auth"] = auth;
        }
        item["variable"] = json!(
            rows(&node["variables"])
                .map(export_variable)
                .collect::<Vec<_>>()
        );
        item["item"] = json!(rows(&node["items"]).map(export_item).collect::<Vec<_>>());
        return item;
    }
    let mut raw = text(node, "url", "").to_owned();
    let query = rows(&node["params"])
        .filter(|row| row["enabled"] != false && !string_value(&row["key"]).trim().is_empty())
        .map(|row| {
            use crate::prepare::encode_component;
            format!(
                "{}={}",
                encode_component(&string_value(&row["key"])),
                encode_component(&string_value(&row["value"]))
            )
        })
        .collect::<Vec<_>>()
        .join("&");
    if !query.is_empty() {
        raw.push(if raw.contains('?') { '&' } else { '?' });
        raw.push_str(&query);
    }
    let mut url = json!({"raw":raw,"query":rows(&node["params"]).map(|row| export_row(row, true)).collect::<Vec<_>>()});
    if let Some(path) = node["pathVars"].as_array().filter(|vars| !vars.is_empty()) {
        url["variable"] = json!(
            path.iter()
                .map(|row| json!({"key":row["key"],"value":row["value"]}))
                .collect::<Vec<_>>()
        );
    }
    let mut headers: Vec<_> = rows(&node["headers"])
        .map(|row| export_row(row, false))
        .collect();
    let mut request = json!({"method":node["method"],"url":url});
    if let Some(auth) = auth {
        request["auth"] = auth;
    }
    let body = &node["body"];
    let content_type =
        |header: &Value| text(header, "key", "").eq_ignore_ascii_case("content-type");
    match body["mode"].as_str() {
        Some("raw") => {
            let mime = text(body, "contentType", "");
            if !mime.is_empty()
                && !headers
                    .iter()
                    .any(|header| content_type(header) && header["disabled"] != true)
            {
                headers.push(json!({"key":"Content-Type","value":mime,"disabled":false}));
            }
            let language = match mime {
                "application/json" => "json",
                "application/xml" => "xml",
                "text/html" => "html",
                "application/javascript" => "javascript",
                _ => "text",
            };
            request["body"] = json!({"mode":"raw","raw":body["raw"],"options":{"raw":{"language":language,"contentType":mime}}});
        }
        Some("form-data") => {
            let fields: Vec<_> = rows(&body["form"])
                .map(|row| {
                    let mut field = export_row(row, true);
                    if row["type"] == "file" {
                        if let Some(object) = field.as_object_mut() {
                            object.remove("value");
                        }
                        field["src"] = row["value"].clone();
                        field["type"] = json!("file");
                    } else {
                        field["type"] = json!("text");
                    }
                    field
                })
                .collect();
            request["body"] = json!({"mode":"formdata","formdata":fields});
        }
        Some("urlencoded") => {
            let mut found = false;
            for header in headers.iter_mut().filter(|row| content_type(row)) {
                found = true;
                header["value"] = json!(FORM_CONTENT_TYPE);
                header["disabled"] = json!(false);
            }
            if !found {
                headers
                    .push(json!({"key":"Content-Type","value":FORM_CONTENT_TYPE,"disabled":false}));
            }
            request["body"] = json!({"mode":"urlencoded","urlencoded":rows(&body["form"]).map(|row| export_row(row, true)).collect::<Vec<_>>()});
        }
        _ => {}
    }
    request["header"] = json!(headers);
    if let Some(examples) = node["examples"]
        .as_array()
        .filter(|items| !items.is_empty())
    {
        item["response"] = json!(
            examples
                .iter()
                .map(|example| export_example(example, &request))
                .collect::<Vec<_>>()
        );
    }
    item["request"] = request;
    item
}

fn export_example(example: &Value, request: &Value) -> Value {
    let headers: Vec<_> = rows(&example["headers"])
        .map(|row| json!({"key":row["key"],"value":row["value"]}))
        .collect();
    let mime = headers
        .iter()
        .find(|row| text(row, "key", "").eq_ignore_ascii_case("content-type"))
        .map(|row| text(row, "value", "").to_ascii_lowercase())
        .unwrap_or_default();
    let preview = if mime.contains("json")
        || (mime.is_empty() && serde_json::from_str::<Value>(text(example, "body", "")).is_ok())
    {
        "json"
    } else if mime.contains("xml") {
        "xml"
    } else if mime.contains("javascript") {
        "javascript"
    } else {
        "text"
    };
    let status = example["statusText"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| string_value(&example["status"]).into_owned());
    json!({"id":example["id"],"name":example["name"],"originalRequest":request,"status":status,"code":example["status"],
        "_postman_previewlanguage":preview,"header":headers,"cookie":[],"body":example["body"],
        "responseTime":example["elapsedMs"],"time":example["savedAt"],"size":example["sizeBytes"]})
}

fn environment_rows(document: &Value) -> &Value {
    match &document["values"] {
        Value::Null => &document["variables"],
        values => values,
    }
}

fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn rows(value: &Value) -> impl Iterator<Item = &Value> {
    value.as_array().into_iter().flatten()
}

fn field(source: &Value, name: &str, default: &str) -> Value {
    match &source[name] {
        Value::Null => Value::String(default.to_owned()),
        value => value.clone(),
    }
}

fn text<'a>(source: &'a Value, name: &str, default: &'a str) -> &'a str {
    source[name].as_str().unwrap_or(default)
}

fn string_value(value: &Value) -> Cow<'_, str> {
    match value {
        Value::Null => Cow::Borrowed(""),
        Value::String(text) => Cow::Borrowed(text),
        Value::Object(_) => Cow::Borrowed("[object Object]"),
        Value::Array(values) => Cow::Owned(
            values
                .iter()
                .map(string_value)
                .collect::<Vec<_>>()
                .join(","),
        ),
        value => {
            let encoded = value.to_string();
            Cow::Owned(
                encoded
                    .strip_suffix(".0")
                    .map_or(encoded.clone(), str::to_owned),
            )
        }
    }
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null | Value::Bool(false) => false,
        Value::Number(number) => number.as_f64().is_some_and(|number| number != 0.0),
        Value::String(text) => !text.is_empty(),
        _ => true,
    }
}

fn normal_row(
    key: Value,
    value: Value,
    enabled: bool,
    secret: bool,
    description: &str,
    kind: &str,
) -> Value {
    json!({"key": key, "value": value, "enabled": enabled, "secret": secret, "description": description, "type": kind})
}

fn import_variables(array: &Value) -> Vec<Value> {
    rows(array)
        .map(|row| {
            normal_row(
                field(row, "key", ""),
                json!(string_value(&row["value"])),
                row["enabled"] != false && !truthy(&row["disabled"]),
                row["secret"] == true || row["type"] == "secret",
                "",
                "text",
            )
        })
        .collect()
}

fn auth_field<'a>(auth: &'a Value, kind: &str, key: &str) -> &'a Value {
    rows(&auth[kind])
        .find(|entry| entry["key"] == key)
        .map_or(&Value::Null, |entry| &entry["value"])
}

fn unsupported_oauth(auth: &Value) -> bool {
    auth["type"] == "oauth2"
        && !matches!(
            auth_field(auth, "oauth2", "grant_type").as_str(),
            Some("client_credentials" | "authorization_code")
        )
}

fn import_auth(raw: &Value, fallback: &str) -> Value {
    let mut auth = json!({
        "type": fallback, "bearer": {"token": ""}, "basic": {"username": "", "password": ""},
        "apikey": {"key": "", "value": "", "addTo": "header"},
        "oauth2": {"grantType": "client_credentials", "authorizeUrl": "", "tokenUrl": "", "clientId": "",
            "scope": "", "clientAuthIn": "body", "redirectPort": ""}
    });
    let kind = text(raw, "type", "");
    let value = |key| {
        let field = auth_field(raw, kind, key);
        if field.is_null() {
            json!("")
        } else {
            field.clone()
        }
    };
    match kind {
        "noauth" => {}
        "bearer" => auth[kind]["token"] = value("token"),
        "basic" => {
            auth[kind] = json!({"username": value("username"), "password": value("password")})
        }
        "apikey" => {
            auth[kind] = json!({"key": value("key"), "value": value("value"),
            "addTo": if value("in") == "query" { "query" } else { "header" }})
        }
        "oauth2" if !unsupported_oauth(raw) => {
            let redirect_uri = value("redirect_uri");
            let redirect_port = url::Url::parse(&string_value(&redirect_uri))
                .ok()
                .map(|uri| uri.port().unwrap_or(0).to_string())
                .unwrap_or_default();
            auth[kind] = json!({
                "grantType": value("grant_type"), "authorizeUrl": value("authUrl"), "tokenUrl": value("accessTokenUrl"),
                "clientId": value("clientId"), "scope": value("scope"), "redirectPort": redirect_port,
                "clientAuthIn": if value("client_authentication") == "header" { "basic" } else { "body" }
            });
        }
        _ => return auth,
    }
    auth["type"] = json!(kind);
    auth
}

fn import_events(events: &Value) -> Value {
    let read = |listen: &str| {
        rows(events)
            .find(|event| event["listen"] == listen)
            .map(|event| {
                rows(&event["script"]["exec"])
                    .map(string_value)
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default()
    };
    json!({"pre": read("prerequest"), "test": read("test")})
}

fn import_items(items: &[Value], skipped: &mut usize, inherited_unsupported: bool) -> Vec<Value> {
    let mut result = Vec::new();
    for item in items {
        if let Some(children) = item["item"].as_array() {
            let auth = &item["auth"];
            result.push(json!({
                "kind": "folder", "id": new_id(), "name": field(item, "name", "Folder"),
                "items": import_items(children, skipped, if truthy(auth) { unsupported_oauth(auth) } else { inherited_unsupported }),
                "auth": import_auth(auth, "inherit"), "variables": import_variables(&item["variable"]),
                "scripts": import_events(&item["event"]),
            }));
        } else if truthy(&item["request"]) {
            let auth = &item["request"]["auth"];
            if if truthy(auth) {
                unsupported_oauth(auth)
            } else {
                inherited_unsupported
            } {
                *skipped += 1;
            } else {
                result.push(import_request(item));
            }
        }
    }
    result
}

pub(crate) fn decode_uri_component(raw: &str) -> String {
    let mut decoded = Vec::with_capacity(raw.len());
    let mut bytes = raw.bytes();
    let hex = |byte: u8| char::from(byte).to_digit(16).map(|digit| digit as u8);
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            let (Some(high), Some(low)) = (bytes.next().and_then(hex), bytes.next().and_then(hex))
            else {
                return raw.to_owned();
            };
            decoded.push(high * 16 + low);
        } else {
            decoded.push(byte);
        }
    }
    String::from_utf8(decoded).unwrap_or_else(|_| raw.to_owned())
}

fn import_query(raw: &str, structured: &Value) -> Vec<Value> {
    if let Some(query) = structured.as_array() {
        return query
            .iter()
            .map(|row| {
                normal_row(
                    field(row, "key", ""),
                    json!(string_value(&row["value"])),
                    !truthy(&row["disabled"]),
                    false,
                    &string_value(&row["description"]),
                    "text",
                )
            })
            .collect();
    }
    raw.split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            normal_row(
                json!(decode_uri_component(key)),
                json!(decode_uri_component(value)),
                true,
                false,
                "",
                "text",
            )
        })
        .collect()
}

pub(crate) fn path_variables(url: &str, variables: &Value) -> Vec<Value> {
    let path = URL_PATH
        .captures(url)
        .and_then(|parts| parts.get(1))
        .map_or("", |path| path.as_str());
    let mut seen = HashSet::new();
    PATH_VARIABLE
        .captures_iter(path)
        .filter_map(|parts| {
            let key = parts.get(1)?.as_str();
            if !seen.insert(key) {
                return None;
            }
            let value = rows(variables)
                .filter(|row| row["key"] == key)
                .last()
                .map_or(Cow::Borrowed(""), |row| string_value(&row["value"]));
            Some(json!({"key": key, "value": value}))
        })
        .collect()
}

fn import_request(item: &Value) -> Value {
    let request = &item["request"];
    let url = &request["url"];
    let raw_url = request
        .as_str()
        .or_else(|| url.as_str())
        .unwrap_or_else(|| text(url, "raw", ""));
    let mut parts = raw_url.split('?');
    let base = parts.next().unwrap_or_default();
    let query = import_query(parts.next().unwrap_or_default(), &url["query"]);
    let headers = if request["header"].is_array() {
        rows(&request["header"])
            .filter(|row| truthy(&row["key"]))
            .map(|row| {
                normal_row(
                    field(row, "key", ""),
                    field(row, "value", ""),
                    !truthy(&row["disabled"]),
                    false,
                    "",
                    "text",
                )
            })
            .collect::<Vec<_>>()
    } else {
        vec![normal_row(
            json!("Accept"),
            json!("*/*"),
            true,
            false,
            "",
            "text",
        )]
    };
    let examples: Vec<_> = rows(&item["response"])
        .filter(|example| example.is_object())
        .map(import_example)
        .collect();
    json!({
        "kind": "request", "id": new_id(), "name": field(item, "name", "Request"),
        "method": text(request, "method", "GET").to_uppercase(), "url": base,
        "params": query, "pathVars": path_variables(base, &url["variable"]),
        "body": import_body(&request["body"], &headers), "headers": headers,
        "auth": import_auth(&request["auth"], "inherit"), "scripts": import_events(&item["event"]),
        "examples": examples,
    })
}

fn import_body(raw: &Value, headers: &[Value]) -> Value {
    let options = &raw["options"]["raw"];
    let header = |enabled_only: bool| {
        headers
            .iter()
            .find(|row| {
                (!enabled_only || row["enabled"] == true)
                    && text(row, "key", "").eq_ignore_ascii_case("content-type")
            })
            .map(|row| &row["value"])
    };
    let (mode, raw_body, form, content_type) = match text(raw, "mode", "") {
        "raw" if raw["raw"].is_string() => {
            let language_type = match text(options, "language", "") {
                "json" => "application/json",
                "xml" => "application/xml",
                "html" => "text/html",
                "text" => "text/plain",
                "javascript" => "application/javascript",
                _ => "",
            };
            (
                "raw",
                text(raw, "raw", ""),
                Vec::new(),
                header(true)
                    .filter(|value| !value.is_null())
                    .cloned()
                    .unwrap_or(json!(language_type)),
            )
        }
        "formdata" => (
            "form-data",
            "",
            import_form(&raw["formdata"], true),
            json!(""),
        ),
        "urlencoded" => (
            "urlencoded",
            "",
            import_form(&raw["urlencoded"], false),
            json!(FORM_CONTENT_TYPE),
        ),
        _ => ("none", "", Vec::new(), json!("")),
    };
    let content_type = if mode == "urlencoded" {
        json!(FORM_CONTENT_TYPE)
    } else if !options["contentType"].is_null() {
        options["contentType"].clone()
    } else if truthy(&content_type) {
        content_type
    } else {
        header(false)
            .filter(|value| truthy(value))
            .cloned()
            .unwrap_or(json!("application/json"))
    };
    json!({"mode": mode, "raw": raw_body, "contentType": content_type, "form": form})
}

fn import_form(array: &Value, multipart: bool) -> Vec<Value> {
    rows(array)
        .map(|row| {
            let is_file = multipart && row["type"] == "file";
            let value = if is_file {
                let source = &row["src"];
                source
                    .as_array()
                    .map_or(source, |paths| paths.first().unwrap_or(&Value::Null))
            } else {
                &row["value"]
            };
            normal_row(
                field(row, "key", ""),
                json!(string_value(value)),
                !truthy(&row["disabled"]),
                false,
                &string_value(&row["description"]),
                if is_file { "file" } else { "text" },
            )
        })
        .collect()
}

fn numeric(value: &Value) -> f64 {
    value
        .as_f64()
        .or_else(|| value.as_str()?.trim().parse::<f64>().ok())
        .or_else(|| value.as_bool().map(|value| if value { 1.0 } else { 0.0 }))
        .unwrap_or(0.0)
}

fn number_value(from: f64) -> Value {
    if !from.is_finite() {
        return json!(0);
    }
    if from.fract() == 0.0 && from.abs() <= 9_007_199_254_740_991.0 {
        json!(from as i64)
    } else {
        json!(from)
    }
}

fn imported_date(raw: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(raw)
        .or_else(|_| DateTime::parse_from_rfc2822(raw))
        .ok()
        .map(|date| date.to_utc())
        .or_else(|| {
            NaiveDate::parse_from_str(raw, "%Y-%m-%d")
                .ok()?
                .and_hms_opt(0, 0, 0)
                .map(|date| date.and_utc())
        })
        .or_else(|| {
            let parsed = NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S%.f").ok()?;
            Local
                .from_local_datetime(&parsed)
                .earliest()
                .map(|date| date.to_utc())
        })
        .unwrap_or_else(Utc::now)
}

fn import_example(raw: &Value) -> Value {
    let headers: Vec<_> = rows(&raw["header"])
        .filter_map(|header| {
            let (key, value) = if let Some(raw) = header.as_str() {
                let (key, value) = raw.split_once(':').unwrap_or((raw, ""));
                (Cow::Borrowed(key.trim()), Cow::Borrowed(value.trim()))
            } else {
                (string_value(&header["key"]), string_value(&header["value"]))
            };
            (!key.is_empty()).then(|| json!({"key": key, "value": value}))
        })
        .collect();
    let body = text(raw, "body", "");
    let mut size = numeric(&raw["size"]);
    if size == 0.0 || size.is_nan() {
        size = body.len() as f64;
    }
    let name = string_value(&raw["name"]);
    let mut units = 0;
    let name: String = name
        .trim()
        .chars()
        .take_while(|character| {
            units += character.len_utf16();
            units <= 80
        })
        .collect();
    let id = text(raw, "id", "");
    json!({
        "id": if id.is_empty() { new_id() } else { id.to_owned() },
        "name": if name.is_empty() { "Example" } else { &name },
        "savedAt": imported_date(text(raw, "time", "")).to_rfc3339_opts(SecondsFormat::Millis, true),
        "status": number_value(numeric(&raw["code"])), "statusText": text(raw, "status", ""),
        "headers": headers, "body": body, "bodyB64": null, "elapsedMs": number_value(numeric(&raw["responseTime"])),
        "sizeBytes": number_value(size), "truncated": false, "note": null,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sample_collection_imports_nested_requests_and_scripts() {
        let doc: Value =
            serde_json::from_str(include_str!("../../.verify/sample-collection.json")).unwrap();
        assert!(is_collection(&doc));
        let loaded = import_collection(&doc).unwrap();
        assert_eq!(loaded.skipped_auth, 0);
        let collection = loaded.collection;
        assert_eq!(collection["name"], "Sample Import");
        assert_eq!(collection["auth"]["type"], "noauth");
        let users = &collection["items"][0];
        assert_eq!(users["kind"], "folder");
        assert_eq!(users["auth"]["apikey"]["addTo"], "header");
        assert_eq!(users["items"][0]["items"][0]["method"], "DELETE");
        assert_eq!(users["items"][1]["params"][0]["key"], "page");
        assert_eq!(users["items"][2]["body"]["contentType"], "application/json");
        assert!(
            users["items"][3]["scripts"]["pre"]
                .as_str()
                .unwrap()
                .contains("X-Scripted")
        );
        assert_eq!(collection["items"][1]["auth"]["basic"]["username"], "alice");
    }

    #[test]
    fn preserves_environment_secret_and_disabled_flags() {
        let doc: Value =
            serde_json::from_str(include_str!("../../.verify/sample-environment.json")).unwrap();
        assert!(is_environment(&doc));
        let env = import_environment(&doc).unwrap();
        assert_eq!(env["variables"][1]["secret"], true);
        assert_eq!(env["variables"][2]["enabled"], false);
        let exported = export_environment(&env);
        assert_eq!(exported["_postman_id"], env["id"]);
        assert_eq!(exported["values"], doc["values"]);
        assert!(!is_environment(&json!({"values": false, "variables": []})));
        assert!(import_environment(&json!({"values": "wrong"})).is_err());
    }

    #[test]
    fn unsupported_oauth_is_counted_without_dropping_explicit_override() {
        let input = json!({
            "auth": {"type": "oauth2", "oauth2": [{"key": "grant_type", "value": "password_credentials"}]},
            "item": [
                {"name": "blocked", "request": "https://example.test"},
                {"name": "folder", "item": [{"name": "blocked", "request": "https://example.test"}]},
                {"name": "override", "request": {"url": "https://example.test", "auth": {"type": "noauth"}}},
                {"name": "OAuth", "request": {"url": "https://example.test", "auth": {
                    "type": "oauth2", "oauth2": [
                        {"key": "grant_type", "value": "authorization_code"},
                        {"key": "redirect_uri", "value": "http://127.0.0.1:49152/callback"},
                        {"key": "clientSecret", "value": "never retain"},
                        {"key": "client_authentication", "value": "header"}
                    ]
                }}}
            ]
        });
        let loaded = import_collection(&input).unwrap();
        assert_eq!(loaded.skipped_auth, 2);
        assert_eq!(loaded.collection["items"][0]["items"], json!([]));
        assert_eq!(loaded.collection["items"][1]["auth"]["type"], "noauth");
        let oauth = &loaded.collection["items"][2]["auth"]["oauth2"];
        assert_eq!(oauth["grantType"], "authorization_code");
        assert_eq!(oauth["redirectPort"], "49152");
        assert_eq!(oauth["clientAuthIn"], "basic");
        assert!(!loaded.collection.to_string().contains("never retain"));
    }

    #[test]
    fn preserves_path_query_form_and_examples_across_roundtrip() {
        let input = json!({"item": [{
            "name": "Compound",
            "request": {"method": "post", "url": {
                "raw": "https://example.test:8080/:slug?ignore", "variable": [{"key": "slug", "value": "a / b"}],
                "query": [{"key": "q", "value": "x+y ~", "description": "query help"}, {"key": "off", "disabled": true}]
            }, "header": [{"key": "Content-Type", "value": "application/json", "disabled": true}],
                "body": {"mode":"urlencoded", "urlencoded":[{"key": "r", "value": "1", "description": "body help"}]}},
            "response": [null, 7, {"id": "example", "name": "  Saved  ", "code": "201", "status": "Created", "body": "{\"a\":1}",
                "time": "2026-01-01T01:00:00+01:00", "header": ["Content-Type: application/json", "X-Empty"], "responseTime": 9}]
        }]});
        let original = import_collection(&input).unwrap().collection;
        let request = &original["items"][0];
        assert_eq!(request["method"], "POST");
        assert_eq!(
            request["pathVars"],
            json!([{"key": "slug", "value": "a / b"}])
        );
        assert_eq!(
            request["body"]["contentType"],
            "application/x-www-form-urlencoded"
        );
        assert_eq!(
            request["examples"][0]["savedAt"],
            "2026-01-01T00:00:00.000Z"
        );
        assert_eq!(request["examples"][0]["name"], "Saved");
        let exported = export_collection(&original, None);
        let node = &exported["item"][0];
        assert_eq!(
            node["request"]["url"]["raw"],
            "https://example.test:8080/:slug?q=x%2By%20~"
        );
        assert_eq!(
            node["request"]["header"][0],
            json!({"key":"Content-Type", "value":"application/x-www-form-urlencoded", "disabled":false})
        );
        assert_eq!(
            node["request"]["body"]["urlencoded"][0]["description"],
            "body help"
        );
        assert_eq!(node["response"][0]["_postman_previewlanguage"], "json");
        assert_eq!(node["response"][0]["originalRequest"], node["request"]);
        let reimported = import_collection(&exported).unwrap().collection;
        assert_eq!(reimported["items"][0]["examples"], request["examples"]);
    }
}
