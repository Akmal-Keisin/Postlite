use super::{Table, Workbench};
use gpui::{AppContext, Context, Entity, Subscription, Window};
use gpui_component::input::{InputEvent, InputState};
use serde_json::{Map, Value, json};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FieldTarget {
    Row(Table, usize, &'static str),
    Body(&'static str),
    Auth(&'static str, &'static str),
    AuthType,
    Script(&'static str),
}

impl FieldTarget {
    fn is_multiline(self) -> bool {
        self == Self::Body("raw") || matches!(self, Self::Script(_))
    }

    fn value(self, request: &Value) -> &Value {
        match self {
            Self::Row(table, index, key) => &table.rows(request)[index][key],
            Self::Body(key) => &request["body"][key],
            Self::Auth(kind, key) => &request["auth"][kind][key],
            Self::AuthType => &request["auth"]["type"],
            Self::Script(key) => &request["scripts"][key],
        }
    }

    fn replace(self, request: &mut Value, value: Value) -> bool {
        let (object, key) = match self {
            Self::Row(table, index, key) => {
                let Some(row) = table.rows_mut(request).and_then(|rows| rows.get_mut(index)) else {
                    return false;
                };
                (row, key)
            }
            Self::Body(key) => {
                let Some(body) = property(request, "body") else {
                    return false;
                };
                (body, key)
            }
            Self::Auth(kind, key) => {
                let Some(auth) = property(request, "auth").and_then(|auth| property(auth, kind))
                else {
                    return false;
                };
                (auth, key)
            }
            Self::AuthType => {
                let Some(auth) = property(request, "auth") else {
                    return false;
                };
                (auth, "type")
            }
            Self::Script(key) => {
                let Some(script) = property(request, "scripts") else {
                    return false;
                };
                (script, key)
            }
        };
        let Some(field) = property(object, key) else {
            return false;
        };
        if *field == value {
            return false;
        }
        *field = value;
        true
    }
}

pub(crate) struct FieldEditor {
    target: FieldTarget,
    input: Entity<InputState>,
    subscription: Subscription,
}

impl Workbench {
    pub(super) fn field_input(&self, target: FieldTarget) -> Option<&Entity<InputState>> {
        let editor = self.panel_editor.as_ref()?;
        (editor.target == target).then_some(&editor.input)
    }

    pub(super) fn edit_field(
        &mut self,
        target: FieldTarget,
        masked: bool,
        placeholder: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(request) = self.active_request() else {
            return;
        };
        let value = target.value(request).as_str().unwrap_or("").to_owned();
        let state = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(target.is_multiline())
                .placeholder(placeholder)
                .masked(masked)
        });
        state.update(cx, |input, cx| input.set_value(value, window, cx));
        let subscription = cx.subscribe_in(&state, window, move |this, input, event, _, cx| {
            if this
                .panel_editor
                .as_ref()
                .is_none_or(|editor| editor.input.entity_id() != input.entity_id())
            {
                return;
            }
            match event {
                InputEvent::Change => {
                    let value = Value::String(input.read(cx).value().to_string());
                    this.set_request_field(target, value, cx);
                }
                InputEvent::Blur | InputEvent::PressEnter { .. } if !target.is_multiline() => {
                    if let Some(editor) = this.panel_editor.take() {
                        drop(editor.subscription);
                    }
                    cx.notify();
                }
                InputEvent::Blur => {
                    if let Some(editor) = this.panel_editor.take() {
                        drop(editor.subscription);
                    }
                    cx.notify();
                }
                _ => {}
            }
        });
        self.panel_editor = Some(FieldEditor {
            target,
            input: state.clone(),
            subscription,
        });
        state.update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    pub(super) fn set_request_field(
        &mut self,
        target: FieldTarget,
        value: Value,
        cx: &mut Context<Self>,
    ) {
        if self
            .active_request_mut()
            .is_some_and(|request| target.replace(request, value))
        {
            self.changed(cx);
        }
    }

    pub(super) fn add_request_row(
        &mut self,
        table: Table,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(rows) = self
            .active_request_mut()
            .and_then(|request| table.rows_mut(request))
        else {
            return;
        };
        let index = rows.len();
        rows.push(json!({"key": "", "value": "", "enabled": true}));
        self.changed(cx);
        self.edit_field(
            FieldTarget::Row(table, index, "key"),
            false,
            "Key",
            window,
            cx,
        );
    }

    pub(super) fn remove_request_row(
        &mut self,
        table: Table,
        index: usize,
        cx: &mut Context<Self>,
    ) {
        let Some(rows) = self
            .active_request_mut()
            .and_then(|request| table.rows_mut(request))
        else {
            return;
        };
        if index < rows.len() {
            rows.remove(index);
            self.panel_editor = None;
            self.changed(cx);
        }
    }
}

impl Table {
    pub(super) fn rows(self, request: &Value) -> &Value {
        match self {
            Self::Query => &request["params"],
            Self::Headers => &request["headers"],
            Self::Multipart | Self::UrlEncoded => &request["body"]["form"],
            Self::Path => &request["pathVars"],
        }
    }

    fn rows_mut(self, request: &mut Value) -> Option<&mut Vec<Value>> {
        let slot = match self {
            Self::Query => property(request, "params"),
            Self::Headers => property(request, "headers"),
            Self::Multipart | Self::UrlEncoded => property(property(request, "body")?, "form"),
            Self::Path => property(request, "pathVars"),
        }?;
        if slot.is_null() {
            *slot = Value::Array(Vec::new());
        }
        slot.as_array_mut()
    }
}

fn property<'a>(value: &'a mut Value, key: &str) -> Option<&'a mut Value> {
    if value.is_null() {
        *value = Value::Object(Map::new());
    }
    Some(value.as_object_mut()?.entry(key).or_insert(Value::Null))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_the_requested_section_without_dropping_unknown_data() {
        let mut request = json!({
            "body": {"mode": "raw", "raw": "old", "extra": [1, 2]},
            "headers": [{"key": "X-Trace", "value": "old", "enabled": false, "extra": "keep"}],
        });
        assert!(FieldTarget::Body("raw").replace(&mut request, json!("{\n  \"key\": true\n}")));
        assert!(FieldTarget::Row(Table::Headers, 0, "value").replace(&mut request, json!("new")));
        assert_eq!(request["headers"][0]["value"], "new");
        assert_eq!(request["headers"][0]["enabled"], false);
        assert_eq!(request["headers"][0]["extra"], "keep");
        assert_eq!(request["body"]["extra"], json!([1, 2]));
        assert_eq!(request["body"]["raw"], "{\n  \"key\": true\n}");
    }

    #[test]
    fn form_fields_cannot_overwrite_query_parameters() {
        let mut request = json!({"params": [{"key": "query", "value": "1"}]});
        Table::UrlEncoded
            .rows_mut(&mut request)
            .unwrap()
            .push(json!({"key": "form", "value": "field"}));
        assert!(
            FieldTarget::Row(Table::UrlEncoded, 0, "value").replace(&mut request, json!("updated"))
        );
        assert_eq!(request["body"]["form"][0]["value"], "updated");
        assert_eq!(request["params"][0]["value"], "1");
        assert!(
            !FieldTarget::Row(Table::Headers, 42, "value").replace(&mut request, json!("never"))
        );
    }

    #[test]
    fn malformed_body_is_not_overwritten_by_an_edit() {
        let mut request = json!({"body": "legacy data"});
        assert!(!FieldTarget::Body("raw").replace(&mut request, json!("new")));
        assert_eq!(request["body"], "legacy data");
    }
}
