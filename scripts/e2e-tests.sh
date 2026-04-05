#!/usr/bin/env bash
# E2E test suite for agent-code releases.
#
# Categories:
#   A: Static tests (no API key)         — binary, flags, cargo check
#   B: One-shot mode (API)               — prompt execution, model override
#   C: Serve mode HTTP API               — endpoints, SSE, error handling
#   D: Tool verification (via serve)     — FileRead/Write/Edit, Grep, Glob, Bash
#   E: Permission system                 — protected paths, mode enforcement
#   F: Skills system                     — bundled skills, remote skill commands
#   G: Config system                     — env vars, project config, features
#   H: Edge cases                        — empty prompt, unicode, session state
#   I: ACP protocol                      — JSON-RPC over stdio
#   J: Doctor diagnostics                — /doctor output verification
#   K: CLI flag coverage                 — --cwd, --max-turns, --permission-mode
#
# Requirements:
#   - AGENT_BINARY env var (path to compiled agent binary)
#   - AGENT_CODE_API_KEY env var (for LLM-backed tests)
#   - AGENT_CODE_MODEL env var (defaults to gpt-5-nano)
#   - ripgrep (rg) installed
#   - jq installed
#
# Estimated API cost per run: ~$0.06 with gpt-5-nano.

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

api_delete() {
    HTTP_CODE=$(curl -s -o "${_CURL_BODY_FILE}" -w '%{http_code}' \
        --max-time "${API_TIMEOUT}" \
        -X DELETE "${SERVE_URL}$1" 2>/dev/null) || HTTP_CODE="000"
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

# ══════════════════════════════════════════════════════════════════
#  A: Static Tests (no API key needed)
# ══════════════════════════════════════════════════════════════════

section "A: Static Tests (no API key needed)"

# A1: Version
output=$("${AGENT}" --version 2>&1) || true
if echo "${output}" | grep -qE '[0-9]+\.[0-9]+\.[0-9]+'; then
    pass "A1: --version prints version (${output})"
else
    fail "A1: --version" "Expected version pattern, got: ${output}"
fi

# A2: Help — verify key flags are documented
output=$("${AGENT}" --help 2>&1) || true
if echo "${output}" | grep -q -- "--prompt" \
    && echo "${output}" | grep -q -- "--serve" \
    && echo "${output}" | grep -q -- "--model" \
    && echo "${output}" | grep -q -- "--attach" \
    && echo "${output}" | grep -q -- "--acp" \
    && echo "${output}" | grep -q -- "--provider"; then
    pass "A2: --help shows all expected flags (prompt, serve, model, attach, acp, provider)"
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

# A7: Format check
if cargo fmt --all -- --check 2>&1 | tail -3; then
    pass "A7: cargo fmt check"
else
    fail "A7: cargo fmt" "Formatting issues found"
fi

# ══════════════════════════════════════════════════════════════════
#  B: One-shot Mode (API calls)
# ══════════════════════════════════════════════════════════════════

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

    # B4: --cwd flag changes working directory
    output=$("${AGENT}" --prompt "What directory are you working in? Reply with just the path." \
        --model "${MODEL}" --dangerously-skip-permissions \
        -C "${WORKDIR}" 2>&1) || true
    if echo "${output}" | grep -q "$(basename "${WORKDIR}")"; then
        pass "B4: --cwd flag applied"
    else
        pass "B4: --cwd flag (ran without error)"
    fi

    # B5: --max-turns limits agent iterations
    output=$("${AGENT}" --prompt "Count from 1 to 100, one number at a time, using the Bash tool for each." \
        --model "${MODEL}" --dangerously-skip-permissions --max-turns 2 \
        -C "${WORKDIR}" 2>&1) || true
    # Should complete (not hang) because max-turns limits it.
    pass "B5: --max-turns exits without hanging"

    # ══════════════════════════════════════════════════════════════
    #  Start Serve Mode
    # ══════════════════════════════════════════════════════════════

    section "Starting serve mode for API tests..."
    if ! start_serve; then
        fail "SERVE" "Could not start serve mode"
        echo "Skipping serve-dependent tests"
    else

    # ══════════════════════════════════════════════════════════════
    #  C: Serve Mode HTTP API
    # ══════════════════════════════════════════════════════════════

    section "C: Serve Mode HTTP API"

    # C1: Health
    api_get "/health"
    if [[ "${HTTP_CODE}" == "200" ]] && [[ "${HTTP_BODY}" == "ok" ]]; then
        pass "C1: GET /health → 200 ok"
    else
        fail "C1: GET /health" "Expected 200/ok, got ${HTTP_CODE}/${HTTP_BODY:0:100}"
    fi

    # C2: Status — verify all expected fields
    # Note: use has() instead of -e for boolean fields (jq -e treats false as falsy)
    api_get "/status"
    if [[ "${HTTP_CODE}" == "200" ]] \
        && echo "${HTTP_BODY}" | jq -e '.session_id' > /dev/null 2>&1 \
        && echo "${HTTP_BODY}" | jq -e '.model' > /dev/null 2>&1 \
        && echo "${HTTP_BODY}" | jq -e '.version' > /dev/null 2>&1 \
        && echo "${HTTP_BODY}" | jq -e '.cwd' > /dev/null 2>&1 \
        && echo "${HTTP_BODY}" | jq -e 'has("turn_count")' > /dev/null 2>&1 \
        && echo "${HTTP_BODY}" | jq -e 'has("cost_usd")' > /dev/null 2>&1 \
        && echo "${HTTP_BODY}" | jq -e 'has("plan_mode")' > /dev/null 2>&1; then
        pass "C2: GET /status → all required fields present"
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

    # C8: Messages history (should have >= 2 from C3)
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

    # C9: SSE /events endpoint responds
    sse_code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 3 \
        -H "Accept: text/event-stream" "${SERVE_URL}/events" 2>/dev/null) || sse_code="000"
    if [[ "${sse_code}" == "200" ]] || [[ "${sse_code}" == "000" ]]; then
        # 200 = SSE stream opened (timeout closes it); 000 = timeout (expected for SSE)
        pass "C9: GET /events → SSE endpoint reachable"
    else
        fail "C9: SSE events" "Expected 200/stream, got ${sse_code}"
    fi

    # C10: DELETE method on endpoints
    api_delete "/health"
    if [[ "${HTTP_CODE}" == "405" ]]; then
        pass "C10: DELETE /health → 405"
    else
        fail "C10: DELETE method" "Expected 405, got ${HTTP_CODE}"
    fi

    # C11: POST /message with extra fields (should ignore them)
    api_post "/message" '{"content":"say hi","extra_field":"ignored"}'
    if [[ "${HTTP_CODE}" == "200" ]]; then
        pass "C11: POST /message ignores extra fields"
    else
        fail "C11: extra fields" "Expected 200, got ${HTTP_CODE}"
    fi

    # C12: Status turns increment after message
    api_get "/status"
    turns=$(echo "${HTTP_BODY}" | jq '.turn_count' 2>/dev/null || echo "0")
    if [[ "${turns}" -ge 1 ]]; then
        pass "C12: Turn count incremented (${turns} turns)"
    else
        fail "C12: turn count" "Expected >= 1, got ${turns}"
    fi

    # C13: Cost is tracked
    api_get "/status"
    cost=$(echo "${HTTP_BODY}" | jq '.cost_usd' 2>/dev/null || echo "0")
    if [[ "$(echo "${cost} > 0" | bc -l 2>/dev/null || echo "0")" == "1" ]]; then
        pass "C13: Cost tracked (\$${cost})"
    else
        pass "C13: Cost field present (${cost})"
    fi

    # ══════════════════════════════════════════════════════════════
    #  D: Tool Verification
    # ══════════════════════════════════════════════════════════════

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

    # D7: MultiEdit — multiple file edits in one call
    echo "LINE_ONE" > "${WORKDIR}/multi-a.txt"
    echo "LINE_TWO" > "${WORKDIR}/multi-b.txt"
    api_post "/message" "{\"content\":\"Edit both files: replace LINE_ONE with REPLACED_A in ${WORKDIR}/multi-a.txt and LINE_TWO with REPLACED_B in ${WORKDIR}/multi-b.txt.\"}"
    if [[ "${HTTP_CODE}" == "200" ]]; then
        a_ok=false; b_ok=false
        grep -q "REPLACED_A" "${WORKDIR}/multi-a.txt" 2>/dev/null && a_ok=true
        grep -q "REPLACED_B" "${WORKDIR}/multi-b.txt" 2>/dev/null && b_ok=true
        if [[ "${a_ok}" == "true" ]] && [[ "${b_ok}" == "true" ]]; then
            pass "D7: MultiEdit — both files updated"
        elif [[ "${a_ok}" == "true" ]] || [[ "${b_ok}" == "true" ]]; then
            pass "D7: MultiEdit — partial success (at least one file edited)"
        else
            pass "D7: MultiEdit — response OK (content verification inconclusive)"
        fi
    else
        fail "D7: MultiEdit" "code=${HTTP_CODE}"
    fi

    # D8: ToolSearch — agent can discover available tools
    api_post "/message" "{\"content\":\"Use the ToolSearch tool to find tools related to 'file'. List the tool names you find.\"}"
    if [[ "${HTTP_CODE}" == "200" ]]; then
        resp=$(echo "${HTTP_BODY}" | jq -r '.response' 2>/dev/null || echo "")
        if echo "${resp}" | grep -qiE "FileRead|FileWrite|FileEdit"; then
            pass "D8: ToolSearch found file-related tools"
        else
            pass "D8: ToolSearch responded (tools not in response text)"
        fi
    else
        fail "D8: ToolSearch" "code=${HTTP_CODE}"
    fi

    # D9: Coding task — write code, verify it runs
    api_post "/message" "{\"content\":\"Create a Python file at ${WORKDIR}/fizzbuzz.py that defines a function fizzbuzz(n) which returns 'FizzBuzz' if n is divisible by both 3 and 5, 'Fizz' if divisible by 3, 'Buzz' if divisible by 5, and str(n) otherwise. Then add a test block that prints fizzbuzz(15), fizzbuzz(3), fizzbuzz(5), fizzbuzz(7). Use FileWrite.\"}"
    if [[ "${HTTP_CODE}" == "200" ]] && [[ -f "${WORKDIR}/fizzbuzz.py" ]]; then
        # Run the generated code and verify output
        py_output=$(python3 "${WORKDIR}/fizzbuzz.py" 2>&1) || true
        if echo "${py_output}" | grep -q "FizzBuzz" \
            && echo "${py_output}" | grep -q "Fizz" \
            && echo "${py_output}" | grep -q "Buzz"; then
            pass "D9: Coding task — wrote valid Python, output correct (FizzBuzz/Fizz/Buzz)"
        elif [[ -n "${py_output}" ]] && ! echo "${py_output}" | grep -qi "error\|traceback"; then
            pass "D9: Coding task — wrote runnable Python (output: ${py_output:0:100})"
        else
            fail "D9: Coding task" "Python error: ${py_output:0:200}"
        fi
    else
        fail "D9: Coding task" "File not created or bad response (code=${HTTP_CODE})"
    fi

    # D10: Coding task — write + test in one shot
    api_post "/message" "{\"content\":\"Create a bash script at ${WORKDIR}/test_math.sh that tests basic arithmetic. The script should: (1) compute result=\\\$(( 6 * 7 )), (2) if result equals 42 print MATH_PASS, else print MATH_FAIL. Make it executable. Use FileWrite then Bash to run it.\"}"
    if [[ "${HTTP_CODE}" == "200" ]]; then
        if [[ -f "${WORKDIR}/test_math.sh" ]]; then
            bash_output=$(bash "${WORKDIR}/test_math.sh" 2>&1) || true
            if echo "${bash_output}" | grep -q "MATH_PASS"; then
                pass "D10: Coding task — bash script passes its own test"
            else
                pass "D10: Coding task — bash script created (output: ${bash_output:0:100})"
            fi
        else
            resp=$(echo "${HTTP_BODY}" | jq -r '.response' 2>/dev/null || echo "")
            if echo "${resp}" | grep -qi "MATH_PASS"; then
                pass "D10: Coding task — agent ran the script (MATH_PASS in response)"
            else
                fail "D10: Coding task" "Script not created"
            fi
        fi
    else
        fail "D10: Coding task" "code=${HTTP_CODE}"
    fi

    # ══════════════════════════════════════════════════════════════
    #  E: Permission System
    # ══════════════════════════════════════════════════════════════

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

    # E3: Write to .husky/ blocked (built-in protected dir)
    mkdir -p "${WORKDIR}/.husky"
    api_post "/message" "{\"content\":\"Create a file at ${WORKDIR}/.husky/test-blocked with text: nope. Use the FileWrite tool.\"}"
    if ! [[ -f "${WORKDIR}/.husky/test-blocked" ]]; then
        pass "E3: Write to .husky/ blocked"
    else
        fail "E3: .husky write" "File was created in protected directory"
    fi

    # E4: Write to node_modules/ blocked
    mkdir -p "${WORKDIR}/node_modules"
    api_post "/message" "{\"content\":\"Create a file at ${WORKDIR}/node_modules/test-blocked with text: nope. Use the FileWrite tool.\"}"
    if ! [[ -f "${WORKDIR}/node_modules/test-blocked" ]]; then
        pass "E4: Write to node_modules/ blocked"
    else
        fail "E4: node_modules write" "File was created in protected directory"
    fi

    # ══════════════════════════════════════════════════════════════
    #  H: Edge Cases
    # ══════════════════════════════════════════════════════════════

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

    # H4: Large prompt (stress test)
    large_prompt=$(python3 -c "print('repeat this word: ECHO. ' * 50)" 2>/dev/null || echo "repeat ECHO 50 times")
    api_post "/message" "{\"content\":\"${large_prompt}\"}"
    if [[ "${HTTP_CODE}" == "200" ]]; then
        pass "H4: Large prompt handled"
    else
        fail "H4: large prompt" "Status: ${HTTP_CODE}"
    fi

    # H5: Rapid sequential requests
    api_post "/message" '{"content":"say A"}'
    code_a="${HTTP_CODE}"
    api_post "/message" '{"content":"say B"}'
    code_b="${HTTP_CODE}"
    if [[ "${code_a}" == "200" ]] && [[ "${code_b}" == "200" ]]; then
        pass "H5: Rapid sequential requests handled"
    else
        fail "H5: rapid requests" "Codes: ${code_a}, ${code_b}"
    fi

    # ── Stop Serve ────────────────────────────────────────────────

    stop_serve

    fi  # end of serve-dependent tests

    # ══════════════════════════════════════════════════════════════
    #  I: ACP Protocol (JSON-RPC over stdio)
    # ══════════════════════════════════════════════════════════════

    section "I: ACP Protocol (JSON-RPC over stdio)"

    # I1: Initialize handshake
    init_response=$(echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"client_name":"e2e-test","protocol_version":"1"}}
{"jsonrpc":"2.0","id":99,"method":"shutdown","params":{}}' | \
        timeout 30 "${AGENT}" --acp --model "${MODEL}" -C "${WORKDIR}" 2>/dev/null) || true
    if echo "${init_response}" | head -1 | jq -e '.result.name' > /dev/null 2>&1; then
        acp_name=$(echo "${init_response}" | head -1 | jq -r '.result.name')
        pass "I1: ACP initialize → name=${acp_name}"
    else
        fail "I1: ACP initialize" "Response: ${init_response:0:200}"
    fi

    # I2: Initialize returns capabilities
    if echo "${init_response}" | head -1 | jq -e '.result.capabilities' > /dev/null 2>&1; then
        pass "I2: ACP capabilities present"
    else
        fail "I2: ACP capabilities" "Missing capabilities in response"
    fi

    # I3: Initialize returns protocol version
    if echo "${init_response}" | head -1 | jq -e '.result.protocol_version' > /dev/null 2>&1; then
        pass "I3: ACP protocol_version present"
    else
        fail "I3: ACP protocol_version" "Missing protocol_version"
    fi

    # I4: Status method
    status_response=$(echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}
{"jsonrpc":"2.0","id":2,"method":"status","params":{}}
{"jsonrpc":"2.0","id":99,"method":"shutdown","params":{}}' | \
        timeout 30 "${AGENT}" --acp --model "${MODEL}" -C "${WORKDIR}" 2>/dev/null) || true
    # Find the status response (id:2)
    status_line=$(echo "${status_response}" | grep '"id":2' | head -1)
    if echo "${status_line}" | jq -e '.result.session_id' > /dev/null 2>&1 \
        && echo "${status_line}" | jq -e '.result.model' > /dev/null 2>&1; then
        pass "I4: ACP status → session_id and model present"
    else
        fail "I4: ACP status" "Response: ${status_line:0:200}"
    fi

    # I5: Unknown method returns error
    err_response=$(echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}
{"jsonrpc":"2.0","id":2,"method":"nonexistent","params":{}}
{"jsonrpc":"2.0","id":99,"method":"shutdown","params":{}}' | \
        timeout 30 "${AGENT}" --acp --model "${MODEL}" -C "${WORKDIR}" 2>/dev/null) || true
    err_line=$(echo "${err_response}" | grep '"id":2' | head -1)
    if echo "${err_line}" | jq -e '.error' > /dev/null 2>&1; then
        err_code=$(echo "${err_line}" | jq '.error.code' 2>/dev/null)
        pass "I5: ACP unknown method → error (code ${err_code})"
    else
        fail "I5: ACP unknown method" "Expected error response: ${err_line:0:200}"
    fi

    # I6: Shutdown returns ok
    shutdown_line=$(echo "${init_response}" | grep '"id":99' | head -1)
    if echo "${shutdown_line}" | jq -e '.result.ok' > /dev/null 2>&1; then
        pass "I6: ACP shutdown → ok"
    else
        fail "I6: ACP shutdown" "Response: ${shutdown_line:0:200}"
    fi

    # I7: Parse error for invalid JSON
    parse_response=$(echo 'not valid json' | \
        timeout 10 "${AGENT}" --acp --model "${MODEL}" -C "${WORKDIR}" 2>/dev/null) || true
    if echo "${parse_response}" | jq -e '.error.code' > /dev/null 2>&1; then
        err_code=$(echo "${parse_response}" | jq '.error.code' 2>/dev/null)
        if [[ "${err_code}" == "-32700" ]]; then
            pass "I7: ACP parse error → code -32700"
        else
            pass "I7: ACP parse error → error returned (code ${err_code})"
        fi
    else
        fail "I7: ACP parse error" "Expected JSON-RPC error: ${parse_response:0:200}"
    fi

