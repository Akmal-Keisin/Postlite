use serde_json::{Map, Value};
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone)]
pub struct Document {
    pub settings: Value,
    pub workspaces: Vec<Workspace>,
    pub active_workspace: usize,
    extra: Map<String, Value>,
}

#[derive(Clone)]
pub struct Workspace {
    pub id: String,
    pub name: String,
    pub collections: Vec<Value>,
    pub environments: Vec<Value>,
    pub history: Vec<Value>,
    pub tabs: Vec<Value>,
    pub active_tab: Option<usize>,
    extra: Map<String, Value>,
}

pub struct LocatedRequest<'a> {
    pub request: &'a Value,
    pub collection: &'a Value,
    pub ancestors: Vec<&'a Value>,
}

impl Default for Document {
    fn default() -> Self {
        Self {
            settings: Value::Object(Map::new()),
            workspaces: vec![Workspace::default()],
            active_workspace: 0,
            extra: Map::new(),
        }
    }
}

impl Default for Workspace {
    fn default() -> Self {
        Self::new("default", "My Workspace")
    }
}

pub fn load(path: &Path) -> Result<Document, String> {
    let bytes = std::fs::read(path)
        .map_err(|error| format!("Could not read workspace at {}: {error}", path.display()))?;
    let document = serde_json::from_slice(&bytes)
        .map_err(|error| format!("Invalid workspace JSON at {}: {error}", path.display()))?;
    parse(document).map_err(|error| format!("{}: {error}", path.display()))
}

pub fn save_isolated(path: &Path, document: &Document) -> Result<(), String> {
    save(path, document, default_workspace_path().as_deref())
        .map_err(|error| format!("Could not save workspace at {}: {error}", path.display()))
}

fn save(path: &Path, document: &Document, read_only: Option<&Path>) -> io::Result<()> {
    let file_name = path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "a workspace filename is required",
        )
    })?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    let directory = fs::canonicalize(parent.unwrap_or_else(|| Path::new(".")))?;
    let target = directory.join(file_name);
    if is_tauri_profile(&target, read_only) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "the Tauri workspace profile is read-only",
        ));
    }

    let (staging, mut file) = new_staging_file(&directory)?;
    let result = serde_json::to_writer(&mut file, &document.snapshot())
        .map_err(io::Error::other)
        .and_then(|_| file.sync_all());
    drop(file);
    let result = result.and_then(|_| fs::rename(&staging, &target));
    if result.is_err() {
        let _ = fs::remove_file(staging);
    }
    result
}

fn is_tauri_profile(target: &Path, read_only: Option<&Path>) -> bool {
    let Some(read_only) = read_only else {
        return false;
    };
    let Some(data_dir) = read_only.parent() else {
        return false;
    };
    let canonical_dir = fs::canonicalize(data_dir).unwrap_or_else(|_| data_dir.to_owned());
    if target.starts_with(&canonical_dir) || target.starts_with(data_dir) {
        return true;
    }
    fs::canonicalize(target).is_ok_and(|resolved| {
        resolved.starts_with(&canonical_dir)
            || fs::canonicalize(read_only).is_ok_and(|original| resolved == original)
    })
}

