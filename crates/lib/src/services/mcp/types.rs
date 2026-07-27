//! MCP protocol types.

use serde::{Deserialize, Serialize};

/// Configuration for an MCP server connection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// Transport type.
    pub transport: McpTransport,
    /// Human-readable server name.
    pub name: String,
    /// Optional environment variables to set for the server process.
    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,
}

impl McpServerConfig {
    /// Stable digest of everything that decides *what process or endpoint*
    /// this server name resolves to: the transport and its command, args,
    /// url and environment.
    ///
    /// Durable permission grants for `mcp__{server}__{tool}` embed this.
    /// The server name is mutable configuration — a branch switch or an
    /// updated project settings file can repoint `[mcp_servers.foo]` at a
    /// different command or URL — and without the binding in the key an
    /// approval given for one server would silently suppress the prompt
    /// while the input was dispatched to a replacement external process.
    ///
    /// A digest, not the values: commands, URLs and env values carry
    /// credentials (tokens in an SSE url, API keys in `env`), and the
    /// grant file lives in the config directory, where secrets must never
    /// be written.
    ///
    /// For stdio the *configured* command text does not identify the
    /// process. [`super::transport::McpTransportConnection::connect_stdio`]
    /// hands it to `Command::new`, which resolves a bare name like `npx`
    /// through the inherited `PATH` and the launch directory — so the
    /// same text can mean a different executable, or a project-local
    /// package, in another session. The resolved executable and
    /// `launch_cwd` (the directory the server process inherits) are part
    /// of the digest, and a swap re-prompts.
    ///
    /// The resolved path is canonicalized, so flipping the symlink an
    /// entry like `/usr/bin/npx` points at also re-prompts. `PATH` itself
    /// is deliberately *not* hashed: it varies with shell and version
    /// manager on every launch, and hashing it would re-prompt constantly
    /// while adding nothing once the resolution result is already bound.
    pub fn binding_fingerprint(&self, launch_cwd: &std::path::Path) -> String {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        let mut part = |bytes: &[u8]| {
            // Length-prefix every field so adjacent values cannot blur
            // into each other ("ab"+"c" vs "a"+"bc").
            h.update((bytes.len() as u64).to_le_bytes());
            h.update(bytes);
        };
        match &self.transport {
            McpTransport::Stdio { command, args } => {
                part(b"stdio");
                part(command.as_bytes());
                // Raw path bytes, never lossy strings — two executables
                // differing only in invalid UTF-8 must not collapse into
                // one fingerprint.
                part(b"|exe|");
                match resolve_executable(command, launch_cwd) {
                    Some(exe) => part(&crate::config::os_path_bytes(&exe)),
                    // Distinct from any resolved path, so an unresolvable
                    // command never shares a digest with a resolved one.
                    None => part(b"|unresolved|"),
                }
                part(&crate::config::os_path_bytes(launch_cwd));
                part(b"|args|");
                for a in args {
                    part(a.as_bytes());
                }
            }
            McpTransport::Sse { url } => {
                part(b"sse");
                part(url.as_bytes());
            }
        }
        // Sorted: `HashMap` iteration order is not stable, and a
        // fingerprint that changed between runs would re-prompt forever.
        part(b"|env|");
        let mut env: Vec<(&String, &String)> = self.env.iter().collect();
        env.sort();
        for (k, v) in env {
            part(k.as_bytes());
            part(v.as_bytes());
        }
        h.finalize().iter().map(|b| format!("{b:02x}")).collect()
    }
}

/// Canonical path of the executable a stdio `command` actually launches,
/// resolved the way `Command::new` resolves it: a name containing a
/// separator is taken relative to `launch_cwd`, a bare name is searched
/// along `PATH`.
///
/// `None` when nothing matches — the spawn would fail too, so the
/// binding falls back to the configured text and launch directory.
fn resolve_executable(command: &str, launch_cwd: &std::path::Path) -> Option<std::path::PathBuf> {
    use std::path::Path;

    if command.is_empty() {
        return None;
    }
    let has_separator = command.contains('/') || (cfg!(windows) && command.contains('\\'));
    if has_separator {
        let raw = Path::new(command);
        let joined = if raw.is_absolute() {
            raw.to_path_buf()
        } else {
            launch_cwd.join(raw)
        };
        return joined.canonicalize().ok();
    }

    for dir in std::env::split_paths(&std::env::var_os("PATH")?) {
        // A relative `PATH` entry resolves against the launch directory,
        // exactly as the child's own lookup would.
        let dir = if dir.is_absolute() {
            dir
        } else {
            launch_cwd.join(dir)
        };
        for candidate in executable_candidates(&dir, command) {
            if is_executable_file(&candidate) {
                return candidate.canonicalize().ok();
            }
        }
    }
    None
}