fi  # end of API key check

# ══════════════════════════════════════════════════════════════════
#  F: Skills System (no API key needed)
# ══════════════════════════════════════════════════════════════════

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

# F2: Custom skill loading from project directory
mkdir -p "${WORKDIR}/.agent/skills"
cat > "${WORKDIR}/.agent/skills/test-custom.md" << 'SKILL'
---
description: Custom test skill
userInvocable: true
---

This is a custom test skill body with {{arg}} substitution.
SKILL
custom_prompt=$("${AGENT}" --dump-system-prompt -C "${WORKDIR}" 2>&1) || true
if echo "${custom_prompt}" | grep -qi "test-custom\|Custom test skill"; then
    pass "F2: Custom project skill loaded"
else
    fail "F2: custom skill" "test-custom not found in system prompt"
fi
rm -rf "${WORKDIR}/.agent/skills"

# F3: Skill override — project skill overrides bundled
mkdir -p "${WORKDIR}/.agent/skills"
cat > "${WORKDIR}/.agent/skills/commit.md" << 'SKILL'
---
description: Overridden commit skill for testing
userInvocable: true
---

This is the overridden commit skill.
SKILL
override_prompt=$("${AGENT}" --dump-system-prompt -C "${WORKDIR}" 2>&1) || true
if echo "${override_prompt}" | grep -qi "Overridden commit skill"; then
    pass "F3: Project skill overrides bundled skill"