fn new_staging_file(directory: &Path) -> io::Result<(PathBuf, File)> {
    static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    loop {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let name = format!(".postlite-workspace-{}-{id}.tmp", std::process::id());
        let path = directory.join(name);
        match options.open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
}

pub fn default_workspace_path() -> Option<PathBuf> {
    fn absolute_env(key: &str) -> Option<PathBuf> {
        std::env::var_os(key)
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
    }

    #[cfg(target_os = "windows")]
    let data = absolute_env("APPDATA")
        .or_else(|| absolute_env("USERPROFILE").map(|home| home.join("AppData").join("Roaming")));
    #[cfg(target_os = "macos")]
    let data = absolute_env("HOME").map(|home| home.join("Library").join("Application Support"));
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let data = absolute_env("XDG_DATA_HOME")
        .or_else(|| absolute_env("HOME").map(|home| home.join(".local").join("share")));

    Some(data?.join("app.postlite.desktop").join("workspace.json"))
}

impl Workspace {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            collections: Vec::new(),
            environments: Vec::new(),
            history: Vec::new(),
            tabs: Vec::new(),
            active_tab: None,
            extra: Map::new(),
        }
    }

    pub fn find_request(&self, id: &str) -> Option<LocatedRequest<'_>> {
        self.collections.iter().find_map(|collection| {
            let mut ancestors = Vec::new();
            let request = find_request(collection["items"].as_array()?, id, &mut ancestors)?;
            Some(LocatedRequest {
                request,
                collection,
                ancestors,
            })
        })
    }

    fn snapshot(&self) -> Value {
        let mut snapshot = self.extra.clone();
        snapshot.insert("id".to_owned(), Value::String(self.id.clone()));
        snapshot.insert("name".to_owned(), Value::String(self.name.clone()));
        snapshot.insert(
            "collections".to_owned(),
            Value::Array(self.collections.clone()),
        );
        snapshot.insert(
            "environments".to_owned(),
            Value::Array(self.environments.clone()),
        );
        snapshot.insert("history".to_owned(), Value::Array(self.history.clone()));

        // A session that was disabled at load can still have saved tabs worth retaining.
        if !self.tabs.is_empty() || !snapshot.contains_key("tabs") {
            snapshot.insert("tabs".to_owned(), Value::Array(self.tabs.clone()));
            let active = self.active_tab.filter(|index| *index < self.tabs.len());
            snapshot.insert(
                "activeTabIndex".to_owned(),
                active.map_or(Value::from(-1), Value::from),
            );
            snapshot.insert(
                "activeTabId".to_owned(),
                active
                    .and_then(|index| self.tabs[index].get("id"))
                    .cloned()
                    .unwrap_or(Value::Null),
            );
        }
        Value::Object(snapshot)
    }
}

impl Document {
    pub(crate) fn snapshot(&self) -> Value {
        let mut snapshot = self.extra.clone();
        if snapshot
            .get("version")
            .and_then(Value::as_u64)
            .is_none_or(|version| version < 2)
        {
            snapshot.insert("version".to_owned(), Value::from(2));
        }
        snapshot.insert("settings".to_owned(), self.settings.clone());
        let active = self
            .workspaces
            .get(self.active_workspace)
            .or_else(|| self.workspaces.first());
        snapshot.insert(
            "activeWorkspaceId".to_owned(),
            active.map_or(Value::Null, |workspace| Value::String(workspace.id.clone())),
        );
        snapshot.insert(
            "workspaces".to_owned(),
            Value::Array(self.workspaces.iter().map(Workspace::snapshot).collect()),
        );
        let mut snapshot = Value::Object(snapshot);
        strip_proxy_credentials(&mut snapshot);
        snapshot
    }
}

fn strip_proxy_credentials(value: &mut Value) {
    match value {
        Value::Object(map) => {
            if let Some(Value::Object(proxy)) = map.get_mut("proxy") {
                proxy.remove("username");
                proxy.remove("password");
            }
            map.values_mut().for_each(strip_proxy_credentials);
        }
        Value::Array(items) => items.iter_mut().for_each(strip_proxy_credentials),
        _ => {}
    }
}

fn find_request<'a>(
    nodes: &'a [Value],
    id: &str,
    ancestors: &mut Vec<&'a Value>,
) -> Option<&'a Value> {
    for node in nodes {
        if let Some(items) = node["items"]
            .as_array()
            .filter(|_| node["kind"] == "folder")
        {
            ancestors.push(node);
            if let Some(request) = find_request(items, id, ancestors) {
                return Some(request);
            }
            ancestors.pop();
        } else if node["kind"] == "request" && node["id"].as_str() == Some(id) {
            return Some(node);
        }
    }
    None
}

