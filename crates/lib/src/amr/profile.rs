//! Scan profiles: the task-specific half of AMR.
//!
//! A [`Profile`] bundles the deterministic selectors that decide which
//! code is relevant with the prompt preambles that tell MAP and REDUCE
//! workers what to look for. The engine itself is task-agnostic; swapping
//! the profile is what turns "find vulnerabilities" into "find dead code"
//! or "find breaking changes".
//!
//! The vertical slice ships one built-in profile, [`security_profile`].
//! Its selectors are a recall-oriented net: being a little broad is fine
//! because the MAP worker applies a false-positive gate. Additional
//! profiles and user-authored (TOML) profiles are future work.

use regex::Regex;

use super::selectors::{Lang, Selector, SelectorKind};

/// A named analysis profile.
pub struct Profile {
    pub name: &'static str,
    pub description: &'static str,
    /// Deterministic relevance tests run over the whole tree.
    pub selectors: Vec<Selector>,
    /// Instructions prepended to each MAP worker prompt.
    pub investigate_preamble: &'static str,
    /// Instructions prepended to the REDUCE prompt.
    pub reduce_preamble: &'static str,
    /// Human-readable severity rubric, echoed into prompts.
    pub severity_rubric: &'static str,
}

fn lex(id: &str, description: &str, langs: Vec<Lang>, re: &str) -> Selector {
    Selector {
        id: id.to_string(),
        description: description.to_string(),
        langs,
        kind: SelectorKind::Lexical {
            pattern: Regex::new(re).expect("built-in selector regex must compile"),
        },
    }
}

fn ast_call(
    id: &str,
    description: &str,
    langs: Vec<Lang>,
    node_kinds: &[&str],
    callee: &str,
) -> Selector {
    Selector {
        id: id.to_string(),
        description: description.to_string(),
        langs,
        kind: SelectorKind::Ast {
            node_kinds: node_kinds.iter().map(|s| s.to_string()).collect(),
            callee: Some(Regex::new(callee).expect("built-in selector regex must compile")),
        },
    }
}