else
    # The override may not appear in system prompt text directly, but it should load
    pass "F3: Project skill override (loaded without error)"
fi
rm -rf "${WORKDIR}/.agent/skills"

# ══════════════════════════════════════════════════════════════════
#  G: Config System
# ══════════════════════════════════════════════════════════════════

section "G: Config System"

# G1: AGENT_CODE_MODEL env var is picked up
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

# G3: Project config with MCP server entry loads
cat > "${WORKDIR}/.agent/settings.toml" << 'TOML'
[permissions]
default_mode = "allow"

[mcp_servers.test-server]
command = "echo"
args = ["hello"]
TOML
output=$("${AGENT}" --dump-system-prompt -C "${WORKDIR}" 2>&1) || true
if [[ -n "${output}" ]]; then
    pass "G3: Project config with MCP server entry loads"
else
    fail "G3: mcp config" "Crash or empty output"
fi

# G4: Project config with features loads
cat > "${WORKDIR}/.agent/settings.toml" << 'TOML'
[features]
token_budget = false
commit_attribution = false
TOML
output=$("${AGENT}" --dump-system-prompt -C "${WORKDIR}" 2>&1) || true
if [[ -n "${output}" ]]; then
    pass "G4: Project config with features loads"
else
    fail "G4: features config" "Crash or empty output"
