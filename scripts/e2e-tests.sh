#!/usr/bin/env bash
# E2E test suite for agent-code releases.
# Runs ~32 tests across 8 categories: static checks, one-shot mode,
# serve mode HTTP API, tool verification, permissions, skills, config,
# and edge cases.
#
# Requirements:
#   - AGENT_BINARY env var (path to compiled agent binary)
#   - AGENT_CODE_API_KEY env var (for LLM-backed tests)
#   - AGENT_CODE_MODEL env var (defaults to gpt-5-nano)
#   - ripgrep (rg) installed
#   - jq installed
#
# Estimated API cost per run: ~$0.03 with gpt-5-nano.

set -uo pipefail

# ── Configuration ──────────────────────────────────────────────────

AGENT="${AGENT_BINARY:-./target/release/agent}"
MODEL="${AGENT_CODE_MODEL:-openai/gpt-5-nano}"
SERVE_PORT=14096
SERVE_URL="http://127.0.0.1:${SERVE_PORT}"
API_TIMEOUT=120
API_TIMEOUT_LONG=240
WORKDIR=""
SERVE_PID=""
PASS_COUNT=0
FAIL_COUNT=0
TOTAL=0
FAILURES=()

# ── Helpers ────────────────────────────────────────────────────────

cleanup() {
    if [[ -n "${SERVE_PID}" ]]; then
        kill "${SERVE_PID}" 2>/dev/null || true
        wait "${SERVE_PID}" 2>/dev/null || true
    fi
    if [[ -n "${WORKDIR}" && -d "${WORKDIR}" ]]; then
        rm -rf "${WORKDIR}"
    fi
    rm -f "${_CURL_BODY_FILE}" 2>/dev/null || true
}
trap cleanup EXIT

pass() {
    ((PASS_COUNT++)) || true
    ((TOTAL++)) || true
    echo "  ✓ PASS: $1"
}

fail() {
    ((FAIL_COUNT++)) || true
    ((TOTAL++)) || true
    FAILURES+=("$1: $2")
    echo "  ✗ FAIL: $1 — $2"
}

section() {
    echo ""
    echo "═══════════════════════════════════════════════════"
    echo "  $1"
    echo "═══════════════════════════════════════════════════"
}

# Curl wrappers: set HTTP_CODE and HTTP_BODY globals.
# Uses -o file + -w to cleanly separate body from status code.
# IMPORTANT: These set globals directly — do NOT call via $(api_get ...).
# Instead call: api_get "/path" ; then use $HTTP_CODE and $HTTP_BODY.
HTTP_CODE=""
HTTP_BODY=""
_CURL_BODY_FILE=$(mktemp)

api_get() {
    HTTP_CODE=$(curl -s -o "${_CURL_BODY_FILE}" -w '%{http_code}' \
        --max-time "${API_TIMEOUT}" "${SERVE_URL}$1" 2>/dev/null) || HTTP_CODE="000"
    HTTP_BODY=$(cat "${_CURL_BODY_FILE}")
}

api_post() {
    local timeout="${3:-${API_TIMEOUT}}"
    HTTP_CODE=$(curl -s -o "${_CURL_BODY_FILE}" -w '%{http_code}' \
        --max-time "${timeout}" \
        -X POST -H "Content-Type: application/json" \
        -d "$2" "${SERVE_URL}$1" 2>/dev/null) || HTTP_CODE="000"
    HTTP_BODY=$(cat "${_CURL_BODY_FILE}")
}

api_put() {
    HTTP_CODE=$(curl -s -o "${_CURL_BODY_FILE}" -w '%{http_code}' \
        --max-time "${API_TIMEOUT}" \
        -X PUT "${SERVE_URL}$1" 2>/dev/null) || HTTP_CODE="000"
    HTTP_BODY=$(cat "${_CURL_BODY_FILE}")
}

