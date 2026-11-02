# QWE POC orchestration. Platform-specific recipes write isolated evidence.
set shell := ["bash", "-euo", "pipefail", "-c"]

poc := justfile_directory() / "poc"

default:
    @just --list

doctor:
    #!/usr/bin/env bash
    set -u
    root="$(CDPATH= cd -- "{{ justfile_directory() }}" && pwd)"
    cd "$root"

    version_of() {
      if command -v "$1" >/dev/null 2>&1; then
        "$1" --version 2>&1 | sed -n '1p'
      else
        printf 'missing'
      fi
    }

    if command -v sw_vers >/dev/null 2>&1; then
      os="macOS $(sw_vers -productVersion)"
      kernel="$(uname -r)"
    elif [ -r /etc/os-release ]; then
      os="$(. /etc/os-release && printf '%s' "$PRETTY_NAME")"
      kernel="$(uname -r)"
    else
      os="$(uname -s)"
      kernel="$(uname -r)"
    fi

    if command -v nproc >/dev/null 2>&1; then
      cpu_cores="$(nproc)"
    elif command -v sysctl >/dev/null 2>&1; then
      cpu_cores="$(sysctl -n hw.ncpu 2>/dev/null || printf unknown)"
    else
      cpu_cores="unknown"
    fi

    if command -v sysctl >/dev/null 2>&1 &&
       memory_bytes="$(sysctl -n hw.memsize 2>/dev/null)"; then
      :
    elif [ -r /proc/meminfo ]; then
      memory_bytes="$(
        awk '/^MemTotal:/ { printf "%.0f\n", $2 * 1024 }' /proc/meminfo
      )"
    else
      memory_bytes="unknown"
    fi

    printf 'os=%s\n' "$os"
    printf 'kernel=%s\n' "$kernel"
    printf 'architecture=%s\n' "$(uname -m)"
    printf 'cpu_cores=%s\n' "$cpu_cores"
    printf 'memory_bytes=%s\n' "$memory_bytes"
    printf 'rustc=%s\n' "$(version_of rustc)"
    printf 'cargo=%s\n' "$(version_of cargo)"
    printf 'go=%s\n' "$(if command -v go >/dev/null 2>&1; then go version; else printf missing; fi)"
    printf 'cxx=%s\n' "$(version_of c++)"
    printf 'cmake=%s\n' "$(version_of cmake)"
    printf 'ninja=%s\n' "$(version_of ninja)"
    printf 'make=%s\n' "$(version_of make)"
    printf 'sqlite3=%s\n' "$(version_of sqlite3)"
    printf 'jq=%s\n' "$(version_of jq)"
    printf 'python3=%s\n' "$(if command -v python3 >/dev/null 2>&1; then python3 --version 2>&1; else printf missing; fi)"
    printf 'bash=%s\n' "$(version_of bash)"
    printf 'zsh=%s\n' "$(version_of zsh)"
    printf 'nvim=%s\n' "$(version_of nvim)"
    printf 'htop=%s\n' "$(version_of htop)"
    printf 'just=%s\n' "$(version_of just)"
    printf 'git_commit=%s\n' "$(git rev-parse --short HEAD 2>/dev/null || printf uncommitted)"
    printf 'worktree=%s\n' "$(if [ -z "$(git status --porcelain 2>/dev/null)" ]; then printf clean; else printf dirty; fi)"

build language="all":
    #!/usr/bin/env bash
    set -euo pipefail
    root="{{ poc }}"
    target="{{ language }}"
    build_root="$root/build"

    build_rust() {
      mkdir -p "$build_root/rust"
      CARGO_TARGET_DIR="$build_root/rust/cargo" \
        cargo build --release --manifest-path "$root/rust/Cargo.toml"
      cp "$build_root/rust/cargo/release/qwe-poc" "$build_root/rust/qwe-poc"
    }

    build_go() {
      mkdir -p "$build_root/go"
      (
        cd "$root/go"
        go build -trimpath -o "$build_root/go/qwe-poc" ./cmd/qwe-poc
      )
    }

    build_cpp() {
      mkdir -p "$build_root/cpp"
      cmake -S "$root/cpp" -B "$build_root/cpp" -G Ninja \
        -DCMAKE_BUILD_TYPE=Release
      cmake --build "$build_root/cpp"
    }

    case "$target" in
      rust) build_rust ;;
      go) build_go ;;
      cpp) build_cpp ;;
      all)
        build_rust
        build_go
        build_cpp
        ;;
      *)
        printf 'usage: just build [all|rust|go|cpp]\n' >&2
        exit 2
        ;;
    esac