pub(crate) fn parse(document: Value) -> Result<Document, String> {
    let Value::Object(mut document) = document else {
        return Err("Workspace data must be a JSON object".to_owned());
    };
    let settings = document
        .remove("settings")
        .filter(Value::is_object)
        .unwrap_or_else(|| Value::Object(Map::new()));
    let restore_tabs = settings["general"]["restoreLastSession"]
        .as_bool()
        .unwrap_or(true);
    let active = document.remove("activeWorkspaceId");
    let workspaces = match document.remove("workspaces") {
        Some(Value::Array(items)) => items
            .into_iter()
            .enumerate()
            .map(|(index, item)| {
                let Value::Object(item) = item else {
                    return Err("Workspace entries must be JSON objects".to_owned());
                };
                Ok(hydrate(item, format!("workspace-{index}"), restore_tabs))
            })
            .collect::<Result<Vec<_>, _>>()?,
        None if ["collections", "environments", "tabs"]
            .iter()
            .any(|key| document.contains_key(*key)) =>
        {
            let workspace = [
                "id",
                "name",
                "collections",
                "environments",
                "history",
                "tabs",
                "activeTabIndex",
                "activeTabId",
            ]
            .into_iter()
            .filter_map(|key| document.remove(key).map(|value| (key.to_owned(), value)))
            .collect();
            vec![hydrate(workspace, "default".to_owned(), restore_tabs)]
        }
        _ => return Err("The file contains no workspace data".to_owned()),
    };
    let workspaces = if workspaces.is_empty() {
        vec![Workspace::default()]
    } else {
        workspaces
    };
    let active_workspace = active
        .as_ref()
        .and_then(Value::as_str)
        .and_then(|id| workspaces.iter().position(|workspace| workspace.id == id))
        .unwrap_or(0);
    Ok(Document {
        settings,
        workspaces,
        active_workspace,
        extra: document,
    })
}

fn hydrate(mut data: Map<String, Value>, fallback_id: String, restore_tabs: bool) -> Workspace {
    let mut workspace = Workspace {
        id: take_string(&mut data, "id").unwrap_or(fallback_id),
        name: take_string(&mut data, "name").unwrap_or_else(|| "My Workspace".to_owned()),
        collections: objects(data.remove("collections"))
            .map(|mut collection| {
                normalize_items(&mut collection);
                Value::Object(collection)
            })
            .collect(),
        environments: objects(data.remove("environments"))
            .map(Value::Object)
            .collect(),
        history: objects(data.remove("history"))
            .take(100)
            .map(Value::Object)
            .collect(),
        tabs: Vec::new(),
        active_tab: None,
        extra: Map::new(),
    };
    if restore_tabs {
        workspace.tabs = objects(data.remove("tabs"))
            .filter_map(|mut tab| {
                if tab.get("kind").and_then(Value::as_str) == Some("environment") {
                    let id = tab.get("envId")?.as_str()?;
                    return workspace
                        .environments
                        .iter()
                        .any(|environment| environment["id"].as_str() == Some(id))
                        .then_some(Value::Object(tab));
                }
                tab.insert("kind".to_owned(), Value::String("request".to_owned()));
                if tab.get("examples").is_none_or(Value::is_null) {
                    let examples = tab
                        .get("id")
                        .and_then(Value::as_str)
                        .and_then(|id| workspace.find_request(id))
                        .and_then(|found| found.request.get("examples"))
                        .cloned()
                        .unwrap_or_else(|| Value::Array(Vec::new()));
                    tab.insert("examples".to_owned(), examples);
                }
                Some(Value::Object(tab))
            })
            .collect();
        if !workspace.tabs.is_empty() {
            workspace.active_tab = Some(
                data.get("activeTabIndex")
                    .and_then(Value::as_u64)
                    .and_then(|index| usize::try_from(index).ok())
                    .filter(|index| *index < workspace.tabs.len())
                    .unwrap_or(0),
            );
        }
        data.remove("activeTabIndex");
        data.remove("activeTabId");
    }
    workspace.extra = data;
    workspace
}