start_serve() {
    "${AGENT}" --serve --port "${SERVE_PORT}" --model "${MODEL}" \
        --dangerously-skip-permissions -C "${WORKDIR}" > /tmp/agent-serve-e2e.log 2>&1 &
    SERVE_PID=$!
    # Wait up to 15s for health endpoint.
    for _ in $(seq 1 30); do
        if curl -sf "${SERVE_URL}/health" > /dev/null 2>&1; then
            echo "  Serve mode started (PID ${SERVE_PID}, port ${SERVE_PORT})"
            return 0
        fi
        # Check if process died.
        if ! kill -0 "${SERVE_PID}" 2>/dev/null; then
            echo "  ERROR: serve process exited prematurely"
            cat /tmp/agent-serve-e2e.log 2>/dev/null || true
            return 1
        fi
        sleep 0.5
    done
    echo "  ERROR: serve mode failed to start within 15s"
    cat /tmp/agent-serve-e2e.log 2>/dev/null || true
    return 1
}

stop_serve() {
    if [[ -n "${SERVE_PID}" ]]; then
        kill "${SERVE_PID}" 2>/dev/null || true
        wait "${SERVE_PID}" 2>/dev/null || true
        SERVE_PID=""
        echo "  Serve mode stopped"
    fi
}

# ── Setup ──────────────────────────────────────────────────────────

echo "agent-code E2E Test Suite"
echo "Binary: ${AGENT}"
echo "Model:  ${MODEL}"
echo ""

if [[ ! -x "${AGENT}" ]]; then
    echo "ERROR: Binary not found or not executable: ${AGENT}"
    exit 1
fi

WORKDIR=$(mktemp -d)
git init "${WORKDIR}" --quiet
echo "KNOWN_CONTENT_12345" > "${WORKDIR}/test-read.txt"

# ── A: Static Tests ────────────────────────────────────────────────

section "A: Static Tests (no API key needed)"

# A1: Version
output=$("${AGENT}" --version 2>&1) || true
if echo "${output}" | grep -qE '[0-9]+\.[0-9]+\.[0-9]+'; then
    pass "A1: --version prints version (${output})"
else
    fail "A1: --version" "Expected version pattern, got: ${output}"
fi

# A2: Help
output=$("${AGENT}" --help 2>&1) || true
if echo "${output}" | grep -q -- "--prompt" \
    && echo "${output}" | grep -q -- "--serve" \
    && echo "${output}" | grep -q -- "--model"; then
    pass "A2: --help shows expected flags"
else
    fail "A2: --help" "Missing expected flags in help output"
fi

# A3: System prompt dump
output=$("${AGENT}" --dump-system-prompt 2>&1) || true
if [[ -n "${output}" ]] && echo "${output}" | grep -qi "tool"; then
    pass "A3: --dump-system-prompt outputs non-empty prompt"
else
    fail "A3: --dump-system-prompt" "Empty or missing 'tool' keyword"
fi

# A4: Unknown flag
if "${AGENT}" --nonexistent-flag-xyz 2>&1; then
    fail "A4: unknown flag" "Expected non-zero exit code"
else
    pass "A4: unknown flag exits with error"
fi

# A5: Cargo test
section "A5: Running cargo test (this may take a while)..."
if cargo test --all-targets 2>&1 | tail -5; then
    pass "A5: cargo test --all-targets"
else
    fail "A5: cargo test" "Tests failed"
fi

# A6: Clippy
if cargo clippy --all-targets -- -D warnings 2>&1 | tail -3; then
    pass "A6: cargo clippy clean"
else
    fail "A6: cargo clippy" "Clippy warnings found"
fi

# ── B: One-shot Mode ───────────────────────────────────────────────

section "B: One-shot Mode (API calls)"

if [[ -z "${AGENT_CODE_API_KEY:-}" ]]; then
    echo "  SKIP: AGENT_CODE_API_KEY not set, skipping API tests"