run language command *args:
    "{{ poc }}/build/{{ language }}/qwe-poc" {{ command }} {{ args }}

verify-skeleton:
    #!/usr/bin/env bash
    set -euo pipefail
    root="{{ poc }}"
    expected='{"contract":"qwe-poc/0.1","implementation":"LANGUAGE"}'

    just --justfile "{{ justfile() }}" build all

    for language in rust go cpp; do
      actual="$("$root/build/$language/qwe-poc" probe)"
      expected_for_language="${expected/LANGUAGE/$language}"
      if [ "$actual" != "$expected_for_language" ]; then
        printf 'contract mismatch for %s\nexpected: %s\nactual:   %s\n' \
          "$language" "$expected_for_language" "$actual" >&2
        exit 1
      fi

      if "$root/build/$language/qwe-poc" unknown >/dev/null 2>&1; then
        printf 'unknown command unexpectedly succeeded for %s\n' "$language" >&2
        exit 1
      fi
      printf 'skeleton %-4s PASS\n' "$language"
    done
    printf 'POC skeleton verification PASS\n'

verify-fixtures:
    #!/usr/bin/env bash
    set -euo pipefail
    root="{{ poc }}"
    fixtures="$root/test-data/scripts"
    tmp="$(mktemp -d "${TMPDIR:-/tmp}/qwe-poc-fixtures.XXXXXX")"
    trap 'rm -rf "$tmp"' EXIT

    output="$("$fixtures/long_output.sh" 3)"
    expected="$(printf 'QWE-LINE-%08d\n' 3 2 1)"
    if [ "$output" != "$expected" ]; then
      printf 'long_output fixture mismatch\n' >&2
      exit 1
    fi

    "$fixtures/spawn_tree.sh" 3 0 2>"$tmp/tree.log"
    if [ "$(grep -c '^QWE-PROCESS ' "$tmp/tree.log")" -ne 3 ]; then
      printf 'spawn_tree produced the wrong depth\n' >&2
      exit 1
    fi

    set +e
    "$fixtures/child_crash.sh" fail >/dev/null 2>&1
    crash_status=$?
    set -e
    if [ "$crash_status" -ne 42 ]; then
      printf 'child_crash returned %s, expected 42\n' "$crash_status" >&2
      exit 1
    fi

    for fixture in signal_test timeout_test; do
      "$fixtures/$fixture.sh" 300 >"$tmp/$fixture.out" 2>"$tmp/$fixture.err" &
      fixture_pid=$!
      attempts=0
      while ! grep -q 'QWE-READY' "$tmp/$fixture.err"; do
        attempts=$((attempts + 1))
        if [ "$attempts" -gt 50 ]; then
          kill -KILL "$fixture_pid" 2>/dev/null || true
          printf '%s did not become ready\n' "$fixture" >&2
          exit 1
        fi
        sleep 0.02
      done

      kill -TERM "$fixture_pid"
      set +e
      wait "$fixture_pid"
      termination_status=$?
      set -e

      case "$fixture" in
        signal_test)
          expected_status=143
          marker='QWE-SIGNAL name=SIGTERM'
          ;;
        timeout_test)
          expected_status=124
          marker='QWE-TIMEOUT'
          ;;
      esac

      if [ "$termination_status" -ne "$expected_status" ] ||
         ! grep -q "$marker" "$tmp/$fixture.err"; then
        printf '%s termination mismatch (status=%s)\n' \
          "$fixture" "$termination_status" >&2
        exit 1
      fi
    done

    if ! "$fixtures/cpu_stress.sh" 1 1 >"$tmp/cpu.out" 2>"$tmp/cpu.err" ||
       ! grep -q 'QWE-CPU-READY workers=1' "$tmp/cpu.err"; then
      printf 'cpu_stress fixture failed\n' >&2
      exit 1
    fi

    if ! "$fixtures/memory_stress.sh" 8 1 >"$tmp/memory.out" \
       2>"$tmp/memory.err" ||
       ! grep -q 'QWE-MEMORY-READY' "$tmp/memory.err"; then
      printf 'memory_stress fixture failed\n' >&2
      exit 1
    fi

    sleep 30 &
    daemon_pid=$!
    "$fixtures/daemon_crash.sh" "$daemon_pid" >"$tmp/daemon.out" \
      2>"$tmp/daemon.err"
    set +e
    wait "$daemon_pid" 2>/dev/null
    daemon_status=$?
    set -e
    if [ "$daemon_status" -ne 137 ] ||
       ! grep -q "QWE-DAEMON-CRASH pid=$daemon_pid signal=KILL" \
         "$tmp/daemon.err"; then
      printf 'daemon_crash fixture failed (status=%s)\n' "$daemon_status" >&2
      exit 1
    fi
    printf 'shared fixture verification PASS\n'