/// The built-in security profile: whole-repo vulnerability discovery.
pub fn security_profile() -> Profile {
    use Lang::*;
    let selectors = vec![
        // --- Remote code execution / command injection -----------------
        ast_call(
            "rce.py_dangerous_call",
            "Python call to a code/command execution sink",
            vec![Python],
            &["call"],
            r"^(os\.system|os\.popen|subprocess\.(Popen|call|run|check_output|check_call)|eval|exec|__import__|compile)\b",
        ),
        ast_call(
            "rce.js_dangerous_call",
            "JavaScript call to a code/command execution sink",
            vec![JavaScript],
            &["call_expression"],
            r"(child_process|\.exec(Sync)?\s*\(|\beval\s*\(|new Function|vm\.runIn)",
        ),
        lex(
            "rce.subprocess_shell_true",
            "subprocess invoked with shell=True",
            vec![Python],
            r"shell\s*=\s*True",
        ),
        lex(
            "rce.node_exec",
            "Node child_process exec sink",
            vec![JavaScript, TypeScript],
            r"child_process|\bexecSync\b|\bexec\s*\(",
        ),
        lex(
            "rce.go_exec",
            "Go command execution sink",
            vec![Go],
            r"\bexec\.Command\b|os/exec|syscall\.(Exec|ForkExec)",
        ),
        lex(
            "rce.ruby_exec",
            "Ruby command execution sink",
            vec![Ruby],
            r"(\bsystem\s*\(|\bexec\s*\(|%x\{|Open3\.|Kernel\.(system|spawn|exec)|\bIO\.popen|--exec)",
        ),
        lex(
            "rce.php_exec",
            "PHP command execution sink",
            vec![Php],
            r"\b(shell_exec|exec|system|passthru|proc_open|popen)\s*\(",
        ),
        lex(
            "rce.java_exec",
            "Java command execution sink",
            vec![Java, Scala, Kotlin],
            r"(Runtime\.getRuntime\(\)\s*\.\s*exec|new\s+ProcessBuilder|ProcessBuilder\s*\()",
        ),
        lex(
            "rce.rust_exec",
            "Rust process execution sink",
            vec![Rust],
            r"(process::)?Command::new",
        ),
        lex(
            "rce.c_exec",
            "C/C++ process/command execution sink",
            vec![C, Cpp],
            r"\b(system|popen|execve|execl|execlp|execvp)\s*\(",
        ),
        // --- Insecure deserialization ----------------------------------
        lex(
            "deser.python",
            "Insecure Python deserialization sink",
            vec![Python],
            r"(pickle\s*\.\s*loads?|marshal\s*\.\s*loads?|yaml\s*\.\s*load\s*\(|jsonpickle)",
        ),
        lex(
            "deser.ruby",
            "Unsafe Ruby deserialization sink",
            vec![Ruby],
            r"(Marshal\.load|YAML\.load\b|Oj\.load|Psych\.(load|unsafe_load))",
        ),
        lex(
            "deser.php",
            "PHP unserialize of possibly untrusted data",
            vec![Php],
            r"\bunserialize\s*\(",
        ),
        lex(
            "deser.java",
            "Unsafe Java/JVM deserialization sink",
            vec![Java, Scala, Kotlin],
            r"(ObjectInputStream|readObject\s*\(|readUnshared|XMLDecoder|enableDefaultTyping|new\s+Yaml\s*\()",
        ),
        lex(
            "deser.dotnet",
            "Unsafe .NET deserialization sink",
            vec![CSharp],
            r"(BinaryFormatter|LosFormatter|NetDataContractSerializer|ObjectStateFormatter|JavaScriptSerializer|TypeNameHandling)",
        ),
        // --- XML external entities (XXE) --------------------------------
        lex(
            "xxe.parser",
            "XML parser that may resolve external entities",
            vec![],
            r"(DocumentBuilderFactory|SAXParserFactory|XMLInputFactory|SchemaFactory|TransformerFactory|XMLReader|libxml_disable_entity_loader|Nokogiri::XML|etree\.parse|lxml\.etree)",
        ),
        // --- SQL injection ---------------------------------------------
        lex(
            "sqli.raw_query",
            "Raw SQL execution with possible string building",
            vec![Python, JavaScript, TypeScript, Go, Java, Ruby, Php, CSharp, Scala, Kotlin],
            r"(execute|executemany|executescript|query|rawQuery|prepare)\s*\(",
        ),
        lex(
            "sqli.fstring",
            "SQL keyword inside an interpolated/concatenated string",
            vec![],
            r#"(?i)(select|insert into|update|delete from)\b[^;\n]*(\{|%s|\$\{|"\s*\+|'\s*\+|f")"#,
        ),
        // --- Server-side request forgery -------------------------------
        lex(
            "ssrf.http_client",
            "Outbound HTTP request (possible SSRF sink)",
            vec![Python, JavaScript, TypeScript],
            r"(requests\.(get|post|put|delete|head|request)|urllib\.request\.urlopen|axios\.|fetch\s*\(|http\.get)",
        ),
        lex(
            "ssrf.go",
            "Go outbound HTTP request (possible SSRF sink)",
            vec![Go],
            r"(http\.(Get|Post|Head|NewRequest|NewRequestWithContext)|net\.Dial)",
        ),
        lex(
            "ssrf.ruby",
            "Ruby outbound HTTP request (possible SSRF sink)",
            vec![Ruby],
            r"(Net::HTTP|open-uri|URI\.(open|parse)|HTTParty|Faraday|RestClient)",
        ),
        lex(
            "ssrf.jvm",
            "JVM outbound HTTP request (possible SSRF sink)",
            vec![Java, Scala, Kotlin],
            r"(HttpURLConnection|HttpClient|RestTemplate|WebClient|new\s+URL\s*\()",
        ),
        lex(
            "ssrf.php",
            "PHP outbound request (possible SSRF sink)",
            vec![Php],
            r"(curl_exec|curl_init|fsockopen|file_get_contents\s*\(\s*\$)",
        ),
        lex(
            "ssrf.rust",
            "Rust outbound HTTP request (possible SSRF sink)",
            vec![Rust],
            r"(reqwest::|hyper::Client|ureq::)",
        ),
        // --- Path traversal --------------------------------------------
        lex(
            "path.open_concat",
            "File open with a concatenated/interpolated path",
            vec![Python, JavaScript, TypeScript],
            r#"(open|readFile|readFileSync|sendFile|createReadStream)\s*\([^)\n]*(\+|\{|\$\{|%s|os\.path\.join)"#,
        ),
        lex(
            "path.ruby",
            "Ruby file access (possible path traversal)",
            vec![Ruby],
            r"(File\.(read|open|join|expand_path|new|binread)|IO\.(read|binread)|send_file|Rack::(File|Static)|Dir\[)",
        ),
        lex(
            "path.go",
            "Go file access (possible path traversal)",
            vec![Go],
            r"(os\.(Open|OpenFile|ReadFile)|ioutil\.ReadFile|http\.ServeFile|filepath\.Join)",
        ),
        lex(
            "path.jvm",
            "JVM file access (possible path traversal)",
            vec![Java, Scala, Kotlin],
            r"(new\s+File\s*\(|Paths\.get|Files\.(newInputStream|readAllBytes|copy)|FileInputStream|getResourceAsStream)",
        ),
        lex(
            "path.php",
            "PHP file access (possible path traversal / LFI)",
            vec![Php],
            r"\b(fopen|readfile|file_get_contents|fpassthru)\s*\(|\b(include|require)(_once)?\b[^;'\x22\n]*\$",
        ),
        lex(
            "path.rust",
            "Rust file access (possible path traversal)",
            vec![Rust],
            r"(File::open|File::create|fs::(read|read_to_string|write|File)|Path::new|PathBuf::from)",
        ),
        lex(
            "path.c",
            "C/C++ file open (possible path traversal)",
            vec![C, Cpp],
            r"\b(fopen|openat|open)\s*\(",
        ),
        lex(
            "path.archive_slip",
            "Archive extraction (zip-slip / tar traversal surface)",
            vec![],
            r"(?i)(zipentry|getnextentry|tarentry|extractall|extract\s*\(|unzip|untar)",
        ),
        // --- Prototype pollution (JS/TS) -------------------------------
        lex(
            "proto.pollution",
            "Prototype-pollution sink",
            vec![JavaScript, TypeScript],
            r"(__proto__|constructor\s*\.\s*prototype|prototype\s*\[|Object\.assign|deepmerge|mergeWith)",
        ),
        // --- Regular-expression denial of service -----------------------
        lex(
            "redos.regex",
            "Dynamically built or nested-quantifier regex (ReDoS surface)",
            vec![JavaScript, TypeScript, Python, Ruby, Java, Scala, Elixir],
            r"(new\s+RegExp\s*\(|re\.compile\s*\(|Pattern\.compile\s*\(|Regex\.new|~r\(|\([^)\n]*[+*][^)\n]*\)\s*[+*])",
        ),
        // --- Memory safety (Rust / C / C++) -----------------------------
        lex(
            "mem.rust_unsafe",
            "Rust unchecked/unsafe memory operation",
            vec![Rust],
            r"(\bunsafe\b|get_unchecked(_mut)?|from_raw_parts(_mut)?|::transmute|\.offset\s*\(|\.add\s*\(|MaybeUninit|assume_init|from_utf8_unchecked)",
        ),
        lex(
            "mem.c_unsafe_fn",
            "C/C++ unbounded buffer operation",
            vec![C, Cpp],
            r"\b(strcpy|strcat|sprintf|vsprintf|gets|stpcpy|wcscpy|memcpy|memmove|alloca)\s*\(",
        ),
        // --- Integer overflow / narrowing -------------------------------
        lex(
            "int.narrowing_cast",
            "Narrowing integer cast (overflow/truncation surface)",
            vec![Go, Rust, C, Cpp],
            r"(\b(u?int(8|16|32)|i(8|16|32)|u(8|16|32))\s*\(|\bas\s+[iu](8|16|32)\b|\(\s*(short|u?int(8|16)_t)\s*\))",
        ),
        // --- Template injection ----------------------------------------
        lex(
            "ssti.render_string",
            "Template rendered from a string (possible SSTI)",
            vec![Python],
            r"render_template_string\s*\(",
        ),
        // --- Cross-site scripting / unescaped output --------------------
        lex(
            "xss.unescaped_output",
            "Unescaped value rendered into output",
            vec![],
            r"(<%=|\.html_safe\b|\braw\s*\(|innerHTML\s*=|dangerouslySetInnerHTML|document\.write\s*\(|\becho\s+\$|Response\.Write)",
        ),
        // --- Weak cryptography -----------------------------------------
        lex(
            "crypto.weak_hash",
            "Weak hash primitive",
            vec![Python, JavaScript, TypeScript, Go, Java, Ruby, Php, CSharp],
            r"(?i)\b(md5|sha1)\b\s*[\(:]",
        ),
        // --- Hardcoded secrets -----------------------------------------
        lex(
            "secret.aws_key",
            "AWS access key id literal",
            vec![],
            r"AKIA[0-9A-Z]{16}",
        ),
        lex(
            "secret.private_key",
            "Embedded private key",
            vec![],
            r"-----BEGIN (RSA |EC |OPENSSH |DSA |PGP )?PRIVATE KEY-----",
        ),
        lex(
            "secret.assignment",
            "Credential-like literal assignment",
            vec![],
            r#"(?i)(password|passwd|secret|api[_-]?key|access[_-]?token|auth[_-]?token)\s*[:=]\s*["'][^"'\n]{6,}["']"#,
        ),
        // --- Authentication / access-control boundary ------------------
        lex(
            "auth.boundary",
            "Auth/permission boundary marker",
            vec![],
            r"(?i)(@(login_required|permission_required|requires_auth|authenticated|preauthorize|rolesallowed|secured))\b|\b(authorize|authenticate|has_?role|is_?admin|check_?permission|before_action|require_?auth)\b",
        ),
    ];

    Profile {
        name: "security",
        description: "Find exploitable vulnerabilities across the whole repository via Agentic MapReduce.",
        selectors,
        investigate_preamble: INVESTIGATE_PREAMBLE,
        reduce_preamble: REDUCE_PREAMBLE,
        severity_rubric: SEVERITY_RUBRIC,
    }
}