/// Filenames a bare `command` can match in one directory. On Windows the
/// loader appends each `PATHEXT` suffix, so `foo` finds `foo.exe`.
fn executable_candidates(dir: &std::path::Path, command: &str) -> Vec<std::path::PathBuf> {
    let exact = dir.join(command);
    #[cfg(windows)]
    {
        let mut out = vec![exact];
        let pathext = std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string())
            .to_ascii_lowercase();
        for ext in pathext.split(';').filter(|e| !e.is_empty()) {
            out.push(dir.join(format!("{command}{ext}")));
        }
        out
    }
    #[cfg(not(windows))]
    {
        vec![exact]
    }
}

fn is_executable_file(path: &std::path::Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Transport configuration for connecting to an MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum McpTransport {
    /// Subprocess communicating via stdin/stdout.
    #[serde(rename = "stdio")]
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
    },
    /// HTTP server with Server-Sent Events.
    #[serde(rename = "sse")]
    Sse { url: String },
}

/// A tool exposed by an MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpTool {
    /// Tool name (may be prefixed with server name).
    pub name: String,
    /// Human-readable description.
    pub description: Option<String>,
    /// JSON Schema for the tool's input.
    #[serde(rename = "inputSchema")]
    pub input_schema: serde_json::Value,
}

/// A resource exposed by an MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpResource {
    pub uri: String,
    pub name: Option<String>,
    pub description: Option<String>,
    #[serde(rename = "mimeType")]
    pub mime_type: Option<String>,
}

/// Result from calling an MCP tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolResult {
    pub content: Vec<McpContent>,
    #[serde(rename = "isError", default)]
    pub is_error: bool,
}

/// Content block in an MCP response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum McpContent {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image {
        data: String,
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
    #[serde(rename = "resource")]
    Resource { resource: McpResource },
}

/// JSON-RPC 2.0 request.
#[derive(Debug, Serialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: &'static str,
    pub id: u64,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

impl JsonRpcRequest {
    pub fn new(id: u64, method: &str, params: Option<serde_json::Value>) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            method: method.to_string(),
            params,
        }
    }
}

/// JSON-RPC 2.0 response.
#[derive(Debug, Deserialize)]
pub struct JsonRpcResponse {
    pub id: Option<u64>,
    pub result: Option<serde_json::Value>,
    pub error: Option<JsonRpcError>,
}

/// JSON-RPC 2.0 error.
#[derive(Debug, Deserialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    pub data: Option<serde_json::Value>,
}

/// Connection status of an MCP server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpConnectionStatus {
    Connecting,
    Connected,
    Disconnected,
    Error(String),
}