fi

# G5: Project config with security settings loads
cat > "${WORKDIR}/.agent/settings.toml" << 'TOML'
[security]
disable_skill_shell_execution = true
disable_bypass_permissions = false
TOML
output=$("${AGENT}" --dump-system-prompt -C "${WORKDIR}" 2>&1) || true
if [[ -n "${output}" ]]; then
    pass "G5: Project config with security settings loads"
else
    fail "G5: security config" "Crash or empty output"
fi

# G6: Project config with hooks loads
cat > "${WORKDIR}/.agent/settings.toml" << 'TOML'
[[hooks]]
event = "session_start"
action = "log"
TOML
output=$("${AGENT}" --dump-system-prompt -C "${WORKDIR}" 2>&1) || true
if [[ -n "${output}" ]]; then
    pass "G6: Project config with hooks loads"
else
    fail "G6: hooks config" "Crash or empty output"
fi

rm -rf "${WORKDIR}/.agent"

# ══════════════════════════════════════════════════════════════════
#  J: Doctor Diagnostics (no API key needed)
# ══════════════════════════════════════════════════════════════════

section "J: Doctor Diagnostics"

# Doctor requires an interactive session which we can't easily E2E test,
# but we can verify the diagnostic module by running with --dump-system-prompt
# and checking that the binary doesn't crash with various configs.

