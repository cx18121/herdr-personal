use std::{
    env,
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    error::{AppError, AppResult},
    layout::{LayoutDescription, LayoutNode, SplitDirection},
};

static REQUEST_ID: AtomicU64 = AtomicU64::new(1);

pub struct HerdrClient {
    socket_path: PathBuf,
}

impl HerdrClient {
    pub fn from_env() -> AppResult<Self> {
        let socket_path = env::var_os("HERDR_SOCKET_PATH")
            .map(PathBuf::from)
            .ok_or_else(|| AppError::Message("HERDR_SOCKET_PATH is missing".into()))?;
        Ok(Self { socket_path })
    }

    pub fn call(&self, method: &str, params: Value) -> AppResult<Value> {
        let mut stream = UnixStream::connect(&self.socket_path)?;
        stream.set_read_timeout(Some(Duration::from_secs(15)))?;
        stream.set_write_timeout(Some(Duration::from_secs(15)))?;
        let id = format!("layouts-{}", REQUEST_ID.fetch_add(1, Ordering::Relaxed));
        let request = json!({ "id": id, "method": method, "params": params });
        writeln!(stream, "{request}")?;

        let mut response = String::new();
        BufReader::new(stream).read_line(&mut response)?;
        let ApiResponse { result, error } = serde_json::from_str(&response)?;

        if let Some(ApiError { code, message }) = error {
            return Err(AppError::Herdr { code, message });
        }

        result.ok_or_else(|| AppError::Message("Herdr returned an empty response".into()))
    }

    pub fn export_layout(&self, pane_id: &str) -> AppResult<LayoutDescription> {
        let result = self.call("layout.export", json!({ "pane_id": pane_id }))?;
        let layout = serde_json::from_value(result["layout"].clone())?;
        Ok(layout)
    }

    pub fn rearrange_layout(
        &self,
        tab_id: &str,
        root: &LayoutNode,
    ) -> AppResult<LayoutDescription> {
        let result = self.call(
            "layout.rearrange",
            json!({ "tab_id": tab_id, "root": rearrange_node(root) }),
        )?;
        Ok(serde_json::from_value(result["layout"].clone())?)
    }

    pub fn set_split_ratio(&self, tab_id: &str, path: &[bool], ratio: f64) -> AppResult<()> {
        self.call(
            "layout.set_split_ratio",
            json!({ "tab_id": tab_id, "path": path, "ratio": ratio }),
        )?;
        Ok(())
    }

    pub fn create_parking_tab(&self, workspace_id: &str) -> AppResult<(String, String)> {
        let result = self.call(
            "tab.create",
            json!({
                "workspace_id": workspace_id,
                "label": "layouts-temporary",
                "focus": false
            }),
        )?;
        let tab_id = string_at(&result, &["tab", "tab_id"])?;
        let pane_id = string_at(&result, &["root_pane", "pane_id"])?;
        Ok((tab_id, pane_id))
    }

    pub fn swap_panes(&self, source_pane_id: &str, target_pane_id: &str) -> AppResult<()> {
        let result = self.call(
            "pane.swap",
            json!({
                "source_pane_id": source_pane_id,
                "target_pane_id": target_pane_id
            }),
        )?;
        if result["swap"]["changed"].as_bool() != Some(true) {
            let reason = result["swap"]["reason"]
                .as_str()
                .unwrap_or("unknown reason");
            return Err(AppError::Message(format!(
                "Herdr did not swap panes: {reason}"
            )));
        }
        Ok(())
    }

    pub fn move_pane(
        &self,
        pane_id: &str,
        tab_id: &str,
        target_pane_id: &str,
        direction: SplitDirection,
        ratio: f64,
        focus: bool,
    ) -> AppResult<String> {
        let result = self.call(
            "pane.move",
            json!({
                "pane_id": pane_id,
                "destination": {
                    "type": "tab",
                    "tab_id": tab_id,
                    "target_pane_id": target_pane_id,
                    "split": direction.as_str(),
                    "ratio": ratio
                },
                "focus": focus
            }),
        )?;

        if result["move_result"]["changed"].as_bool() != Some(true) {
            let reason = result["move_result"]["reason"]
                .as_str()
                .unwrap_or("unknown reason");
            return Err(AppError::Message(format!(
                "Herdr did not move pane {pane_id}: {reason}"
            )));
        }

        string_at(&result, &["move_result", "pane", "pane_id"])
    }

    pub fn close_tab(&self, tab_id: &str) -> AppResult<()> {
        self.call("tab.close", json!({ "tab_id": tab_id }))?;
        Ok(())
    }

    pub fn open_picker(&self, pane_id: &str) -> AppResult<()> {
        self.call(
            "plugin.pane.open",
            json!({
                "plugin_id": "local.layouts",
                "entrypoint": "picker",
                "placement": "popup",
                "width": 64,
                "height": 20,
                "focus": true,
                "env": { "LAYOUT_ORIGIN_PANE_ID": pane_id }
            }),
        )?;
        Ok(())
    }

    pub fn notify(&self, title: &str, body: &str) {
        let _ = self.call(
            "notification.show",
            json!({ "title": title, "body": body, "sound": "none" }),
        );
    }
}

fn rearrange_node(node: &LayoutNode) -> Value {
    match node {
        LayoutNode::Pane { pane_id, .. } => json!({
            "type": "pane",
            "pane_id": pane_id
        }),
        LayoutNode::Split {
            direction,
            ratio,
            first,
            second,
        } => json!({
            "type": "split",
            "direction": direction,
            "ratio": ratio,
            "first": rearrange_node(first),
            "second": rearrange_node(second)
        }),
    }
}

fn string_at(value: &Value, path: &[&str]) -> AppResult<String> {
    path.iter()
        .try_fold(value, |value, key| value.get(*key))
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| AppError::Message(format!("Herdr response is missing {}", path.join("."))))
}

#[derive(Deserialize)]
struct ApiResponse {
    result: Option<Value>,
    error: Option<ApiError>,
}

#[derive(Deserialize)]
struct ApiError {
    code: String,
    message: String,
}