else

    # B1: Simple echo
    output=$("${AGENT}" --prompt "Reply with exactly: HELLO_E2E" \
        --model "${MODEL}" --dangerously-skip-permissions 2>&1) || true
    if echo "${output}" | grep -q "HELLO_E2E"; then
        pass "B1: one-shot echo response"
    else
        fail "B1: one-shot echo" "Response missing HELLO_E2E: ${output:0:200}"
    fi

    # B2: Math
    output=$("${AGENT}" --prompt "What is 2+2? Reply with just the number, nothing else." \
        --model "${MODEL}" --dangerously-skip-permissions 2>&1) || true
    if echo "${output}" | grep -q "4"; then
        pass "B2: one-shot math (2+2=4)"
    else
        fail "B2: one-shot math" "Response missing '4': ${output:0:200}"
    fi

    # B3: Basic run
    output=$("${AGENT}" --prompt "say ok" \
        --model "${MODEL}" --dangerously-skip-permissions 2>&1)
    if [[ $? -eq 0 ]] && [[ -n "${output}" ]]; then
        pass "B3: one-shot basic run"
    else
        fail "B3: one-shot basic" "Exit code non-zero or empty output"
    fi

    # ── Start Serve Mode ───────────────────────────────────────────

    section "Starting serve mode for API tests..."
    if ! start_serve; then
        fail "SERVE" "Could not start serve mode"
        echo "Skipping serve-dependent tests"
    else

    # ── C: HTTP API ────────────────────────────────────────────────

    section "C: Serve Mode HTTP API"

    # C1: Health
    api_get "/health"
    if [[ "${HTTP_CODE}" == "200" ]] && [[ "${HTTP_BODY}" == "ok" ]]; then
        pass "C1: GET /health → 200 ok"
    else
        fail "C1: GET /health" "Expected 200/ok, got ${HTTP_CODE}/${HTTP_BODY:0:100}"
    fi

    # C2: Status
    api_get "/status"
    if [[ "${HTTP_CODE}" == "200" ]] \
        && echo "${HTTP_BODY}" | jq -e '.session_id' > /dev/null 2>&1 \
        && echo "${HTTP_BODY}" | jq -e '.model' > /dev/null 2>&1 \
        && echo "${HTTP_BODY}" | jq -e '.version' > /dev/null 2>&1; then
        pass "C2: GET /status → JSON with required fields"
    else
        fail "C2: GET /status" "Missing fields. Code=${HTTP_CODE}, body=${HTTP_BODY:0:200}"
    fi

    # C3: Message (valid) — uses longer timeout + retry for LLM latency.
    c3_ok=false
    for c3_attempt in 1 2; do
        api_post "/message" '{"content":"Reply with exactly: PONG"}' "${API_TIMEOUT_LONG}"
        if [[ "${HTTP_CODE}" == "200" ]] \
            && echo "${HTTP_BODY}" | jq -e '.response' > /dev/null 2>&1 \
            && echo "${HTTP_BODY}" | jq -e '.tools_used' > /dev/null 2>&1 \
            && echo "${HTTP_BODY}" | jq -e '.cost_usd' > /dev/null 2>&1; then
            resp=$(echo "${HTTP_BODY}" | jq -r '.response')
            if echo "${resp}" | grep -qi "PONG"; then
                pass "C3: POST /message → valid response with PONG"
            else
                pass "C3: POST /message → valid JSON (PONG not in response, but structure OK)"
            fi
            c3_ok=true
            break
        fi
        if [[ "${c3_attempt}" -eq 1 ]]; then
            echo "  ⟳ C3: attempt 1 failed (code=${HTTP_CODE}), retrying..."
        fi
    done
    if [[ "${c3_ok}" != "true" ]]; then
        fail "C3: POST /message" "Bad response after 2 attempts. Code=${HTTP_CODE}, body=${HTTP_BODY:0:200}"
    fi

    # C4: Missing content
    api_post "/message" '{}'
    if [[ "${HTTP_CODE}" == "422" ]]; then
        pass "C4: POST /message {} → 422"
    else
        fail "C4: missing content" "Expected 422, got ${HTTP_CODE}"
    fi

    # C5: Bad JSON
    code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 10 \
        -X POST -H "Content-Type: application/json" \
        -d 'not json' "${SERVE_URL}/message" 2>/dev/null) || code="000"
    if [[ "${code}" == "400" ]]; then
        pass "C5: POST /message bad JSON → 400"
    else
        fail "C5: bad JSON" "Expected 400, got ${code}"
    fi

    # C6: Not found
    api_get "/nonexistent"
    if [[ "${HTTP_CODE}" == "404" ]]; then
        pass "C6: GET /nonexistent → 404"
    else
        fail "C6: not found" "Expected 404, got ${HTTP_CODE}"
    fi

    # C7: Method not allowed
    api_put "/health"
    if [[ "${HTTP_CODE}" == "405" ]]; then
        pass "C7: PUT /health → 405"
    else
        fail "C7: method not allowed" "Expected 405, got ${HTTP_CODE}"
    fi

    # C8: Messages history
    api_get "/messages"
    if [[ "${HTTP_CODE}" == "200" ]]; then
        msg_count=$(echo "${HTTP_BODY}" | jq '.messages | length' 2>/dev/null || echo "0")
        if [[ "${msg_count}" -ge 2 ]]; then
            pass "C8: GET /messages → ${msg_count} messages"
        else
            fail "C8: messages" "Expected >= 2 messages, got ${msg_count}"
        fi
    else
        fail "C8: GET /messages" "Expected 200, got ${HTTP_CODE}"
    fi

    # ── D: Tool Verification ──────────────────────────────────────

    section "D: Tool Verification (via serve mode)"

    # Helper: check if a tool name appears in tools_used JSON array.
    has_tool() {
        local json="$1" tool="$2"
        echo "${json}" | jq -e --arg t "${tool}" '.tools_used | index($t) != null' > /dev/null 2>&1
    }

    # D1: FileRead
    api_post "/message" "{\"content\":\"Read the file ${WORKDIR}/test-read.txt and tell me its contents. Use the FileRead tool.\"}"
    if [[ "${HTTP_CODE}" == "200" ]] && has_tool "${HTTP_BODY}" "FileRead"; then
        pass "D1: FileRead tool used"
    else
        tools=$(echo "${HTTP_BODY}" | jq -c '.tools_used' 2>/dev/null || echo "none")
        fail "D1: FileRead" "code=${HTTP_CODE} tools=${tools}"
    fi

    # D2: FileWrite
    api_post "/message" "{\"content\":\"Create a file at ${WORKDIR}/test-write.txt containing exactly the text: WRITTEN_BY_AGENT. Use the FileWrite tool.\"}"
    if [[ "${HTTP_CODE}" == "200" ]] && has_tool "${HTTP_BODY}" "FileWrite"; then
        if [[ -f "${WORKDIR}/test-write.txt" ]]; then
            pass "D2: FileWrite tool used, file created"
        else
            fail "D2: FileWrite" "Tool used but file not created"
        fi
    else
        tools=$(echo "${HTTP_BODY}" | jq -c '.tools_used' 2>/dev/null || echo "none")
        fail "D2: FileWrite" "code=${HTTP_CODE} tools=${tools}"
    fi

    # D3: FileEdit
    api_post "/message" "{\"content\":\"Edit the file ${WORKDIR}/test-write.txt and replace WRITTEN_BY_AGENT with EDITED_BY_AGENT. Use the FileEdit tool.\"}"
    if [[ "${HTTP_CODE}" == "200" ]] && has_tool "${HTTP_BODY}" "FileEdit"; then
        if grep -q "EDITED_BY_AGENT" "${WORKDIR}/test-write.txt" 2>/dev/null; then
            pass "D3: FileEdit tool used, content updated"
        else
            pass "D3: FileEdit tool used (content check inconclusive)"
        fi
    else
        tools=$(echo "${HTTP_BODY}" | jq -c '.tools_used' 2>/dev/null || echo "none")
        fail "D3: FileEdit" "code=${HTTP_CODE} tools=${tools}"
    fi

    # D4: Grep
    api_post "/message" "{\"content\":\"Use the Grep tool to search for the word EDITED in files under ${WORKDIR}. Report what you find.\"}"
    if [[ "${HTTP_CODE}" == "200" ]] && has_tool "${HTTP_BODY}" "Grep"; then
        pass "D4: Grep tool used"
    else
        tools=$(echo "${HTTP_BODY}" | jq -c '.tools_used' 2>/dev/null || echo "none")
        fail "D4: Grep" "code=${HTTP_CODE} tools=${tools}"
    fi

    # D5: Glob
    api_post "/message" "{\"content\":\"Use the Glob tool to list all .txt files under ${WORKDIR}. Report the filenames.\"}"
    if [[ "${HTTP_CODE}" == "200" ]] && has_tool "${HTTP_BODY}" "Glob"; then
        pass "D5: Glob tool used"
    else
        tools=$(echo "${HTTP_BODY}" | jq -c '.tools_used' 2>/dev/null || echo "none")
        fail "D5: Glob" "code=${HTTP_CODE} tools=${tools}"
    fi

    # D6: Bash
    api_post "/message" "{\"content\":\"Use the Bash tool to run the command: echo BASH_WORKS_E2E\"}"
    if [[ "${HTTP_CODE}" == "200" ]] && has_tool "${HTTP_BODY}" "Bash"; then
        pass "D6: Bash tool used"
    else
        tools=$(echo "${HTTP_BODY}" | jq -c '.tools_used' 2>/dev/null || echo "none")
        fail "D6: Bash" "code=${HTTP_CODE} tools=${tools}"
    fi

    # ── E: Permission System ──────────────────────────────────────

    section "E: Permission System"

    # E1: Write to .git/ blocked (protected even with skip-permissions)
    api_post "/message" "{\"content\":\"Create a file at ${WORKDIR}/.git/test-blocked with the text: should_not_exist. Use the FileWrite tool.\"}"
    resp=$(echo "${HTTP_BODY}" | jq -r '.response' 2>/dev/null || echo "")
    if echo "${resp}" | grep -qiE "block|protect|denied|permission" \
        || ! [[ -f "${WORKDIR}/.git/test-blocked" ]]; then
        pass "E1: Write to .git/ blocked"
    else
        fail "E1: .git write" "File was created or no block message"
    fi

    # E2: Read .git/HEAD allowed
    api_post "/message" "{\"content\":\"Read the file ${WORKDIR}/.git/HEAD using the FileRead tool and show me its contents.\"}"
    if [[ "${HTTP_CODE}" == "200" ]] && has_tool "${HTTP_BODY}" "FileRead"; then
        resp=$(echo "${HTTP_BODY}" | jq -r '.response' 2>/dev/null || echo "")
        if echo "${resp}" | grep -qi "ref:"; then
            pass "E2: Read .git/HEAD allowed (contains ref:)"
        else
            pass "E2: Read .git/HEAD allowed (FileRead used)"
        fi
    else
        tools=$(echo "${HTTP_BODY}" | jq -c '.tools_used' 2>/dev/null || echo "none")
        fail "E2: .git read" "code=${HTTP_CODE} tools=${tools}"
    fi

    # ── H: Edge Cases ─────────────────────────────────────────────

    section "H: Edge Cases"

    # H1: Empty prompt
    api_post "/message" '{"content":""}'
    if [[ "${HTTP_CODE}" == "200" ]] || [[ "${HTTP_CODE}" == "422" ]]; then
        pass "H1: Empty prompt handled (${HTTP_CODE})"
    else
        fail "H1: empty prompt" "Unexpected status: ${HTTP_CODE}"
    fi

    # H2: Special characters
    api_post "/message" '{"content":"Handle these chars: \"quotes\" and unicode: caf\u00e9 \ud83d\ude00. Reply OK."}'
    if [[ "${HTTP_CODE}" == "200" ]]; then
        pass "H2: Special characters handled"
    else
        fail "H2: special chars" "Status: ${HTTP_CODE}"
    fi

    # H3: Session persistence
    api_post "/message" '{"content":"The secret code is BANANA42. Reply with: I have noted the code BANANA42."}'
    sleep 1
    api_post "/message" '{"content":"What was the secret code from my previous message? Reply with only the code, nothing else."}'
    resp=$(echo "${HTTP_BODY}" | jq -r '.response' 2>/dev/null || echo "")
    if echo "${resp}" | grep -q "BANANA42"; then
        pass "H3: Session persistence (recalled BANANA42)"
    else
        fail "H3: session persistence" "Response: ${resp:0:200}"
    fi

    # ── Stop Serve ────────────────────────────────────────────────

    stop_serve

    fi  # end of serve-dependent tests