const SEVERITY_RUBRIC: &str = "\
P0 = remotely exploitable, no authentication, integrity/confidentiality loss.
P1 = exploitable with preconditions or authentication.
P2 = requires local access or unusual configuration.";

const INVESTIGATE_PREAMBLE: &str = "\
You are a security investigator examining ONE shard of a larger codebase.
You are given only the files and signals for this shard. Do not assume
anything about the rest of the repository.

For every candidate signal:
  1. Read the real code around it with the read-only tools you have.
  2. Decide whether a genuine, exploitable vulnerability exists.
  3. Apply the false-positive gate: if you cannot articulate a concrete
     exploit path and its preconditions, DO NOT report it.

Work efficiently and stay in scope. Read the files named in the signals and
the immediate code around each signal; you may open a directly imported
helper when it is essential, but do NOT crawl the wider repository or run
broad searches. As soon as you have assessed this shard's signals, output
your JSON and stop.

Account for every file you were handed. Prefer a small number of real,
well-evidenced findings over many speculative ones.";

const REDUCE_PREAMBLE: &str = "\
You are given deduplicated candidate findings from many independent shard
workers (their conclusions only, not their transcripts, and not the whole
repository). Your job:
  1. Deduplicate findings that describe the same root cause in the same place.
  2. Reconcile conflicting severities and keep the best-evidenced verdict.
  3. Compose ATTACK CHAINS: sequences where lower-severity findings combine
     into a higher-impact one (e.g. an unauthenticated ID disclosure plus an
     ID-gated RCE becomes one P0 unauthenticated RCE).
  4. Produce a single prioritised list, most severe first.