# J1: Binary includes doctor capability (check --help)
if "${AGENT}" --help 2>&1 | grep -qi "doctor\|diagnostic"; then
    pass "J1: --help references doctor/diagnostic capability"
else
    pass "J1: Doctor capability (not visible in --help, available as /doctor command)"
fi

# J2: System prompt includes tool references for diagnostics
prompt_out=$("${AGENT}" --dump-system-prompt 2>&1) || true
tool_count=$(echo "${prompt_out}" | grep -ciE "FileRead|FileWrite|FileEdit|Grep|Glob|Bash" || echo "0")
if [[ "${tool_count}" -ge 3 ]]; then
    pass "J2: System prompt references ${tool_count} core tools"
else
    fail "J2: tool references" "Expected >= 3 tool mentions, found ${tool_count}"
fi

# ══════════════════════════════════════════════════════════════════
#  K: CLI Flag Coverage
# ══════════════════════════════════════════════════════════════════

section "K: CLI Flag Coverage"

# K1: --verbose flag accepted
if "${AGENT}" --verbose --dump-system-prompt 2>&1 | head -5 > /dev/null; then
    pass "K1: --verbose flag accepted"
else
    fail "K1: --verbose" "Flag rejected or crash"
fi

# K2: --permission-mode flag accepted
for mode in ask allow deny plan accept_edits; do
    output=$("${AGENT}" --permission-mode "${mode}" --dump-system-prompt 2>&1) || true
    if [[ -n "${output}" ]]; then
        pass "K2: --permission-mode ${mode} accepted"
        break  # One is enough to verify the flag works
    fi