#[cfg(test)]
mod binding_tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::Path;

    fn stdio(command: &str, args: &[&str]) -> McpServerConfig {
        McpServerConfig {
            transport: McpTransport::Stdio {
                command: command.to_string(),
                args: args.iter().map(|s| s.to_string()).collect(),
            },
            name: "foo".to_string(),
            env: HashMap::new(),
        }
    }

    /// Write an executable file, returning its path.
    #[cfg(unix)]
    fn write_exe(dir: &Path, name: &str, body: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join(name);
        std::fs::write(&p, body).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    /// A bare command like `npx` is resolved through `PATH` by
    /// `Command::new`, so the configured text alone does not say which
    /// executable runs. Two directories holding different `npx` binaries
    /// must produce different bindings, or a grant approved against one
    /// would silently dispatch to the other.
    #[cfg(unix)]
    #[test]
    fn a_bare_command_binds_to_the_resolved_executable() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        write_exe(a.path(), "npx", "#!/bin/sh\necho a\n");
        write_exe(b.path(), "npx", "#!/bin/sh\necho b\n");

        let cfg = stdio("npx", &["server"]);
        let cwd = Path::new("/");

        let _guard = crate::test_support::EnvGuard::set("PATH", a.path());
        let from_a = cfg.binding_fingerprint(cwd);
        assert_eq!(
            from_a,
            cfg.binding_fingerprint(cwd),
            "the fingerprint must be stable for one resolution"
        );
        drop(_guard);

        let _guard = crate::test_support::EnvGuard::set("PATH", b.path());
        let from_b = cfg.binding_fingerprint(cwd);
        assert_ne!(
            from_a, from_b,
            "the same command text bound to two different executables"
        );
    }

    /// A relative command resolves against the directory the server
    /// process inherits, so the launch cwd is part of the binding.
    #[cfg(unix)]
    #[test]
    fn a_relative_command_binds_to_the_launch_directory() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        write_exe(a.path(), "run.sh", "#!/bin/sh\necho a\n");
        write_exe(b.path(), "run.sh", "#!/bin/sh\necho b\n");

        let cfg = stdio("./run.sh", &[]);
        assert_ne!(
            cfg.binding_fingerprint(a.path()),
            cfg.binding_fingerprint(b.path()),
            "a relative command crossed launch directories"
        );
    }

    /// Canonicalization means repointing the symlink an entry resolves
    /// through re-prompts, even though the configured text never moved.
    #[cfg(unix)]
    #[test]
    fn repointing_a_symlinked_executable_changes_the_binding() {
        let dir = tempfile::tempdir().unwrap();
        let real_a = write_exe(dir.path(), "real-a", "#!/bin/sh\necho a\n");
        let real_b = write_exe(dir.path(), "real-b", "#!/bin/sh\necho b\n");
        let link = dir.path().join("tool");
        std::os::unix::fs::symlink(&real_a, &link).unwrap();

        let cfg = stdio(link.to_str().unwrap(), &[]);
        let before = cfg.binding_fingerprint(Path::new("/"));

        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(&real_b, &link).unwrap();
        assert_ne!(
            before,
            cfg.binding_fingerprint(Path::new("/")),
            "a swapped symlink kept the old binding"
        );
    }

    /// An unresolvable command must not share a digest with a resolved
    /// one, and must stay stable so a broken server does not re-prompt
    /// on a loop.
    #[cfg(unix)]
    #[test]
    fn an_unresolvable_command_gets_its_own_stable_binding() {
        let dir = tempfile::tempdir().unwrap();
        let _guard = crate::test_support::EnvGuard::set("PATH", dir.path());
        let cfg = stdio("definitely-not-on-path-xyz", &[]);
        let first = cfg.binding_fingerprint(Path::new("/"));
        assert_eq!(first, cfg.binding_fingerprint(Path::new("/")));

        write_exe(dir.path(), "definitely-not-on-path-xyz", "#!/bin/sh\n");
        assert_ne!(
            first,
            cfg.binding_fingerprint(Path::new("/")),
            "an unresolvable command shared a binding with a resolved one"
        );
    }

    /// An SSE endpoint has no executable to resolve, so the launch
    /// directory must not enter its binding — it would re-prompt for a
    /// remote server that never changed.
    #[test]
    fn an_sse_binding_ignores_the_launch_directory() {
        let cfg = McpServerConfig {
            transport: McpTransport::Sse {
                url: "https://mcp.example.com/sse".to_string(),
            },
            name: "foo".to_string(),
            env: HashMap::new(),
        };
        assert_eq!(
            cfg.binding_fingerprint(Path::new("/one")),
            cfg.binding_fingerprint(Path::new("/two")),
        );
    }

    /// Args are still part of the binding, and they cannot blur into the
    /// fields around them.
    #[test]
    fn args_are_length_prefixed_and_bound() {
        let cwd = Path::new("/test-cwd");
        assert_ne!(
            stdio("/bin/tool", &["--safe"]).binding_fingerprint(cwd),
            stdio("/bin/tool", &["--unsafe"]).binding_fingerprint(cwd),
        );
        assert_ne!(
            stdio("/bin/tool", &["ab", "c"]).binding_fingerprint(cwd),
            stdio("/bin/tool", &["a", "bc"]).binding_fingerprint(cwd),
            "adjacent args blurred into one another"
        );
    }
}
