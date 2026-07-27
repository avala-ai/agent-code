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
    /// `PATH`. On Windows that lookup is case-insensitive, matching the
    /// environment block the child actually receives.
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
/// resolved the way `std::process::Command` resolves it on this platform.
///
/// `env` is the server's configured environment, which `connect_stdio`
/// applies before spawning — so a `PATH` override there decides the
/// lookup, exactly as it will for the real child.
///
/// `None` when nothing matches — the spawn would fail too, so the
/// binding falls back to the configured text and launch directory.
///
/// Mirroring the real resolution is the whole point: a fingerprint bound
/// to a file the child never launches would not change when the file it
/// *does* launch is replaced, and the durable grant would keep matching.
fn resolve_executable(
    command: &str,
    launch_cwd: &std::path::Path,
    env: &std::collections::HashMap<String, String>,
) -> Option<std::path::PathBuf> {
    if command.is_empty() {
        return None;
    }
    #[cfg(windows)]
    {
        resolve_executable_windows(command, launch_cwd, env)
    }
    #[cfg(not(windows))]
    {
        resolve_executable_unix(command, launch_cwd, env)
    }
}

/// `execvp` semantics: a name containing `/` is used as given, a bare
/// name is searched along the child's `PATH` and must be executable.
/// The working directory is never searched.
#[cfg(not(windows))]
fn resolve_executable_unix(
    command: &str,
    launch_cwd: &std::path::Path,
    env: &std::collections::HashMap<String, String>,
) -> Option<std::path::PathBuf> {
    use std::path::Path;

    if command.contains('/') {
        let raw = Path::new(command);
        let joined = if raw.is_absolute() {
            raw.to_path_buf()
        } else {
            launch_cwd.join(raw)
        };
        return joined.canonicalize().ok();
    }

    // The child's PATH: the configured override when there is one,
    // otherwise the one this process would pass down. `exec` consults
    // only the environment the child is given.
    let path_var = match env_lookup(env, "PATH") {
        Some(p) => std::ffi::OsString::from(p),
        None => std::env::var_os("PATH")?,
    };
    for dir in std::env::split_paths(&path_var) {
        // A relative `PATH` entry resolves against the launch directory,
        // exactly as the child's own lookup would.
        let dir = if dir.is_absolute() {
            dir
        } else {
            launch_cwd.join(dir)
        };
        let candidate = dir.join(command);
        if is_executable_file(&candidate) {
            return candidate.canonicalize().ok();
        }
    }
    None
}

/// `CreateProcessW` semantics as `std::process::Command` implements them
/// in `resolve_exe`.
///
/// Two deliberate non-behaviors, both of which would otherwise bind this
/// fingerprint to a file the spawn never reaches — leaving the real
/// target free to be swapped under an unchanged grant:
///
/// - **No `PATHEXT` walk.** `Command` appends `.exe` and nothing else,
///   and only when the file name contains no `.` at all.
/// - **No working-directory search.** `resolve_exe` looks in the child's
///   `PATH`, the directory the agent itself was loaded from, the system
///   and Windows directories, then the agent's own `PATH`. The launch
///   directory is not among them, so an `npx.exe` dropped in the repo
///   must not capture the binding for a `PATH`-resolved `npx`.
#[cfg(windows)]
fn resolve_executable_windows(
    command: &str,
    launch_cwd: &std::path::Path,
    env: &std::collections::HashMap<String, String>,
) -> Option<std::path::PathBuf> {
    use std::path::{Path, PathBuf};

    // Byte-level and case-insensitive, exactly like `resolve_exe`.
    let bytes = command.as_bytes();
    let has_exe_suffix = bytes.len() >= 4 && bytes[bytes.len() - 4..].eq_ignore_ascii_case(b".exe");
    let is_file_name =
        !command.contains(['/', '\\']) && Path::new(command).components().count() == 1;

    if !is_file_name {
        // A path, not a name: no directory search happens at all.
        let raw = Path::new(command);
        let joined = if raw.is_absolute() {
            raw.to_path_buf()
        } else {
            launch_cwd.join(raw)
        };
        if has_exe_suffix {
            return joined.canonicalize().ok();
        }
        // `.exe` is *appended*, not substituted (`a.b` -> `a.b.exe`),
        // and the bare path is the fallback when that does not exist.
        let mut with_exe = joined.clone().into_os_string();
        with_exe.push(".exe");
        return PathBuf::from(with_exe)
            .canonicalize()
            .ok()
            .or_else(|| joined.canonicalize().ok());
    }

    // > If the file name does not contain an extension, .exe is appended.
    //
    // `set_extension` substitutes rather than appends, which is what
    // `resolve_exe` does here — so an extensionless `foo` is looked up
    // *only* as `foo.exe`, never as a bare `foo`.
    let has_extension = command.contains('.');
    for dir in windows_search_dirs(launch_cwd, env) {
        let mut candidate = dir.join(command);
        if !has_extension {
            candidate.set_extension("exe");
        }
        // `program_exists` only asks whether the file opens; there is no
        // executable bit to consult.
        if candidate.is_file() {
            return candidate.canonicalize().ok();
        }
    }
    None
}

