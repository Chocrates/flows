//! flows-tools — agent tools: files, sandbox (bwrap), bash, http, search, plugin.

pub mod bash;
pub mod error;
pub mod files;
pub mod http;
pub mod plugin;
pub mod sandbox;
pub mod search;

pub use bash::{check_allowlist, check_blocked, run_bash};
pub use error::{ToolError, ToolResult};
pub use http::{check_url, http_get, is_blocked_ip};
pub use plugin::{list_plugin_tools, load_plugin_tool, run_plugin_tool, PluginToolDef};
pub use sandbox::{run_confined, sandbox_available, wrap_command};
pub use search::{parse_ddg_html, web_search};