fn normalize_items(container: &mut Map<String, Value>) {
    let source = container
        .remove("items")
        .filter(Value::is_array)
        .or_else(|| container.remove("requests"));
    let items = objects(source)
        .map(|mut node| {
            if node.get("kind").and_then(Value::as_str) == Some("folder") {
                normalize_items(&mut node);
            } else {
                node.insert("kind".to_owned(), Value::String("request".to_owned()));
            }
            Value::Object(node)
        })
        .collect();
    container.insert("items".to_owned(), Value::Array(items));
}

fn objects(value: Option<Value>) -> impl Iterator<Item = Map<String, Value>> {
    let items = match value {
        Some(Value::Array(items)) => items,
        _ => Vec::new(),
    };
    items.into_iter().filter_map(|item| match item {
        Value::Object(item) => Some(item),
        _ => None,
    })
}

fn take_string(data: &mut Map<String, Value>, key: &str) -> Option<String> {
    match data.remove(key) {
        Some(Value::String(text)) => Some(text),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("postlite-save-test-{}-{id}", std::process::id()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn loads_the_pre_folder_workspace_fixture() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../.verify/legacy-workspace.json");
        let document = load(&path).expect("legacy workspace loads");
        assert_eq!(document.active_workspace, 0);
        let workspace = &document.workspaces[0];
        assert_eq!(workspace.name, "My Workspace");
        assert_eq!(workspace.collections[0]["items"][0]["kind"], "request");
        let found = workspace.find_request("legacyreq1").unwrap();
        assert_eq!(found.collection["id"], "legacy1");
        assert_eq!(found.request["headers"][0]["value"], "*/*");
        assert!(found.ancestors.is_empty());
    }

    #[test]
    fn restores_v2_selection_and_finds_the_full_folder_chain() {
        let document = parse(json!({
            "version": 2,
            "settings": {"general":{"appearance":"studio"}},
            "activeWorkspaceId": "team",
            "workspaces": [{"id":"first"}, {
                "id":"team",
                "name":"Team",
                "environments":[{"id":"dev","name":"Development"}],
                "collections":[{"id":"collection","environmentId":"dev","items":[
                    {"kind":"folder","id":"outer","items":[
                        {"kind":"folder","id":"inner","items":[
                            {"kind":"request","id":"request","method":"POST","body":{"raw":"payload"},"plugin":{"data":7}}
                        ]}
                    ]}
                ]}],
                "tabs":[{"kind":"environment","envId":"dev"},{"id":"request","url":"edited"}],
                "activeTabIndex":1
            }]
        })).unwrap();
        assert_eq!(document.settings["general"]["appearance"], "studio");
        assert_eq!(document.active_workspace, 1);
        let workspace = &document.workspaces[document.active_workspace];
        assert_eq!(workspace.id, "team");
        assert_eq!(workspace.active_tab, Some(1));
        assert_eq!(workspace.tabs[1]["url"], "edited");
        let found = workspace.find_request("request").unwrap();
        assert_eq!(found.collection["environmentId"], "dev");
        assert_eq!(found.ancestors[0]["id"], "outer");
        assert_eq!(found.ancestors[1]["id"], "inner");
        assert_eq!(found.request["body"]["raw"], "payload");
        assert_eq!(found.request["plugin"]["data"], 7);
        assert!(workspace.find_request("inner").is_none());
    }

    #[test]
    fn turning_off_session_restore_keeps_history() {
        let document = parse(json!({
            "settings":{"general":{"restoreLastSession":false}},
            "collections":[],
            "tabs":[{"id":"request","url":"http://example.test"}],
            "history":[{"id":"previous"}],
            "activeTabIndex":0
        }))
        .unwrap();
        let workspace = &document.workspaces[0];
        assert!(workspace.tabs.is_empty());
        assert_eq!(workspace.active_tab, None);
        assert_eq!(workspace.history[0]["id"], "previous");
    }

    #[test]
    fn rejects_unrecognized_workspace_files() {
        assert!(parse(json!([{"id":"not a workspace document"}])).is_err());
        assert!(parse(json!({"workspaces":[17]})).is_err());
        assert!(parse(json!({"unrelated":true})).is_err());
    }

    #[test]
    fn replacing_a_workspace_keeps_future_fields_and_request_auth() {
        let dir = TempDir::new();
        let file = dir.0.join("workspace.json");
        fs::write(&file, serde_json::to_vec(&json!({
            "version": 3,
            "futureRoot": {"value":1},
            "settings":{"request":{"proxy":{"url":"http://example.test","username":"private","password":"secret"}}},
            "activeWorkspaceId":"team",
            "workspaces":[{
                "id":"team", "name":"Before", "futureWorkspace":[1],
                "collections":[{"items":[{"id":"request","plugin":{"key":"untouched"},"auth":{"bearer":{"token":"valid"}}}]}],
                "tabs":[{"id":"request"}], "activeTabIndex":0
            }]
        })).unwrap()).unwrap();
        let mut document = load(&file).unwrap();
        document.workspaces[0].name = "After".to_owned();
        save_isolated(&file, &document).unwrap();

        let saved: Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
        assert_eq!(saved["version"], 3);
        assert_eq!(saved["activeWorkspaceId"], "team");
        assert_eq!(saved["futureRoot"]["value"], 1);
        assert_eq!(saved["workspaces"][0]["futureWorkspace"], json!([1]));
        assert!(
            saved["settings"]["request"]["proxy"]
                .get("username")
                .is_none()
        );
        assert!(
            saved["settings"]["request"]["proxy"]
                .get("password")
                .is_none()
        );
        let restored = load(&file).unwrap();
        assert_eq!(restored.workspaces[0].name, "After");
        assert_eq!(restored.workspaces[0].active_tab, Some(0));
        let request = restored.workspaces[0]
            .find_request("request")
            .unwrap()
            .request;
        assert_eq!(request["plugin"]["key"], "untouched");
        assert_eq!(request["auth"]["bearer"]["token"], "valid");
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1);
    }

    #[test]
    fn unrelated_edits_preserve_tabs_with_session_restore_disabled() {
        let dir = TempDir::new();
        let file = dir.0.join("workspace.json");
        let document = parse(json!({
            "version":1,
            "futureRoot":31,
            "settings":{"general":{"restoreLastSession":false}},
            "tabs":[{"id":"unshown","name":"Unsaved request"}],
            "activeTabIndex":0,
            "activeTabId":"old-tab"
        }))
        .unwrap();
        save_isolated(&file, &document).unwrap();

        let saved: Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
        assert_eq!(saved["version"], 2);
        assert_eq!(saved["futureRoot"], 31);
        assert_eq!(saved["workspaces"][0]["tabs"][0]["name"], "Unsaved request");
        assert_eq!(saved["workspaces"][0]["activeTabIndex"], 0);
        assert_eq!(saved["workspaces"][0]["activeTabId"], "old-tab");
    }

    #[test]
    fn prevents_writes_inside_the_tauri_profile() {
        let dir = TempDir::new();
        let profile = dir.0.join("app.postlite.desktop");
        fs::create_dir(&profile).unwrap();
        let live = profile.join("workspace.json");
        fs::write(&live, b"existing Tauri data").unwrap();

        let document = Document::default();
        assert!(save(&live, &document, Some(&live)).is_err());
        assert!(save(&profile.join("another.json"), &document, Some(&live)).is_err());
        #[cfg(unix)]
        {
            let alias = dir.0.join("alias");
            std::os::unix::fs::symlink(&profile, &alias).unwrap();
            assert!(save(&alias.join("workspace.json"), &document, Some(&live)).is_err());
            let alias = dir.0.join("workspace.json");
            std::os::unix::fs::symlink(&live, &alias).unwrap();
            assert!(save(&alias, &document, Some(&live)).is_err());
        }
        assert_eq!(fs::read(&live).unwrap(), b"existing Tauri data");
        assert_eq!(fs::read_dir(&profile).unwrap().count(), 1);
    }

    #[test]
    fn failed_replace_cleans_up_staging_file() {
        let dir = TempDir::new();
        let file = dir.0.join("workspace.json");
        fs::create_dir(&file).unwrap();
        assert!(save_isolated(&file, &Document::default()).is_err());
        assert!(file.is_dir());
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1);
    }
}