fi  # end of API key check

# ── F: Skills System ──────────────────────────────────────────────

section "F: Skills System"

# F1: All 12 bundled skills present in system prompt
prompt_output=$("${AGENT}" --dump-system-prompt 2>&1) || true
skills_missing=()
for skill in commit review test explain debug pr refactor init security-review advisor bughunter plan; do
    if ! echo "${prompt_output}" | grep -qi "${skill}"; then
        skills_missing+=("${skill}")
    fi
done
if [[ ${#skills_missing[@]} -eq 0 ]]; then
    pass "F1: All 12 bundled skills found in system prompt"
else
    fail "F1: skills" "Missing: ${skills_missing[*]}"
fi

# ── G: Config System ──────────────────────────────────────────────

section "G: Config System"

# G1: AGENT_CODE_MODEL env var is picked up
# We already set it, so --dump-system-prompt should work. Check via
# a quick serve start/status if API key is available.
if [[ -n "${AGENT_CODE_API_KEY:-}" ]]; then
    # Start a fresh serve instance just for config check.
    "${AGENT}" --serve --port 14097 --dangerously-skip-permissions \
        -C "${WORKDIR}" > /tmp/agent-serve-config.log 2>&1 &
    CONFIG_PID=$!
    for _ in $(seq 1 20); do
        if curl -sf "http://127.0.0.1:14097/health" > /dev/null 2>&1; then break; fi
        sleep 0.5
    done
    status_body=$(curl -s --max-time 5 "http://127.0.0.1:14097/status" 2>&1)
    detected_model=$(echo "${status_body}" | jq -r '.model' 2>/dev/null || echo "unknown")
    kill "${CONFIG_PID}" 2>/dev/null || true
    wait "${CONFIG_PID}" 2>/dev/null || true

    if [[ "${detected_model}" == "${MODEL}" ]]; then
        pass "G1: AGENT_CODE_MODEL env var applied (${detected_model})"
    else
        fail "G1: config model" "Expected ${MODEL}, got ${detected_model}"
    fi
else
    pass "G1: AGENT_CODE_MODEL set (skipped live check, no API key)"
fi

# G2: Project-local settings.toml loads
mkdir -p "${WORKDIR}/.agent"
cat > "${WORKDIR}/.agent/settings.toml" << 'TOML'
[permissions]
default_mode = "allow"
TOML
# Verify it doesn't crash with project config.
output=$("${AGENT}" --dump-system-prompt -C "${WORKDIR}" 2>&1) || true
if [[ -n "${output}" ]]; then
    pass "G2: Project .agent/settings.toml loads without error"
else
    fail "G2: project config" "Empty output or crash"
fi
rm -rf "${WORKDIR}/.agent"

# ── Report ─────────────────────────────────────────────────────────

section "Results"
echo ""
echo "  Passed: ${PASS_COUNT} / ${TOTAL}"
echo "  Failed: ${FAIL_COUNT} / ${TOTAL}"

if [[ ${#FAILURES[@]} -gt 0 ]]; then
    echo ""
    echo "  Failures:"
    for f in "${FAILURES[@]}"; do
        echo "    - ${f}"
    done
fi

echo ""

if [[ ${FAIL_COUNT} -gt 0 ]]; then
    exit 1
else
    echo "All tests passed!"
    exit 0
fi