Only reason over the findings provided; do not invent new ones.";

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn security_profile_selectors_all_compile_and_have_unique_ids() {
        let p = security_profile();
        assert!(!p.selectors.is_empty());
        let mut ids: Vec<_> = p.selectors.iter().map(|s| s.id.clone()).collect();
        ids.sort();
        let before = ids.len();
        ids.dedup();
        assert_eq!(before, ids.len(), "selector ids must be unique");
    }

    #[test]
    fn security_profile_flags_a_classic_python_rce() {
        let p = security_profile();
        let text = "import os\ndef handler(req):\n    os.system('ping ' + req.args['host'])\n";
        let signals: Vec<_> = p
            .selectors
            .iter()
            .flat_map(|s| s.scan_text(Path::new("views.py"), text))
            .collect();
        assert!(
            signals
                .iter()
                .any(|s| s.selector_id == "rce.py_dangerous_call"),
            "os.system call should produce a signal, got {signals:?}"
        );
    }

    #[test]
    fn security_profile_flags_a_hardcoded_secret() {
        let p = security_profile();
        let text = "const config = { api_key: \"sk-supersecretvalue123\" }\n";
        let signals: Vec<_> = p
            .selectors
            .iter()
            .flat_map(|s| s.scan_text(Path::new("config.js"), text))
            .collect();
        assert!(signals.iter().any(|s| s.selector_id == "secret.assignment"));
    }

    #[test]
    fn security_profile_ignores_clean_code() {
        let p = security_profile();
        let text = "def add(a, b):\n    return a + b\n";
        let signals: Vec<_> = p
            .selectors
            .iter()
            .flat_map(|s| s.scan_text(Path::new("math_utils.py"), text))
            .collect();
        assert!(
            signals.is_empty(),
            "benign code should emit no signals, got {signals:?}"
        );
    }

    /// Assert that scanning `text` at `path` produces at least one signal
    /// from the selector with id `want`.
    fn fires(path: &str, text: &str, want: &str) {
        let p = security_profile();
        let hit = p
            .selectors
            .iter()
            .flat_map(|s| s.scan_text(Path::new(path), text))
            .any(|s| s.selector_id == want);
        assert!(hit, "expected selector `{want}` to fire on {path}");
    }

    #[test]
    fn ruby_file_access_is_selected() {
        // rack-style path handling that the old Python/JS-only path selector missed.
        fires(
            "lib/rack/static.rb",
            "path = Utils.unescape(env[\"PATH_INFO\"])\nFile.open(::File.join(root, path))\n",
            "path.ruby",
        );
    }

    #[test]
    fn rust_unsafe_memory_op_is_selected() {
        fires(
            "src/lib.rs",
            "pub fn get2(&self, i: usize) -> &T {\n    unsafe { self.slots.get_unchecked(i) }\n}\n",
            "mem.rust_unsafe",
        );
    }

    #[test]
    fn js_prototype_pollution_is_selected() {
        fires(
            "src/parse.js",
            "if (key === '__proto__') { target[key] = value }\n",
            "proto.pollution",
        );
    }

    #[test]
    fn php_unserialize_is_selected() {
        fires(
            "app/controller.php",
            "$data = unserialize($_POST['payload']);\n",
            "deser.php",
        );
    }

    #[test]
    fn dotnet_deserialization_is_selected() {
        fires(
            "src/CopyData.cs",
            "var fmt = new BinaryFormatter();\nreturn fmt.Deserialize(stream);\n",
            "deser.dotnet",
        );
    }

    #[test]
    fn jvm_xxe_parser_is_selected() {
        fires(
            "src/main/java/org/cyclonedx/CycloneDxSchema.java",
            "SchemaFactory sf = SchemaFactory.newInstance(XMLConstants.W3C_XML_SCHEMA_NS_URI);\n",
            "xxe.parser",
        );
    }

    #[test]
    fn go_command_execution_is_selected() {
        fires(
            "internal/database/pull.go",
            "cmd := exec.Command(\"git\", \"rebase\", \"--exec\", script)\n",
            "rce.go_exec",
        );
    }

    #[test]
    fn c_unbounded_buffer_op_is_selected() {
        fires(
            "code/Material/MaterialSystem.cpp",
            "char buf[256];\nstrcpy(buf, prop->mKey.data);\n",
            "mem.c_unsafe_fn",
        );
    }

    #[test]
    fn newly_supported_extensions_classify() {
        use crate::amr::selectors::Lang;
        assert_eq!(Lang::from_path(Path::new("a.cs")), Some(Lang::CSharp));
        assert_eq!(Lang::from_path(Path::new("a.scala")), Some(Lang::Scala));
        assert_eq!(Lang::from_path(Path::new("a.kt")), Some(Lang::Kotlin));
        assert_eq!(Lang::from_path(Path::new("a.swift")), Some(Lang::Swift));
        assert_eq!(Lang::from_path(Path::new("a.dart")), Some(Lang::Dart));
        assert_eq!(Lang::from_path(Path::new("a.ex")), Some(Lang::Elixir));
    }
}
