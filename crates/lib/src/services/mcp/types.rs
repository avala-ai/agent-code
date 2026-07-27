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
    /// Resolution uses the *child's* effective `PATH`: `connect_stdio`
    /// applies `env` to the `Command` before spawning, so a server that
    /// sets `env.PATH` is looked up along that, not along the agent's own
    /// `PATH`.
    ///
    /// The resolved path is canonicalized, so flipping the symlink an
    /// entry like `/usr/bin/npx` points at also re-prompts — including a
    /// symlink inside a configured `env.PATH`. The `PATH` *string* is
    /// deliberately not relied on for this: it varies with shell and
    /// version manager on every launch, and it says nothing about what
    /// the entries currently point at.
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
                match resolve_executable(command, launch_cwd, &self.env) {
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
/// along the child's effective `PATH`.
///
/// `env` is the server's configured environment, which `connect_stdio`
/// applies before spawning — so an `env.PATH` override decides the
/// lookup, exactly as it will for the real child.
///
/// `None` when nothing matches — the spawn would fail too, so the
/// binding falls back to the configured text and launch directory.
fn resolve_executable(
    command: &str,
    launch_cwd: &std::path::Path,
    env: &std::collections::HashMap<String, String>,
) -> Option<std::path::PathBuf> {
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

    // The child's PATH: the configured override when there is one,
    // otherwise the one this process would pass down.
    let path_var = match env.get("PATH") {
        Some(p) => std::ffi::OsString::from(p),
        None => std::env::var_os("PATH")?,
    };
    // Windows resolves a bare name against the working directory before
    // walking PATH; Unix `execvp` never does.
    let search_dirs = std::env::split_paths(&path_var);
    #[cfg(windows)]
    let search_dirs = std::iter::once(launch_cwd.to_path_buf()).chain(search_dirs);
    for dir in search_dirs {
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

/// Filenames a bare `command` can match in one directory.
///
/// Windows mirrors `std::process::Command`, which appends **only** `.exe`
/// to an extensionless program — it does not walk `PATHEXT`. Honoring
/// `PATHEXT` here would bind the fingerprint to a `.cmd` or `.com` that
/// the spawn never reaches, leaving the real `.exe` free to be swapped
/// under an unchanged grant.
fn executable_candidates(dir: &std::path::Path, command: &str) -> Vec<std::path::PathBuf> {
    let exact = dir.join(command);
    #[cfg(windows)]
    {
        if std::path::Path::new(command).extension().is_none() {
            return vec![exact, dir.join(format!("{command}.exe"))];
        }
        vec![exact]
    }
    #[cfg(not(windows))]
    {
        vec![exact]
    }
}

impl McpServerConfig {
    /// Whether the working directory changes what this server resolves
    /// to. A stdio child inherits the launch directory and resolves bare
    /// commands against it; an SSE endpoint is the same endpoint from
    /// anywhere.
    pub fn is_cwd_sensitive(&self) -> bool {
        matches!(self.transport, McpTransport::Stdio { .. })
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

    /// `connect_stdio` applies `env` to the `Command` before spawning,
    /// so a server that sets `env.PATH` is looked up along *that*. If
    /// resolution used only the agent's inherited PATH, repointing a
    /// symlink inside the configured PATH would launch a different
    /// server under an unchanged fingerprint.
    #[cfg(unix)]
    #[test]
    fn a_configured_env_path_decides_the_resolution() {
        let inherited = tempfile::tempdir().unwrap();
        let configured_a = tempfile::tempdir().unwrap();
        let configured_b = tempfile::tempdir().unwrap();
        write_exe(inherited.path(), "srv", "#!/bin/sh\necho inherited\n");
        write_exe(configured_a.path(), "srv", "#!/bin/sh\necho a\n");
        write_exe(configured_b.path(), "srv", "#!/bin/sh\necho b\n");

        let _guard = crate::test_support::EnvGuard::set("PATH", inherited.path());
        let cwd = Path::new("/");

        let mut cfg_a = stdio("srv", &[]);
        cfg_a.env.insert(
            "PATH".to_string(),
            configured_a.path().to_str().unwrap().to_string(),
        );
        let mut cfg_b = stdio("srv", &[]);
        cfg_b.env.insert(
            "PATH".to_string(),
            configured_b.path().to_str().unwrap().to_string(),
        );

        assert_ne!(
            cfg_a.binding_fingerprint(cwd),
            cfg_b.binding_fingerprint(cwd),
            "two configured PATHs resolved to the same binding"
        );

        // And the configured PATH wins over the inherited one: the
        // binding must not be the one the agent's own PATH would find.
        let plain = stdio("srv", &[]);
        assert_ne!(
            cfg_a.binding_fingerprint(cwd),
            plain.binding_fingerprint(cwd),
            "env.PATH was ignored in favour of the inherited PATH"
        );
    }

    /// Repointing a symlink *inside* a configured `env.PATH` must
    /// invalidate the grant, even though every configured string —
    /// command, args and the PATH value itself — is unchanged.
    #[cfg(unix)]
    #[test]
    fn repointing_inside_a_configured_env_path_changes_the_binding() {
        let bin = tempfile::tempdir().unwrap();
        let targets = tempfile::tempdir().unwrap();
        let real_a = write_exe(targets.path(), "a", "#!/bin/sh\necho a\n");
        let real_b = write_exe(targets.path(), "b", "#!/bin/sh\necho b\n");
        let link = bin.path().join("srv");
        std::os::unix::fs::symlink(&real_a, &link).unwrap();

        let mut cfg = stdio("srv", &[]);
        cfg.env
            .insert("PATH".to_string(), bin.path().to_str().unwrap().to_string());
        let before = cfg.binding_fingerprint(Path::new("/"));

        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(&real_b, &link).unwrap();
        assert_ne!(
            before,
            cfg.binding_fingerprint(Path::new("/")),
            "a swapped symlink inside env.PATH kept the old binding"
        );
    }

    /// `std::process::Command` on Windows appends only `.exe` to an
    /// extensionless program — it does not walk `PATHEXT`. Binding to a
    /// `.cmd`/`.com` the spawn never reaches would leave the real `.exe`
    /// free to be swapped under an unchanged grant, so a configured
    /// `PATHEXT` must not steer the resolution.
    #[cfg(windows)]
    #[test]
    fn windows_resolution_ignores_pathext() {
        let bin = tempfile::tempdir().unwrap();
        std::fs::write(bin.path().join("srv.cmd"), "echo cmd").unwrap();
        std::fs::write(bin.path().join("srv.exe"), "MZ").unwrap();

        let mut cfg = stdio("srv", &[]);
        cfg.env
            .insert("PATH".to_string(), bin.path().to_str().unwrap().to_string());
        // `.cmd` first would win if PATHEXT were honored.
        cfg.env
            .insert("PATHEXT".to_string(), ".CMD;.EXE".to_string());

        let resolved = resolve_executable("srv", Path::new("/"), &cfg.env).unwrap();
        assert_eq!(
            resolved.file_name().unwrap(),
            std::ffi::OsStr::new("srv.exe"),
            "PATHEXT steered the binding away from what Command spawns"
        );
    }

    /// A program that already carries an extension is taken as-is.
    #[cfg(windows)]
    #[test]
    fn windows_resolution_keeps_an_explicit_extension() {
        let bin = tempfile::tempdir().unwrap();
        std::fs::write(bin.path().join("srv.cmd"), "echo cmd").unwrap();
        let mut cfg = stdio("srv.cmd", &[]);
        cfg.env
            .insert("PATH".to_string(), bin.path().to_str().unwrap().to_string());

        let resolved = resolve_executable("srv.cmd", Path::new("/"), &cfg.env).unwrap();
        assert_eq!(
            resolved.file_name().unwrap(),
            std::ffi::OsStr::new("srv.cmd")
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