terminal-smoke:
    #!/usr/bin/env bash
    set -euo pipefail
    case "$(uname -s)" in
      Darwin) platform="macos-$(uname -m)" ;;
      Linux) platform="linux-$(uname -m)" ;;
      *) printf 'unsupported platform: %s\n' "$(uname -s)" >&2; exit 2 ;;
    esac
    exec just --justfile "{{ justfile() }}" _terminal-smoke "$platform"

terminal-smoke-macos:
    #!/usr/bin/env bash
    set -euo pipefail
    [ "$(uname -s)" = "Darwin" ] || {
      printf 'macOS terminal smoke must run on Darwin\n' >&2
      exit 2
    }
    exec just --justfile "{{ justfile() }}" _terminal-smoke "macos-$(uname -m)"

terminal-smoke-linux:
    #!/usr/bin/env bash
    set -euo pipefail
    [ "$(uname -s)" = "Linux" ] || {
      printf 'Linux terminal smoke must run on Linux\n' >&2
      exit 2
    }
    exec just --justfile "{{ justfile() }}" _terminal-smoke "linux-$(uname -m)"

_terminal-smoke platform:
    #!/usr/bin/env bash
    set -euo pipefail
    root="{{ poc }}"
    repo_root="$(CDPATH= cd -- "$root/.." && pwd)"
    platform="{{ platform }}"
    stamp="$(date -u +%Y%m%dT%H%M%SZ)"
    run_id="$stamp-$$"
    source_commit="$(git -C "$repo_root" rev-parse --short HEAD 2>/dev/null || printf uncommitted)"
    os_name="$(uname -s)"
    architecture="$(uname -m)"
    started_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    failed=0
    results_root="$root/results/$platform"

    case "$platform" in
      macos-*)
        platform_limitation="macOS preflight run; not an official Linux comparison score"
        resize_limitation="macOS requires explicit SIGWINCH delivery after TIOCSWINSZ"
        ;;
      linux-*)
        platform_limitation="Linux run; confirm this environment matches the official comparison baseline"
        resize_limitation="Linux resize behavior was observed through the native PTY driver"
        ;;
      *)
        printf 'unsupported result platform: %s\n' "$platform" >&2
        exit 2
        ;;
    esac

    just --justfile "{{ justfile() }}" build all

    environment_dir="$results_root/environment"
    mkdir -p "$environment_dir"
    environment_file="$environment_dir/$run_id.txt"
    just --justfile "{{ justfile() }}" doctor >"$environment_file"

    for language in rust go cpp; do
      result_dir="$results_root/$language"
      mkdir -p "$result_dir"
      raw_name="T01-T02-$run_id.pty.log"
      stdout_name="T01-T02-$run_id.stdout.json"
      stderr_name="T01-T02-$run_id.stderr.log"
      raw_file="$result_dir/$raw_name"
      stdout_file="$result_dir/$stdout_name"
      stderr_file="$result_dir/$stderr_name"

      start_ms="$(python3 -c 'import time; print(time.time_ns() // 1_000_000)')"
      set +e
      "$root/build/$language/qwe-poc" pty-smoke "$raw_file" \
        >"$stdout_file" 2>"$stderr_file"
      binary_status=$?
      set -e
      end_ms="$(python3 -c 'import time; print(time.time_ns() // 1_000_000)')"
      duration_ms=$((end_ms - start_ms))

      summary="$(cat "$stdout_file")"
      summary_valid=false
      if printf '%s' "$summary" | jq -e \
        --arg language "$language" \
        '.feature == "pty-smoke" and
         .implementation == $language and
         .exit_code == 0 and
         .checks.shell == true and
         .checks.window_size == true' >/dev/null; then
        summary_valid=true
      fi

      markers_valid=false
      if grep -Fq 'QWE_PTY_READY' "$raw_file" &&
         grep -Fq 'QWE_PTY_SIZE 24 80' "$raw_file" &&
         grep -Fq 'QWE_PTY_END' "$raw_file"; then
        markers_valid=true
      fi

      case_status=fail
      if [ "$binary_status" -eq 0 ] &&
         [ "$summary_valid" = true ] &&
         [ "$markers_valid" = true ]; then
        case_status=pass
      else
        failed=1
      fi

      if [ -n "$summary" ] && printf '%s' "$summary" | jq -e . >/dev/null 2>&1; then
        metrics="$(printf '%s' "$summary" | jq -c '{bytes, exit_code}')"
        checks="$(printf '%s' "$summary" | jq -c '.checks')"
      else
        metrics='{}'
        checks='{}'
      fi

      for case_id in T01 T02; do
        result_file="$result_dir/$case_id-$source_commit-$run_id.json"
        jq -n \
          --arg case_id "$case_id" \
          --arg language "$language" \
          --arg status "$case_status" \
          --arg source_commit "$source_commit" \
          --arg os "$os_name" \
          --arg architecture "$architecture" \
          --arg toolchain "$(printf 'rust=%s; go=%s; cxx=%s' \
            "$(rustc --version 2>/dev/null || printf unknown)" \
            "$(go version 2>/dev/null || printf unknown)" \
            "$(c++ --version 2>/dev/null | sed -n '1p' || printf unknown)")" \
          --arg command "poc/build/$language/qwe-poc pty-smoke $raw_file" \
          --arg started_at "$started_at" \
          --argjson duration_ms "$duration_ms" \
          --argjson metrics "$metrics" \
          --argjson checks "$checks" \
          --arg raw "poc/results/$platform/$language/$raw_name" \
          --arg stdout "poc/results/$platform/$language/$stdout_name" \
          --arg stderr "poc/results/$platform/$language/$stderr_name" \
          --arg environment_record "poc/results/$platform/environment/$run_id.txt" \
          --arg platform_limitation "$platform_limitation" \
          '{
            case_id: $case_id,
            language: $language,
            status: $status,
            source_commit: $source_commit,
            environment: {
              os: $os,
              architecture: $architecture,
              toolchain: $toolchain,
              environment_record: $environment_record
            },
            command: $command,
            started_at: $started_at,
            duration_ms: $duration_ms,
            metrics: $metrics,
            checks: $checks,
            evidence: [$raw, $stdout, $stderr],
            known_limitations: [
              $platform_limitation,
              "Covers PTY creation, Shell launch, initial window size, and output capture",
              "Does not cover dynamic resize, Neovim screen behavior, or recovery"
            ]
          }' >"$result_file"
      done

      editor_raw_name="T03-$run_id.pty.log"
      editor_stdout_name="T03-$run_id.stdout.json"
      editor_stderr_name="T03-$run_id.stderr.log"
      editor_raw_file="$result_dir/$editor_raw_name"
      editor_stdout_file="$result_dir/$editor_stdout_name"
      editor_stderr_file="$result_dir/$editor_stderr_name"
      editor_started_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

      start_ms="$(python3 -c 'import time; print(time.time_ns() // 1_000_000)')"
      set +e
      "$root/build/$language/qwe-poc" editor-smoke "$editor_raw_file" \
        >"$editor_stdout_file" 2>"$editor_stderr_file"
      editor_binary_status=$?
      set -e
      end_ms="$(python3 -c 'import time; print(time.time_ns() // 1_000_000)')"
      editor_duration_ms=$((end_ms - start_ms))

      editor_summary="$(cat "$editor_stdout_file")"
      editor_summary_valid=false
      if printf '%s' "$editor_summary" | jq -e \
        --arg language "$language" \
        '.feature == "editor-smoke" and
         .implementation == $language and
         .exit_code == 0 and
         .checks.process == true and
         .checks.screen_text == true and
         .checks.window_size == true' >/dev/null; then
        editor_summary_valid=true
      fi

      editor_markers_valid=false
      if grep -Fq 'QWE_EDITOR_SIZE 24 80' "$editor_raw_file" &&
         grep -Fq 'QWE_NVIM_OK' "$editor_raw_file"; then
        editor_markers_valid=true
      fi

      editor_status=blocked
      if [ "$editor_binary_status" -ne 0 ] ||
         [ "$editor_summary_valid" != true ] ||
         [ "$editor_markers_valid" != true ]; then
        editor_status=fail
        failed=1
      fi

      if [ -n "$editor_summary" ] &&
         printf '%s' "$editor_summary" | jq -e . >/dev/null 2>&1; then
        editor_metrics="$(printf '%s' "$editor_summary" | jq -c '{bytes, exit_code}')"
        editor_checks="$(printf '%s' "$editor_summary" | jq -c '.checks')"
      else
        editor_metrics='{}'
        editor_checks='{}'
      fi

      editor_result_file="$result_dir/T03-$source_commit-$run_id.json"
      jq -n \
        --arg language "$language" \
        --arg status "$editor_status" \
        --arg source_commit "$source_commit" \
        --arg os "$os_name" \
        --arg architecture "$architecture" \
        --arg toolchain "$(printf 'rust=%s; go=%s; cxx=%s' \
          "$(rustc --version 2>/dev/null || printf unknown)" \
          "$(go version 2>/dev/null || printf unknown)" \
          "$(c++ --version 2>/dev/null | sed -n '1p' || printf unknown)")" \
        --arg command "poc/build/$language/qwe-poc editor-smoke $editor_raw_file" \
        --arg started_at "$editor_started_at" \
        --argjson duration_ms "$editor_duration_ms" \
        --argjson metrics "$editor_metrics" \
        --argjson checks "$editor_checks" \
        --arg raw "poc/results/$platform/$language/$editor_raw_name" \
        --arg stdout "poc/results/$platform/$language/$editor_stdout_name" \
        --arg stderr "poc/results/$platform/$language/$editor_stderr_name" \
        --arg environment_record "poc/results/$platform/environment/$run_id.txt" \
        --arg platform_limitation "$platform_limitation" \
        '{
          case_id: "T03",
          language: $language,
          status: $status,
          source_commit: $source_commit,
          environment: {
            os: $os,
            architecture: $architecture,
            toolchain: $toolchain,
            environment_record: $environment_record
          },
          command: $command,
          started_at: $started_at,
          duration_ms: $duration_ms,
          metrics: $metrics,
          checks: $checks,
          evidence: [$raw, $stdout, $stderr],
          known_limitations: [
            $platform_limitation,
            "Automates Neovim startup, initial rendering, and clean exit",
            "Manual cursor, full-screen, scroll, and interaction checks remain"
          ]
        }' >"$editor_result_file"

      resize_raw_name="T07-$run_id.pty.log"
      resize_stdout_name="T07-$run_id.stdout.json"
      resize_stderr_name="T07-$run_id.stderr.log"
      resize_raw_file="$result_dir/$resize_raw_name"
      resize_stdout_file="$result_dir/$resize_stdout_name"
      resize_stderr_file="$result_dir/$resize_stderr_name"
      resize_started_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

      start_ms="$(python3 -c 'import time; print(time.time_ns() // 1_000_000)')"
      set +e
      "$root/build/$language/qwe-poc" resize-smoke "$resize_raw_file" \
        >"$resize_stdout_file" 2>"$resize_stderr_file"
      resize_binary_status=$?
      set -e
      end_ms="$(python3 -c 'import time; print(time.time_ns() // 1_000_000)')"
      resize_duration_ms=$((end_ms - start_ms))

      resize_summary="$(cat "$resize_stdout_file")"
      resize_summary_valid=false
      if printf '%s' "$resize_summary" | jq -e \
        --arg language "$language" \
        '.feature == "pty-resize" and
         .implementation == $language and
         .exit_code == 0 and
         .checks.ready == true and
         .checks.size == true and
         .checks.winch == true and
         (.sizes | length == 4)' >/dev/null; then
        resize_summary_valid=true
      fi

      resize_markers_valid=false
      if grep -Fq 'QWE_RESIZE_READY 24 80' "$resize_raw_file" &&
         grep -Fq 'QWE_SIZE 40 120' "$resize_raw_file" &&
         grep -Fq 'QWE_SIZE 24 80' "$resize_raw_file" &&
         grep -Fq 'QWE_SIZE 50 160' "$resize_raw_file" &&
         grep -Fq 'QWE_SIZE 15 40' "$resize_raw_file" &&
         grep -Fq 'QWE_WINCH 40 120' "$resize_raw_file" &&
         grep -Fq 'QWE_WINCH 24 80' "$resize_raw_file" &&
         grep -Fq 'QWE_WINCH 50 160' "$resize_raw_file" &&
         grep -Fq 'QWE_WINCH 15 40' "$resize_raw_file"; then
        resize_markers_valid=true
      fi

      resize_status=fail
      if [ "$resize_binary_status" -eq 0 ] &&
         [ "$resize_summary_valid" = true ] &&
         [ "$resize_markers_valid" = true ]; then
        resize_status=pass
      else
        failed=1
      fi

      if [ -n "$resize_summary" ] &&
         printf '%s' "$resize_summary" | jq -e . >/dev/null 2>&1; then
        resize_metrics="$(printf '%s' "$resize_summary" | jq -c '{bytes, exit_code}')"
        resize_checks="$(printf '%s' "$resize_summary" | jq -c '.checks')"
      else
        resize_metrics='{}'
        resize_checks='{}'
      fi

      resize_result_file="$result_dir/T07-$source_commit-$run_id.json"
      jq -n \
        --arg language "$language" \
        --arg status "$resize_status" \
        --arg source_commit "$source_commit" \
        --arg os "$os_name" \
        --arg architecture "$architecture" \
        --arg toolchain "$(printf 'rust=%s; go=%s; cxx=%s' \
          "$(rustc --version 2>/dev/null || printf unknown)" \
          "$(go version 2>/dev/null || printf unknown)" \
          "$(c++ --version 2>/dev/null | sed -n '1p' || printf unknown)")" \
        --arg command "poc/build/$language/qwe-poc resize-smoke $resize_raw_file" \
        --arg started_at "$resize_started_at" \
        --argjson duration_ms "$resize_duration_ms" \
        --argjson metrics "$resize_metrics" \
        --argjson checks "$resize_checks" \
        --arg raw "poc/results/$platform/$language/$resize_raw_name" \
        --arg stdout "poc/results/$platform/$language/$resize_stdout_name" \
        --arg stderr "poc/results/$platform/$language/$resize_stderr_name" \
        --arg environment_record "poc/results/$platform/environment/$run_id.txt" \
        --arg platform_limitation "$platform_limitation" \
        --arg resize_limitation "$resize_limitation" \
        '{
          case_id: "T07",
          language: $language,
          status: $status,
          source_commit: $source_commit,
          environment: {
            os: $os,
            architecture: $architecture,
            toolchain: $toolchain,
            environment_record: $environment_record
          },
          command: $command,
          started_at: $started_at,
          duration_ms: $duration_ms,
          metrics: $metrics,
          checks: $checks,
          evidence: [$raw, $stdout, $stderr],
          known_limitations: [
            $platform_limitation,
            $resize_limitation,
            "Covers four dynamic PTY sizes; emulator and pane state are not yet covered"
          ]
        }' >"$resize_result_file"

      printf 'terminal smoke %-4s %s\n' "$language" "$case_status"
      printf 'editor smoke   %-4s %s\n' "$language" "$editor_status"
      printf 'resize smoke   %-4s %s\n' "$language" "$resize_status"
    done

    if [ "$failed" -ne 0 ]; then
      printf 'terminal smoke verification FAIL\n' >&2
      exit 1
    fi

    result_count=0
    while IFS= read -r result_file; do
      jq -e '
        (.case_id | type == "string" and
          test("^(T|P|D|RM|DX|XP|EX|MC)[0-9]{2}$")) and
        (.language == "rust" or .language == "go" or .language == "cpp") and
        (.status | IN("pass", "fail", "blocked", "unsupported", "not-run")) and
        (.source_commit | type == "string" and length > 0) and
        (.environment.os | type == "string") and
        (.environment.architecture | type == "string") and
        (.environment.toolchain | type == "string") and
        (.command | type == "string") and
        (.started_at | type == "string") and
        (.duration_ms | type == "number" and . >= 0) and
        (.metrics | type == "object") and
        ((.checks // {}) | type == "object" and
          all(.[]; type == "boolean")) and
        (.evidence | type == "array" and length > 0 and
          all(.[]; type == "string")) and
        (.known_limitations | type == "array" and
          all(.[]; type == "string"))
      ' "$result_file" >/dev/null
      result_count=$((result_count + 1))
    done < <(
      find "$results_root" -type f -name '*.json' ! -name '*.stdout.json' |
        grep -E '/[A-Z]+[0-9]{2}-[^/]*\.json$' |
        sort
    )

    if [ "$result_count" -eq 0 ]; then
      printf 'no structured results were generated\n' >&2
      exit 1
    fi
    printf 'result structure verification PASS (%s results)\n' "$result_count"
    printf 'terminal smoke verification PASS\n'

fmt:
    cargo fmt --manifest-path "{{ poc }}/rust/Cargo.toml" -- --check
    @test -z "$(gofmt -d {{ poc }}/go/cmd/qwe-poc/main.go)"

lint:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo clippy --manifest-path "{{ poc }}/rust/Cargo.toml" --release -- -D warnings
    (
      cd "{{ poc }}/go"
      go vet ./...
    )
    build_root="$(mktemp -d "${TMPDIR:-/tmp}/qwe-cpp-lint.XXXXXX")"
    cmake -S "{{ poc }}/cpp" -B "$build_root" -G Ninja \
      -DCMAKE_BUILD_TYPE=Release
    cmake --build "$build_root"

test-macos:
    #!/usr/bin/env bash
    set -euo pipefail
    [ "$(uname -s)" = "Darwin" ] || {
      printf 'macOS test suite must run on Darwin\n' >&2
      exit 2
    }
    just --justfile "{{ justfile() }}" doctor
    just --justfile "{{ justfile() }}" verify-skeleton
    just --justfile "{{ justfile() }}" verify-fixtures
    just --justfile "{{ justfile() }}" terminal-smoke-macos
    printf 'macOS POC tests completed\n'

test-linux:
    #!/usr/bin/env bash
    set -euo pipefail
    [ "$(uname -s)" = "Linux" ] || {
      printf 'Linux test suite must run on Linux\n' >&2
      exit 2
    }
    just --justfile "{{ justfile() }}" doctor
    just --justfile "{{ justfile() }}" verify-skeleton
    just --justfile "{{ justfile() }}" verify-fixtures
    just --justfile "{{ justfile() }}" terminal-smoke-linux
    printf 'Linux POC tests completed\n'