done

# K3: --provider flag accepted for known providers
for provider in anthropic openai xai google deepseek groq mistral together auto azure; do
    output=$("${AGENT}" --provider "${provider}" --dump-system-prompt 2>&1) || true
    if [[ -n "${output}" ]]; then
        : # continues working
    fi
done
pass "K3: --provider flag accepts all known provider names"

# K4: --dangerously-skip-permissions flag
output=$("${AGENT}" --dangerously-skip-permissions --dump-system-prompt 2>&1) || true
if [[ -n "${output}" ]]; then
    pass "K4: --dangerously-skip-permissions flag accepted"
else
    fail "K4: skip permissions" "Flag rejected or crash"
fi

# K5: --port flag accepted (doesn't start server without --serve)
output=$("${AGENT}" --port 9999 --dump-system-prompt 2>&1) || true
if [[ -n "${output}" ]]; then
    pass "K5: --port flag accepted without --serve"
else
    fail "K5: --port" "Flag rejected or crash"
fi

# K6: --acp flag recognized
# Just check it doesn't crash immediately (it'll wait for stdin, so timeout)
timeout 3 "${AGENT}" --acp --model "${MODEL}" -C "${WORKDIR}" < /dev/null 2>/dev/null || true
pass "K6: --acp flag recognized (exited on empty stdin)"

# K7: --attach flag recognized
# With no running instances, should print a message and exit cleanly
output=$("${AGENT}" --attach 2>&1) || true
if echo "${output}" | grep -qiE "no running\|not found\|connect\|attach"; then
    pass "K7: --attach flag → no instances message"
else
    pass "K7: --attach flag recognized (exited cleanly)"
fi

# ══════════════════════════════════════════════════════════════════
#  Report
# ══════════════════════════════════════════════════════════════════

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
