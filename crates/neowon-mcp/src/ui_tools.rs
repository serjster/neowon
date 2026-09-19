//! MCP tools for the UI's structure and the RF reference: the UI element
//! tree (`get uitree`, the app's "DOM") and the band plan (`get bands`,
//! `bandplan`, `bandmap goto`).

use rmcp::{ErrorData, handler::server::wrapper::Parameters, tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use crate::Scope;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TreeParams {
    /// Only elements whose role or label contains this text (case
    /// insensitive), returned as a flat list with their paths — e.g.
    /// "Button", "band", "dock section". Omit for the whole tree.
    filter: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct BandParams {
    /// Switch to this band plan first (file stem, e.g. `usa`, `general`).
    plan: Option<String>,
    /// Tune to this band's centre and fit the span to it (e.g. "2m Ham Band").
    goto: Option<String>,
}

/// Every node under `v` matching `needle`, flattened, with its path of
/// ancestor labels/roles.
fn matches(v: &Value, needle: &str, path: &mut Vec<String>, out: &mut Vec<Value>) {
    let role = v["role"].as_str().unwrap_or("");
    let label = v["label"].as_str().unwrap_or("");
    let hit = role.to_lowercase().contains(needle) || label.to_lowercase().contains(needle);
    if hit {
        let mut n = v.clone();
        if let Some(o) = n.as_object_mut() {
            o.remove("children");
            o.insert("path".into(), Value::from(path.join(" > ")));
        }
        out.push(n);
    }
    path.push(if label.is_empty() {
        role.into()
    } else {
        label.into()
    });
    if let Some(kids) = v["children"].as_array() {
        for k in kids {
            matches(k, needle, path, out);
        }
    }
    path.pop();
}

#[tool_router(router = ui_router, vis = "pub(crate)")]
impl Scope {
    #[tool(description = "The app's UI element tree, like a browser's DOM \
        inspector: every widget and custom-painted element (canvas, band \
        segments, dock sections) with its role, label, value, state and rect \
        [x, y, w, h] in logical window pixels, plus the named painted regions. \
        Prefer this over screenshots for checking layout. `filter` returns a \
        flat list of matching elements instead of the whole tree.")]
    async fn ui_tree(&self, p: Parameters<TreeParams>) -> Result<String, ErrorData> {
        let raw = self.req("get uitree")?;
        let Some(needle) = p.0.filter.map(|f| f.to_lowercase()) else {
            return Ok(raw);
        };
        let v: Value = serde_json::from_str(&raw)
            .map_err(|e| ErrorData::internal_error(format!("bad tree JSON: {e}"), None))?;
        let mut out = Vec::new();
        matches(&v["root"], &needle, &mut Vec::new(), &mut out);
        if let Some(regions) = v["regions"].as_array() {
            for r in regions {
                matches(r, &needle, &mut vec!["regions".into()], &mut out);
            }
        }
        Ok(Value::from(out).to_string())
    }

    #[tool(description = "The RF band plan around the SDR: the active plan \
        and the others available, the bands containing the tuned frequency \
        (narrowest first) and those in the displayed span. Optionally switch \
        plan, or jump to a band by name (tunes to its centre and fits the span).")]
    async fn rf_bands(&self, p: Parameters<BandParams>) -> Result<String, ErrorData> {
        if let Some(plan) = p.0.plan {
            self.req(&format!("bandplan {}", plan.trim()))?;
        }
        if let Some(band) = p.0.goto {
            self.req(&format!("bandmap goto {}", band.trim()))?;
        }
        self.req("get bands")
    }
}