/// The directories `search_paths` walks, in order. Note that the agent's
/// own `PATH` is still consulted after a configured one — a child `PATH`
/// leads the search, it does not replace it.
///
/// The system directories are derived from `SystemRoot` rather than
/// `GetSystemDirectoryW`; they are reachable only after the earlier
/// passes miss, and writing to them already requires administrator.
#[cfg(windows)]
fn windows_search_dirs(
    launch_cwd: &std::path::Path,
    env: &std::collections::HashMap<String, String>,
) -> Vec<std::path::PathBuf> {
    use std::path::PathBuf;

    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut push_entries = |dirs: &mut Vec<PathBuf>, raw: std::ffi::OsString| {
        for dir in std::env::split_paths(&raw).filter(|p| !p.as_os_str().is_empty()) {
            dirs.push(if dir.is_absolute() {
                dir
            } else {
                launch_cwd.join(dir)
            });
        }
    };

    if let Some(p) = env_lookup(env, "PATH") {
        push_entries(&mut dirs, std::ffi::OsString::from(p));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            dirs.push(parent.to_path_buf());
        }
    }
    if let Some(root) = std::env::var_os("SystemRoot").or_else(|| std::env::var_os("windir")) {
        let root = PathBuf::from(root);
        dirs.push(root.join("System32"));
        dirs.push(root);
    }
    if let Some(p) = std::env::var_os("PATH") {
        push_entries(&mut dirs, p);
    }
    dirs
}

/// The child's value for an environment variable.
///
/// Windows environment blocks are case-insensitive and `Command::env`
/// keys them that way, so a server configuring `Path` overrides `PATH`
/// for the child. A case-sensitive lookup would miss that, resolve along
/// the agent's own `PATH` instead, and stop tracking swaps inside the
/// configured one.
fn env_lookup<'a>(
    env: &'a std::collections::HashMap<String, String>,
    name: &str,
) -> Option<&'a String> {
    #[cfg(windows)]
    {
        // Several case variants leave the child's own value ambiguous —
        // `HashMap` order decides which `Command::env` call lands last —
        // so pick deterministically rather than let the fingerprint churn.
        env.iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case(name))
            .min_by(|a, b| a.0.cmp(b.0))
            .map(|(_, v)| v)
    }
    #[cfg(not(windows))]
    {
        env.get(name)
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

    /// `resolve_exe` does not search the working directory — it walks the
    /// child's PATH, the agent's own directory, then the system ones. A
    /// `srv.exe` dropped into the launch directory must therefore not
    /// capture the binding, or replacing the `srv.exe` that PATH really
    /// resolves to would leave the durable grant matching.
    #[cfg(windows)]
    #[test]
    fn the_launch_directory_does_not_win_over_path() {
        let cwd = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        std::fs::write(cwd.path().join("srv.exe"), "planted").unwrap();
        std::fs::write(bin.path().join("srv.exe"), "real").unwrap();

        let mut env = HashMap::new();
        env.insert("PATH".to_string(), bin.path().to_str().unwrap().to_string());

        assert_eq!(
            resolve_executable("srv", cwd.path(), &env),
            bin.path().join("srv.exe").canonicalize().ok(),
            "a planted executable in the launch directory captured the binding"
        );
    }

    /// An extensionless program is looked up *only* as `name.exe`:
    /// `resolve_exe` substitutes the extension rather than trying the
    /// bare name first, so a data file named `srv` must not resolve.
    #[cfg(windows)]
    #[test]
    fn an_extensionless_neighbour_does_not_resolve() {
        let bin = tempfile::tempdir().unwrap();
        std::fs::write(bin.path().join("srv"), "not what spawns").unwrap();
        std::fs::write(bin.path().join("srv.exe"), "MZ").unwrap();

        let mut env = HashMap::new();
        env.insert("PATH".to_string(), bin.path().to_str().unwrap().to_string());

        assert_eq!(
            resolve_executable("srv", Path::new("/"), &env),
            bin.path().join("srv.exe").canonicalize().ok(),
            "the extensionless neighbour captured the binding"
        );
    }

    /// Windows environment blocks are case-insensitive and `Command::env`
    /// keys them that way, so `Path` in a server's env overrides `PATH`
    /// for the child. Missing that resolves along the agent's own PATH
    /// and stops tracking swaps inside the configured one.
    #[cfg(windows)]
    #[test]
    fn a_case_insensitive_path_override_decides_the_resolution() {
        let configured = tempfile::tempdir().unwrap();
        let inherited = tempfile::tempdir().unwrap();
        std::fs::write(configured.path().join("srv.exe"), "configured").unwrap();
        std::fs::write(inherited.path().join("srv.exe"), "inherited").unwrap();

        let _guard = crate::test_support::EnvGuard::set("PATH", inherited.path());
        let mut env = HashMap::new();
        env.insert(
            "Path".to_string(),
            configured.path().to_str().unwrap().to_string(),
        );

        assert_eq!(
            resolve_executable("srv", Path::new("/"), &env),
            configured.path().join("srv.exe").canonicalize().ok(),
            "a lowercase Path override was ignored"
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
