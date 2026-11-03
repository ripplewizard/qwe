# QWE POC orchestration. Platform-specific recipes write isolated evidence.
set shell := ["bash", "-euo", "pipefail", "-c"]

poc := justfile_directory() / "poc"
linux_host := env_var_or_default("QWE_LINUX_HOST", "istoria")
linux_repo := env_var_or_default("QWE_LINUX_REPO", "project/uranus/bianca")

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
    printf 'gopath=%s\n' "$(if command -v go >/dev/null 2>&1; then go env GOPATH 2>/dev/null || printf unavailable; else printf missing; fi)"
    printf 'gocache=%s\n' "$(if command -v go >/dev/null 2>&1; then go env GOCACHE 2>/dev/null || printf unavailable; else printf missing; fi)"
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
      CARGO_HOME="$build_root/rust/cargo-home" \
      CARGO_TARGET_DIR="$build_root/rust/cargo" \
        cargo build --release --manifest-path "$root/rust/Cargo.toml"
      cp "$build_root/rust/cargo/release/qwe-poc" "$build_root/rust/qwe-poc"
    }

    build_go() {
      mkdir -p "$build_root/go/gopath" "$build_root/go/cache"
      (
        cd "$root/go"
        GOPATH="$build_root/go/gopath" \
        GOCACHE="$build_root/go/cache" \
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

runtime-smoke:
    #!/usr/bin/env bash
    set -euo pipefail
    case "$(uname -s)" in
      Darwin) platform="macos-$(uname -m)" ;;
      Linux) platform="linux-$(uname -m)" ;;
      *) printf 'unsupported platform: %s\n' "$(uname -s)" >&2; exit 2 ;;
    esac
    exec just --justfile "{{ justfile() }}" _runtime-smoke "$platform"

runtime-smoke-macos:
    #!/usr/bin/env bash
    set -euo pipefail
    [ "$(uname -s)" = "Darwin" ] || {
      printf 'macOS runtime smoke must run on Darwin\n' >&2
      exit 2
    }
    exec just --justfile "{{ justfile() }}" _runtime-smoke "macos-$(uname -m)"

runtime-smoke-linux:
    #!/usr/bin/env bash
    set -euo pipefail
    [ "$(uname -s)" = "Linux" ] || {
      printf 'Linux runtime smoke must run on Linux\n' >&2
      exit 2
    }
    exec just --justfile "{{ justfile() }}" _runtime-smoke "linux-$(uname -m)"

lease-smoke language="all":
    #!/usr/bin/env bash
    set -euo pipefail
    case "$(uname -s)" in
      Darwin) platform="macos-$(uname -m)" ;;
      Linux) platform="linux-$(uname -m)" ;;
      *) printf 'unsupported platform: %s\n' "$(uname -s)" >&2; exit 2 ;;
    esac
    case "{{ language }}" in
      rust|go|cpp)
        just --justfile "{{ justfile() }}" _lease-smoke "$platform" "{{ language }}"
        ;;
      all)
        for implementation in rust go cpp; do
          just --justfile "{{ justfile() }}" _lease-smoke "$platform" "$implementation"
        done
        ;;
      *)
        printf 'usage: just lease-smoke [all|rust|go|cpp]\n' >&2
        exit 2
        ;;
    esac

attach-smoke language="all":
    #!/usr/bin/env bash
    set -euo pipefail
    case "$(uname -s)" in
      Darwin) platform="macos-$(uname -m)" ;;
      Linux) platform="linux-$(uname -m)" ;;
      *) printf 'unsupported platform: %s\n' "$(uname -s)" >&2; exit 2 ;;
    esac
    case "{{ language }}" in
      rust|go|cpp)
        just --justfile "{{ justfile() }}" _attach-smoke "$platform" "{{ language }}"
        ;;
      all)
        for implementation in rust go cpp; do
          just --justfile "{{ justfile() }}" _attach-smoke "$platform" "$implementation"
        done
        ;;
      *)
        printf 'usage: just attach-smoke [all|rust|go|cpp]\n' >&2
        exit 2
        ;;
    esac

terminal-interaction-smoke language="all":
    #!/usr/bin/env bash
    set -euo pipefail
    case "$(uname -s)" in
      Darwin) platform="macos-$(uname -m)" ;;
      Linux) platform="linux-$(uname -m)" ;;
      *) printf 'unsupported platform: %s\n' "$(uname -s)" >&2; exit 2 ;;
    esac
    case "{{ language }}" in
      rust|go|cpp)
        just --justfile "{{ justfile() }}" _terminal-interaction-smoke "$platform" "{{ language }}"
        ;;
      all)
        for implementation in rust go cpp; do
          just --justfile "{{ justfile() }}" _terminal-interaction-smoke "$platform" "$implementation"
        done
        ;;
      *)
        printf 'usage: just terminal-interaction-smoke [all|rust|go|cpp]\n' >&2
        exit 2
        ;;
    esac

developer-experience-smoke language="all":
    #!/usr/bin/env bash
    set -euo pipefail
    case "$(uname -s)" in
      Darwin) platform="macos-$(uname -m)" ;;
      Linux) platform="linux-$(uname -m)" ;;
      *) printf 'unsupported platform: %s\n' "$(uname -s)" >&2; exit 2 ;;
    esac
    case "{{ language }}" in
      rust|go|cpp)
        just --justfile "{{ justfile() }}" _developer-experience-smoke "$platform" "{{ language }}"
        ;;
      all)
        for implementation in rust go cpp; do
          just --justfile "{{ justfile() }}" _developer-experience-smoke "$platform" "$implementation"
        done
        ;;
      *)
        printf 'usage: just developer-experience-smoke [all|rust|go|cpp]\n' >&2
        exit 2
        ;;
    esac

platform-comparison:
    python3 "{{ poc }}/test-runner/platform_comparison.py" \
      --results-root "{{ poc }}/results" \
      --matrix "{{ poc }}/test-spec/matrix.md" \
      --report "{{ poc }}/reports/platform-comparison.md"

review-results:
    python3 "{{ poc }}/test-runner/review_results.py"
    just --justfile "{{ justfile() }}" platform-comparison

verify-results:
    python3 "{{ poc }}/test-runner/verify_results.py" \
      --results-root "{{ poc }}/results" \
      --schema "{{ poc }}/test-spec/result.schema.json" \
      --matrix "{{ poc }}/test-spec/matrix.md"

timeout-smoke language="all":
    #!/usr/bin/env bash
    set -euo pipefail
    case "$(uname -s)" in
      Darwin) platform="macos-$(uname -m)" ;;
      Linux) platform="linux-$(uname -m)" ;;
      *) printf 'unsupported platform: %s\n' "$(uname -s)" >&2; exit 2 ;;
    esac
    case "{{ language }}" in
      rust|go|cpp)
        just --justfile "{{ justfile() }}" _timeout-smoke "$platform" "{{ language }}"
        ;;
      all)
        for implementation in rust go cpp; do
          just --justfile "{{ justfile() }}" _timeout-smoke "$platform" "$implementation"
        done
        ;;
      *)
        printf 'usage: just timeout-smoke [all|rust|go|cpp]\n' >&2
        exit 2
        ;;
    esac

restart-smoke language="all":
    #!/usr/bin/env bash
    set -euo pipefail
    case "$(uname -s)" in
      Darwin) platform="macos-$(uname -m)" ;;
      Linux) platform="linux-$(uname -m)" ;;
      *) printf 'unsupported platform: %s\n' "$(uname -s)" >&2; exit 2 ;;
    esac
    case "{{ language }}" in
      rust|go|cpp)
        just --justfile "{{ justfile() }}" _restart-smoke "$platform" "{{ language }}"
        ;;
      all)
        for implementation in rust go cpp; do
          just --justfile "{{ justfile() }}" _restart-smoke "$platform" "$implementation"
        done
        ;;
      *)
        printf 'usage: just restart-smoke [all|rust|go|cpp]\n' >&2
        exit 2
        ;;
    esac

upgrade-smoke language="all":
    #!/usr/bin/env bash
    set -euo pipefail
    case "$(uname -s)" in
      Darwin) platform="macos-$(uname -m)" ;;
      Linux) platform="linux-$(uname -m)" ;;
      *) printf 'unsupported platform: %s\n' "$(uname -s)" >&2; exit 2 ;;
    esac
    case "{{ language }}" in
      rust|go|cpp)
        just --justfile "{{ justfile() }}" _upgrade-smoke "$platform" "{{ language }}"
        ;;
      all)
        for implementation in rust go cpp; do
          just --justfile "{{ justfile() }}" _upgrade-smoke "$platform" "$implementation"
        done
        ;;
      *)
        printf 'usage: just upgrade-smoke [all|rust|go|cpp]\n' >&2
        exit 2
        ;;
    esac

process-acceptance-smoke language="all":
    #!/usr/bin/env bash
    set -euo pipefail
    case "$(uname -s)" in
      Darwin) platform="macos-$(uname -m)" ;;
      Linux) platform="linux-$(uname -m)" ;;
      *) printf 'unsupported platform: %s\n' "$(uname -s)" >&2; exit 2 ;;
    esac
    case "{{ language }}" in
      rust|go|cpp)
        just --justfile "{{ justfile() }}" _process-acceptance-smoke "$platform" "{{ language }}"
        ;;
      all)
        for implementation in rust go cpp; do
          just --justfile "{{ justfile() }}" _process-acceptance-smoke "$platform" "$implementation"
        done
        ;;
      *)
        printf 'usage: just process-acceptance-smoke [all|rust|go|cpp]\n' >&2
        exit 2
        ;;
    esac

resource-smoke language="all":
    #!/usr/bin/env bash
    set -euo pipefail
    case "$(uname -s)" in
      Darwin) platform="macos-$(uname -m)" ;;
      Linux) platform="linux-$(uname -m)" ;;
      *) printf 'unsupported platform: %s\n' "$(uname -s)" >&2; exit 2 ;;
    esac
    case "{{ language }}" in
      rust|go|cpp)
        just --justfile "{{ justfile() }}" _resource-smoke "$platform" "{{ language }}"
        ;;
      all)
        for implementation in rust go cpp; do
          just --justfile "{{ justfile() }}" _resource-smoke "$platform" "$implementation"
        done
        ;;
      *)
        printf 'usage: just resource-smoke [all|rust|go|cpp]\n' >&2
        exit 2
        ;;
    esac

daemon-depth-smoke language="all":
    #!/usr/bin/env bash
    set -euo pipefail
    case "$(uname -s)" in
      Darwin) platform="macos-$(uname -m)" ;;
      Linux) platform="linux-$(uname -m)" ;;
      *) printf 'unsupported platform: %s\n' "$(uname -s)" >&2; exit 2 ;;
    esac
    case "{{ language }}" in
      rust|go|cpp)
        just --justfile "{{ justfile() }}" _daemon-depth-smoke "$platform" "{{ language }}"
        ;;
      all)
        for implementation in rust go cpp; do
          just --justfile "{{ justfile() }}" _daemon-depth-smoke "$platform" "$implementation"
        done
        ;;
      *)
        printf 'usage: just daemon-depth-smoke [all|rust|go|cpp]\n' >&2
        exit 2
        ;;
    esac

sync-linux-results:
    rsync -az "{{ linux_host }}:{{ linux_repo }}/poc/results/linux-x86_64/" "{{ poc }}/results/linux-x86_64/"

test-linux-remote:
    ssh "{{ linux_host }}" "TERM=dumb zsh -lic 'cd {{ linux_repo }} && just test-linux'"
    just --justfile "{{ justfile() }}" sync-linux-results

check-qwe:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo test --workspace

build-qwe:
    cargo build --workspace

test-qwe:
    cargo test --workspace

t12-host-smoke:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo build -p qwe -p qwed -p qwe-host
    host_socket="/tmp/qwe-t12-host-$$.sock"
    client_socket="/tmp/qwe-t12-client-$$.sock"
    runtime_dir="${host_socket}.runtime"
    host_pid=""
    qwed_pid=""
    session_id="t12-session"
    cleanup_t12() {
      if [ -n "$qwed_pid" ]; then
        kill -TERM "$qwed_pid" 2>/dev/null || true
        wait "$qwed_pid" 2>/dev/null || true
      fi
      if [ -n "$host_pid" ]; then
        kill -TERM "$host_pid" 2>/dev/null || true
        wait "$host_pid" 2>/dev/null || true
      fi
      pkill -TERM -f -- "--worker-socket ${runtime_dir}/" 2>/dev/null || true
      rm -rf -- "$runtime_dir"
      rm -f "$host_socket" "$client_socket" "${client_socket}.sqlite" \
        "${client_socket}.sqlite-shm" "${client_socket}.sqlite-wal"
    }
    trap cleanup_t12 EXIT
    target/debug/qwe-host --socket "$host_socket" &
    host_pid=$!
    for _ in $(seq 1 100); do
      [ -S "$host_socket" ] && break
      sleep 0.02
    done
    python3 -c 'import os,sys; os.setsid(); os.execv(sys.argv[1],sys.argv[1:])' \
      target/debug/qwed --socket "$client_socket" --host-socket "$host_socket" &
    qwed_pid=$!
    for _ in $(seq 1 100); do
      target/debug/qwe --socket "$client_socket" ping >/dev/null 2>&1 && break
      sleep 0.02
    done
    target/debug/qwe --socket "$client_socket" ping >/dev/null
    second_qwed_log="/tmp/qwe-t12-second-qwed-$$.log"
    set +e
    python3 -c 'import os,sys; os.setsid(); os.execv(sys.argv[1],sys.argv[1:])' \
      target/debug/qwed --socket "$client_socket" --host-socket "$host_socket" \
      >"$second_qwed_log" 2>&1 &
    second_qwed_pid=$!
    wait "$second_qwed_pid"
    second_qwed_status=$?
    set -e
    [ "$second_qwed_status" -ne 0 ]
    grep -Fq 'qwed is already running' "$second_qwed_log"
    target/debug/qwe --socket "$client_socket" session create "$session_id" sh >/dev/null
    target/debug/qwe --socket "$client_socket" session input "$session_id" \
      $'printf "QWE_T12_BEFORE\\n"\r' >/dev/null
    before=""
    for _ in $(seq 1 100); do
      before="$(target/debug/qwe --socket "$client_socket" session attach "$session_id")"
      printf '%s' "$before" | jq -e '.screen | contains("QWE_T12_BEFORE")' >/dev/null &&
        break
      sleep 0.02
    done
    printf '%s' "$before" | jq -e '.screen | contains("QWE_T12_BEFORE")' >/dev/null
    session_pid="$(printf '%s' "$before" | jq -r '.pid')"
    host_pgid="$(ps -o pgid= -p "$host_pid" | tr -d ' ')"
    qwed_pgid="$(ps -o pgid= -p "$qwed_pid" | tr -d ' ')"
    [ "$host_pgid" != "$qwed_pgid" ]
    kill -KILL -- "-$qwed_pgid"
    wait "$qwed_pid" 2>/dev/null || true
    qwed_pid=""
    kill -0 "$host_pid"
    python3 -c 'import os,sys; os.setsid(); os.execv(sys.argv[1],sys.argv[1:])' \
      target/debug/qwed --socket "$client_socket" --host-socket "$host_socket" &
    qwed_pid=$!
    for _ in $(seq 1 100); do
      target/debug/qwe --socket "$client_socket" ping >/dev/null 2>&1 && break
      sleep 0.02
    done
    target/debug/qwe --socket "$client_socket" ping >/dev/null
    recovered="$(target/debug/qwe --socket "$client_socket" session attach "$session_id")"
    printf '%s' "$recovered" | jq -e \
      --argjson pid "$session_pid" \
      '.running == true and .pid == $pid and (.screen | contains("QWE_T12_BEFORE"))' \
      >/dev/null
    target/debug/qwe --socket "$client_socket" session input "$session_id" \
      $'printf "QWE_T12_AFTER\\n"\r' >/dev/null
    after=""
    for _ in $(seq 1 100); do
      after="$(target/debug/qwe --socket "$client_socket" session attach "$session_id")"
      printf '%s' "$after" | jq -e '.screen | contains("QWE_T12_AFTER")' >/dev/null &&
        break
      sleep 0.02
    done
    printf '%s' "$after" | jq -e '.screen | contains("QWE_T12_AFTER")' >/dev/null
    printf -v bounded_command 'exec python3 %q\r' \
      "$PWD/poc/test-data/scripts/bounded_output.py"
    target/debug/qwe --socket "$client_socket" session input "$session_id" \
      "$bounded_command" >/dev/null
    bounded_attach="/tmp/qwe-t12-bounded-$$.json"
    for _ in $(seq 1 1500); do
      target/debug/qwe --socket "$client_socket" session status "$session_id" \
        >"$bounded_attach"
      jq -e '.total_written >= 104857600' "$bounded_attach" >/dev/null && break
      sleep 0.02
    done
    jq -e '.total_written >= 104857600' "$bounded_attach" >/dev/null
    target/debug/qwe --socket "$client_socket" session attach "$session_id" \
      >"$bounded_attach"
    jq -e \
      '.snapshot_bytes == 1048576 and
       .total_written >= 104857600 and
       .truncated_from > 0' \
      "$bounded_attach" >/dev/null
    jq -r '.journal_hex' "$bounded_attach" | xxd -r -p |
      grep -Fq 'QWE_BOUNDED_END'
    host_rss="$(ps -o rss= -p "$host_pid" | tr -d ' ')"
    worker_pid="$(jq -r '.worker_pid' "$bounded_attach")"
    worker_rss="$(ps -o rss= -p "$worker_pid" | tr -d ' ')"
    total_rss="$((host_rss + worker_rss))"
    [ "$total_rss" -lt 204800 ]
    target/debug/qwe --socket "$client_socket" session close "$session_id" >/dev/null
    printf 'T12 host ownership PASS (bounded attach, total_rss=%s KiB)\n' "$total_rss"

resilience-smoke:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo build -p qwe -p qwed -p qwe-host
    host_socket="/tmp/qwe-resilience-host-$$.sock"
    client_socket="/tmp/qwe-resilience-client-$$.sock"
    store_path="/tmp/qwe-resilience-store-$$.sqlite"
    runtime_dir="${host_socket}.runtime"
    host_pid=""
    qwed_pid=""
    worker_pid=""
    lost_worker_pid=""
    session_id="resilience-session"
    lost_session_id="lost-session"
    second_qwed_log="/tmp/qwe-resilience-second-qwed-$$.log"

    cleanup_resilience() {
      if [ -n "$qwed_pid" ]; then
        kill -TERM -- "-${qwed_pid}" 2>/dev/null || true
        wait "$qwed_pid" 2>/dev/null || true
      fi
      if [ -n "$host_pid" ]; then
        kill -TERM -- "-${host_pid}" 2>/dev/null || true
        wait "$host_pid" 2>/dev/null || true
      fi
      for worker in "$worker_pid" "$lost_worker_pid"; do
        if [ -n "$worker" ]; then
          kill -TERM "$worker" 2>/dev/null || true
        fi
      done
      pkill -TERM -f -- "--worker-socket ${runtime_dir}/" 2>/dev/null || true
      rm -rf -- "$runtime_dir"
      rm -f "$host_socket" "$client_socket" "$second_qwed_log" \
        "$store_path" "${store_path}-shm" "${store_path}-wal"
    }
    trap cleanup_resilience EXIT

    start_host() {
      target/debug/qwe-host --socket "$host_socket" &
      host_pid=$!
      for _ in $(seq 1 200); do
        [ -S "$host_socket" ] && return
        sleep 0.02
      done
      printf 'host socket did not become ready\n' >&2
      return 1
    }

    start_qwed() {
      python3 -c 'import os,sys; os.setsid(); os.execv(sys.argv[1],sys.argv[1:])' \
        target/debug/qwed --socket "$client_socket" --host-socket "$host_socket" \
        --store "$store_path" &
      qwed_pid=$!
      for _ in $(seq 1 200); do
        target/debug/qwe --socket "$client_socket" ping >/dev/null 2>&1 && return
        sleep 0.02
      done
      printf 'qwed did not become ready\n' >&2
      return 1
    }

    start_host
    start_qwed
    created="$(target/debug/qwe --socket "$client_socket" task start "$session_id" sh)"
    task_id="$(printf '%s' "$created" | jq -r '.task_id')"
    worker_pid="$(printf '%s' "$created" | jq -r '.worker_pid')"
    session_pid="$(printf '%s' "$created" | jq -r '.pid')"
    start_token="$(printf '%s' "$created" | jq -r '.start_token')"
    [ -n "$task_id" ] && [ -n "$worker_pid" ] && [ -n "$start_token" ]
    worker_pgid="$(ps -o pgid= -p "$worker_pid" | tr -d ' ')"
    original_host_pgid="$(ps -o pgid= -p "$host_pid" | tr -d ' ')"
    [ "$worker_pgid" = "$worker_pid" ]
    [ "$worker_pgid" != "$original_host_pgid" ]
    target/debug/qwe --socket "$client_socket" session input "$session_id" \
      $'printf "QWE_RESILIENCE_BEFORE\\n"\r' >/dev/null
    for _ in $(seq 1 100); do
      recovered="$(target/debug/qwe --socket "$client_socket" session attach "$session_id")"
      printf '%s' "$recovered" | jq -e \
        '.screen | contains("QWE_RESILIENCE_BEFORE")' >/dev/null && break
      sleep 0.02
    done
    printf '%s' "$recovered" | jq -e \
      --argjson pid "$session_pid" \
      '.running == true and .pid == $pid' >/dev/null
    lease="$(target/debug/qwe --socket "$client_socket" lease claim \
      "$session_id" owner-a --ttl-ms 60000)"
    generation="$(printf '%s' "$lease" | jq -r '.generation')"
    [ "$generation" = 1 ]

    host_pgid="$(ps -o pgid= -p "$host_pid" | tr -d ' ')"
    kill -KILL -- "-$host_pgid"
    wait "$host_pid" 2>/dev/null || true
    host_pid=""
    kill -0 "$worker_pid"

    start_host
    recovered="$(target/debug/qwe --socket "$client_socket" session attach "$session_id")"
    printf '%s' "$recovered" | jq -e \
      --argjson pid "$session_pid" \
      --arg start "$start_token" \
      '.pid == $pid and .start_token == $start and .running == true and
       (.screen | contains("QWE_RESILIENCE_BEFORE"))' >/dev/null
    qwed_pgid="$(ps -o pgid= -p "$qwed_pid" | tr -d ' ')"
    kill -KILL -- "-$qwed_pgid"
    wait "$qwed_pid" 2>/dev/null || true
    qwed_pid=""
    start_qwed
    control="$(target/debug/qwe --socket "$client_socket" status)"
    printf '%s' "$control" | jq -e \
      --arg id "$session_id" \
      '.daemon == "ok" and
       .reconciliation.live_sessions >= 1 and
       ([.sessions[] | select(.id == $id and .state == "running")] | length == 1) and
       ([.tasks[] | select(.session_id == $id and .state == "running")] | length == 1)' \
      >/dev/null
    target/debug/qwe --socket "$client_socket" session input "$session_id" \
      $'printf "QWE_RESILIENCE_AFTER\\n"\r' >/dev/null
    attached_after=""
    for _ in $(seq 1 100); do
      attached_after="$(target/debug/qwe --socket "$client_socket" session attach "$session_id")"
      printf '%s' "$attached_after" | jq -e \
        '.screen | contains("QWE_RESILIENCE_AFTER")' >/dev/null && break
      sleep 0.02
    done
    printf '%s' "$attached_after" | jq -e \
      '.screen | contains("QWE_RESILIENCE_AFTER")' >/dev/null
    target/debug/qwe --socket "$client_socket" lease touch \
      "$session_id" owner-a 1 --ttl-ms 60000 |
      jq -e '.generation == 1 and .owner == "owner-a"' >/dev/null

    printf -v continuous_command 'exec python3 %q\r' \
      "$PWD/poc/test-data/scripts/continuous_output.py"
    target/debug/qwe --socket "$client_socket" session input "$session_id" \
      "$continuous_command" >/dev/null
    for _ in $(seq 1 100); do
      output_status="$(target/debug/qwe --socket "$client_socket" session status "$session_id")"
      printf '%s' "$output_status" | jq -e '.total_written > 0' >/dev/null && break
      sleep 0.02
    done
    benchmark="$(python3 poc/test-data/scripts/attach_benchmark.py \
      --cli "$PWD/target/debug/qwe" \
      --socket "$client_socket" \
      --session-id "$session_id" \
      --clients 8 \
      --iterations 8)"
    printf '%s\n' "$benchmark" | jq -e \
      '.successes == .requests and .p95_ms <= .max_p95_ms and .p99_ms <= .max_p99_ms' \
      >/dev/null
    target/debug/qwe --socket "$client_socket" session input "$session_id" $'\003' >/dev/null
    for _ in $(seq 1 100); do
      output_status="$(target/debug/qwe --socket "$client_socket" session status "$session_id")"
      printf '%s' "$output_status" | jq -e '.running == false' >/dev/null && break
      sleep 0.02
    done
    printf '%s' "$output_status" | jq -e '.running == false and .exit_code == 0' >/dev/null

    stopped="$(target/debug/qwe --socket "$client_socket" task stop "$task_id")"
    printf '%s' "$stopped" | jq -e '.state == "stopped"' >/dev/null
    target/debug/qwe --socket "$client_socket" task history |
      jq -e --arg id "$task_id" \
        '([.tasks[] | select(.task_id == $id and .state == "stopped")] | length == 1)' \
        >/dev/null

    lost_created="$(target/debug/qwe --socket "$client_socket" task start "$lost_session_id" sh)"
    lost_task_id="$(printf '%s' "$lost_created" | jq -r '.task_id')"
    lost_worker_pid="$(printf '%s' "$lost_created" | jq -r '.worker_pid')"
    qwed_pgid="$(ps -o pgid= -p "$qwed_pid" | tr -d ' ')"
    kill -KILL -- "-$qwed_pgid"
    wait "$qwed_pid" 2>/dev/null || true
    qwed_pid=""
    kill -KILL "$lost_worker_pid"
    lost_worker_pid=""
    start_qwed
    control="$(target/debug/qwe --socket "$client_socket" status)"
    printf '%s' "$control" | jq -e \
      --arg id "$lost_session_id" \
      '.reconciliation.lost_sessions >= 1 and
       ([.sessions[] | select(.id == $id and .state == "lost")] | length == 1)' \
      >/dev/null
    target/debug/qwe --socket "$client_socket" task history |
      jq -e --arg id "$lost_task_id" \
        '([.tasks[] | select(.task_id == $id and .state == "lost")] | length == 1)' \
        >/dev/null

    set +e
    python3 -c 'import os,sys; os.setsid(); os.execv(sys.argv[1],sys.argv[1:])' \
      target/debug/qwed --socket "$client_socket" --host-socket "$host_socket" \
      --store "$store_path" >"$second_qwed_log" 2>&1 &
    second_qwed_pid=$!
    wait "$second_qwed_pid"
    second_qwed_status=$?
    set -e
    [ "$second_qwed_status" -ne 0 ]
    grep -Fq 'qwed is already running' "$second_qwed_log"
    printf '%s\n' "$benchmark"
    printf 'Resilience PASS (host recovery, SQLite lease/task, reconciliation, concurrent attach)\n'

_terminal-smoke platform:
    #!/usr/bin/env bash
    set -euo pipefail
    root="{{ poc }}"
    repo_root="$(CDPATH= cd -- "$root/.." && pwd)"
    platform="{{ platform }}"
    stamp="$(date -u +%Y%m%dT%H%M%SZ)"
    run_id="$stamp-$$"
    if git -C "$repo_root" rev-parse --verify HEAD >/dev/null 2>&1; then
      source_commit="$(git -C "$repo_root" rev-parse --short HEAD)"
      if ! git -C "$repo_root" diff --quiet HEAD --; then
        source_commit="$source_commit-dirty"
      fi
    else
      source_commit="uncommitted"
    fi
    os_name="$(uname -s)"
    architecture="$(uname -m)"
    started_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    toolchain_value="$(printf 'rust=%s; go=%s; cxx=%s' \
      "$(rustc --version 2>/dev/null || printf unknown)" \
      "$(go version 2>/dev/null || printf unknown)" \
      "$(c++ --version 2>/dev/null | sed -n '1p' || printf unknown)")"
    failed=0
    results_root="$root/results/$platform"

    case "$platform" in
      macos-*)
        platform_limitation="macOS preflight run; not an official Linux x86_64 comparison score"
        resize_limitation="PTY resize sends an explicit SIGWINCH to the foreground process group"
        ;;
      linux-*)
        platform_limitation="Linux x86_64 target; verify source cleanliness before final scoring"
        resize_limitation="PTY resize sends an explicit SIGWINCH to the foreground process group"
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
         .checks.bash == true and
         .checks.zsh == true and
         .checks.window_size == true and
         .checks.fd_no_leak == true and
         .checks.process_reaped == true' >/dev/null; then
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
        if [ "$case_id" = "T01" ]; then
          case_checks="$(printf '%s' "$summary" | jq -c \
            '.checks | {shell, fd_no_leak, process_reaped}')"
        else
          case_checks="$(printf '%s' "$summary" | jq -c \
            '.checks | {bash, zsh, window_size, process_reaped}')"
        fi
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
          --argjson checks "$case_checks" \
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
        "$root/test-data/scripts/resize_watcher.py" \
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
         .checks.emulator == true and
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

      long_raw_name="T10-$run_id.pty.log"
      long_stdout_name="T10-$run_id.stdout.json"
      long_stderr_name="T10-$run_id.stderr.log"
      long_raw_file="$result_dir/$long_raw_name"
      long_stdout_file="$result_dir/$long_stdout_name"
      long_stderr_file="$result_dir/$long_stderr_name"
      long_started_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
      long_start_ms="$(python3 -c 'import time; print(time.time_ns() // 1_000_000)')"
      set +e
      "$root/build/$language/qwe-poc" long-output-smoke "$long_raw_file" \
        "$root/test-data/scripts/long_output.sh" \
        >"$long_stdout_file" 2>"$long_stderr_file"
      long_binary_status=$?
      set -e
      long_end_ms="$(python3 -c 'import time; print(time.time_ns() // 1_000_000)')"
      long_duration_ms=$((long_end_ms - long_start_ms))
      long_summary="$(cat "$long_stdout_file")"
      long_summary_valid=false
      if printf '%s' "$long_summary" | jq -e \
        --arg language "$language" \
        '.feature == "long-output" and
         .implementation == $language and
         .exit_code == 0 and
         .lines == 50000 and
         .checks.count == true and
         .checks.first == true and
         .checks.last == true' >/dev/null; then
        long_summary_valid=true
      fi
      long_lines="$(grep -c 'QWE-LINE-' "$long_raw_file" 2>/dev/null || true)"
      long_markers_valid=false
      if [ "$long_lines" -eq 50000 ] &&
         grep -Fq 'QWE-LINE-00050000' "$long_raw_file" &&
         grep -Fq 'QWE-LINE-00000001' "$long_raw_file"; then
        long_markers_valid=true
      fi
      long_status=fail
      if [ "$long_binary_status" -eq 0 ] &&
         [ "$long_summary_valid" = true ] &&
         [ "$long_markers_valid" = true ]; then
        long_status=pass
      else
        failed=1
      fi
      if [ -n "$long_summary" ] &&
         printf '%s' "$long_summary" | jq -e . >/dev/null 2>&1; then
        long_metrics="$(printf '%s' "$long_summary" | jq -c \
          '{bytes, lines, elapsed_ms}')"
        long_checks="$(printf '%s' "$long_summary" | jq -c '.checks')"
      else
        long_metrics='{}'
        long_checks='{}'
      fi
      long_result_file="$result_dir/T10-$source_commit-$run_id.json"
      jq -n \
        --arg language "$language" \
        --arg status "$long_status" \
        --arg source_commit "$source_commit" \
        --arg os "$os_name" \
        --arg architecture "$architecture" \
        --arg toolchain "$toolchain_value" \
        --arg command "poc/build/$language/qwe-poc long-output-smoke $long_raw_file" \
        --arg started_at "$long_started_at" \
        --argjson duration_ms "$long_duration_ms" \
        --argjson metrics "$long_metrics" \
        --argjson checks "$long_checks" \
        --arg raw "poc/results/$platform/$language/$long_raw_name" \
        --arg stdout "poc/results/$platform/$language/$long_stdout_name" \
        --arg stderr "poc/results/$platform/$language/$long_stderr_name" \
        --arg environment_record "poc/results/$platform/environment/$run_id.txt" \
        --arg platform_limitation "$platform_limitation" \
        '{
          case_id: "T10",
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
            "Single-run throughput is not yet a stable performance benchmark"
          ]
        }' >"$long_result_file"

      vt_raw_name="T05-T06-$run_id.pty.log"
      vt_stdout_name="T05-T06-$run_id.stdout.json"
      vt_stderr_name="T05-T06-$run_id.stderr.log"
      vt_raw_file="$result_dir/$vt_raw_name"
      vt_stdout_file="$result_dir/$vt_stdout_name"
      vt_stderr_file="$result_dir/$vt_stderr_name"
      vt_started_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
      vt_start_ms="$(python3 -c 'import time; print(time.time_ns() // 1_000_000)')"
      set +e
      "$root/build/$language/qwe-poc" vt-smoke "$vt_raw_file" \
        "$root/test-data/scripts/vt_fixture.py" \
        >"$vt_stdout_file" 2>"$vt_stderr_file"
      vt_binary_status=$?
      set -e
      vt_end_ms="$(python3 -c 'import time; print(time.time_ns() // 1_000_000)')"
      vt_duration_ms=$((vt_end_ms - vt_start_ms))
      vt_summary="$(cat "$vt_stdout_file")"

      t05_status=fail
      if [ "$vt_binary_status" -eq 0 ] &&
         printf '%s' "$vt_summary" | jq -e \
           --arg language "$language" \
           '.feature == "vt-state" and
            .implementation == $language and
            .exit_code == 0 and
            .checks.base_text == true and
            .checks.colors == true and
            .checks.erase == true and
            .checks.cursor == true and
            .checks.title == true and
            .checks.scroll_region == true and
            .checks.alternate_screen == true' >/dev/null; then
        t05_status=pass
      else
        failed=1
      fi
      t06_status=fail
      if [ "$vt_binary_status" -eq 0 ] &&
         printf '%s' "$vt_summary" | jq -e \
           --arg language "$language" \
           '.feature == "vt-state" and
            .implementation == $language and
            .exit_code == 0 and
            .checks.unicode == true and
            .checks.wide == true' >/dev/null; then
        t06_status=pass
      else
        failed=1
      fi
      if [ -n "$vt_summary" ] &&
         printf '%s' "$vt_summary" | jq -e . >/dev/null 2>&1; then
        vt_metrics="$(printf '%s' "$vt_summary" | jq -c \
          --argjson bytes "$(wc -c <"$vt_raw_file" | tr -d ' ')" \
          '{bytes:$bytes}')"
      else
        vt_metrics='{}'
      fi
      for vt_case in T05 T06; do
        vt_result_file="$result_dir/$vt_case-$source_commit-$run_id.json"
        if [ "$vt_case" = T05 ]; then
          vt_status="$t05_status"
          vt_checks="$(printf '%s' "$vt_summary" | jq -c \
            '.checks | {base_text, colors, erase, cursor, title, scroll_region, alternate_screen}' 2>/dev/null ||
            printf '{}')"
          vt_limitation="SGR color, erase, cursor, title, scroll region, and alternate screen covered; manual visual comparison remains"
        else
          vt_status="$t06_status"
          vt_checks="$(printf '%s' "$vt_summary" | jq -c \
            '.checks | {unicode, wide}' 2>/dev/null || printf '{}')"
          vt_limitation="CJK, emoji, and one combining sequence covered; broader grapheme shaping remains manual"
        fi
        jq -n \
          --arg case_id "$vt_case" \
          --arg language "$language" \
          --arg status "$vt_status" \
          --arg source_commit "$source_commit" \
          --arg os "$os_name" \
          --arg architecture "$architecture" \
          --arg toolchain "$toolchain_value" \
          --arg command "poc/build/$language/qwe-poc vt-smoke $vt_raw_file" \
          --arg started_at "$vt_started_at" \
          --argjson duration_ms "$vt_duration_ms" \
          --argjson metrics "$vt_metrics" \
          --argjson checks "$vt_checks" \
          --arg raw "poc/results/$platform/$language/$vt_raw_name" \
          --arg stdout "poc/results/$platform/$language/$vt_stdout_name" \
          --arg stderr "poc/results/$platform/$language/$vt_stderr_name" \
          --arg environment_record "poc/results/$platform/environment/$run_id.txt" \
          --arg platform_limitation "$platform_limitation" \
          --arg limitation "$vt_limitation" \
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
            known_limitations: [$platform_limitation, $limitation]
          }' >"$vt_result_file"
      done

      printf 'terminal smoke %-4s %s\n' "$language" "$case_status"
      printf 'editor smoke   %-4s %s\n' "$language" "$editor_status"
      printf 'resize smoke   %-4s %s\n' "$language" "$resize_status"
      printf 'long output    %-4s %s\n' "$language" "$long_status"
      printf 'VT state       %-4s T05=%s T06=%s\n' \
        "$language" "$t05_status" "$t06_status"
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
      find "$results_root" -type f -name "*-$run_id.json" \
        ! -name '*.stdout.json' |
        sort
    )

    if [ "$result_count" -eq 0 ]; then
      printf 'no structured results were generated\n' >&2
      exit 1
    fi
    printf 'result structure verification PASS (%s results)\n' "$result_count"
    printf 'terminal smoke verification PASS\n'

_lease-smoke platform language:
    #!/usr/bin/env bash
    set -euo pipefail
    root="{{ poc }}"
    platform="{{ platform }}"
    language="{{ language }}"
    just --justfile "{{ justfile() }}" build "$language"
    python3 "$root/test-runner/lease_smoke.py" \
      --binary "$root/build/$language/qwe-poc" \
      --scenarios "$root/test-spec/lease-scenarios.json" \
      --language "$language" \
      --platform "$platform" \
      --results-dir "$root/results/$platform/$language"

_attach-smoke platform language:
    #!/usr/bin/env bash
    set -euo pipefail
    root="{{ poc }}"
    platform="{{ platform }}"
    language="{{ language }}"
    just --justfile "{{ justfile() }}" build "$language"
    python3 "$root/test-runner/attach_smoke.py" \
      --binary "$root/build/$language/qwe-poc" \
      --language "$language" \
      --platform "$platform" \
      --results-dir "$root/results/$platform/$language"

_terminal-interaction-smoke platform language:
    #!/usr/bin/env bash
    set -euo pipefail
    root="{{ poc }}"
    platform="{{ platform }}"
    language="{{ language }}"
    just --justfile "{{ justfile() }}" build "$language"
    python3 "$root/test-runner/terminal_interaction_smoke.py" \
      --binary "$root/build/$language/qwe-poc" \
      --language "$language" \
      --platform "$platform" \
      --results-dir "$root/results/$platform/$language"

_developer-experience-smoke platform language:
    #!/usr/bin/env bash
    set -euo pipefail
    root="{{ poc }}"
    platform="{{ platform }}"
    language="{{ language }}"
    just --justfile "{{ justfile() }}" build "$language"
    python3 "$root/test-runner/developer_experience_smoke.py" \
      --binary "$root/build/$language/qwe-poc" \
      --language "$language" \
      --platform "$platform" \
      --results-dir "$root/results/$platform/$language"

_timeout-smoke platform language:
    #!/usr/bin/env bash
    set -euo pipefail
    root="{{ poc }}"
    platform="{{ platform }}"
    language="{{ language }}"
    just --justfile "{{ justfile() }}" build "$language"
    python3 "$root/test-runner/timeout_smoke.py" \
      --binary "$root/build/$language/qwe-poc" \
      --language "$language" \
      --platform "$platform" \
      --results-dir "$root/results/$platform/$language"

_restart-smoke platform language:
    #!/usr/bin/env bash
    set -euo pipefail
    root="{{ poc }}"
    platform="{{ platform }}"
    language="{{ language }}"
    just --justfile "{{ justfile() }}" build "$language"
    python3 "$root/test-runner/restart_smoke.py" \
      --binary "$root/build/$language/qwe-poc" \
      --language "$language" \
      --platform "$platform" \
      --results-dir "$root/results/$platform/$language"

_upgrade-smoke platform language:
    #!/usr/bin/env bash
    set -euo pipefail
    root="{{ poc }}"
    platform="{{ platform }}"
    language="{{ language }}"
    just --justfile "{{ justfile() }}" build "$language"
    python3 "$root/test-runner/upgrade_smoke.py" \
      --binary "$root/build/$language/qwe-poc" \
      --language "$language" \
      --platform "$platform" \
      --results-dir "$root/results/$platform/$language"

_process-acceptance-smoke platform language:
    #!/usr/bin/env bash
    set -euo pipefail
    root="{{ poc }}"
    platform="{{ platform }}"
    language="{{ language }}"
    just --justfile "{{ justfile() }}" build "$language"
    python3 "$root/test-runner/process_acceptance_smoke.py" \
      --binary "$root/build/$language/qwe-poc" \
      --language "$language" \
      --platform "$platform" \
      --results-dir "$root/results/$platform/$language"

_resource-smoke platform language:
    #!/usr/bin/env bash
    set -euo pipefail
    root="{{ poc }}"
    platform="{{ platform }}"
    language="{{ language }}"
    just --justfile "{{ justfile() }}" build "$language"
    python3 "$root/test-runner/resource_smoke.py" \
      --binary "$root/build/$language/qwe-poc" \
      --language "$language" \
      --platform "$platform" \
      --results-dir "$root/results/$platform/$language"

_daemon-depth-smoke platform language:
    #!/usr/bin/env bash
    set -euo pipefail
    root="{{ poc }}"
    platform="{{ platform }}"
    language="{{ language }}"
    just --justfile "{{ justfile() }}" build "$language"
    python3 "$root/test-runner/daemon_depth_smoke.py" \
      --binary "$root/build/$language/qwe-poc" \
      --language "$language" \
      --platform "$platform" \
      --results-dir "$root/results/$platform/$language"

_runtime-smoke platform:
    #!/usr/bin/env bash
    set -euo pipefail
    root="{{ poc }}"
    repo_root="$(CDPATH= cd -- "$root/.." && pwd)"
    platform="{{ platform }}"
    stamp="$(date -u +%Y%m%dT%H%M%SZ)"
    run_id="$stamp-$$"
    if git -C "$repo_root" rev-parse --verify HEAD >/dev/null 2>&1; then
      source_commit="$(git -C "$repo_root" rev-parse --short HEAD)"
      if ! git -C "$repo_root" diff --quiet HEAD --; then
        source_commit="$source_commit-dirty"
      fi
    else
      source_commit="uncommitted"
    fi
    os_name="$(uname -s)"
    architecture="$(uname -m)"
    results_root="$root/results/$platform"
    environment_dir="$results_root/environment"
    mkdir -p "$environment_dir"
    environment_file="$environment_dir/runtime-$run_id.txt"
    just --justfile "{{ justfile() }}" doctor >"$environment_file"
    just --justfile "{{ justfile() }}" build all

    case "$platform" in
      macos-*)
        platform_limitation="macOS preflight run; not an official Linux x86_64 comparison score"
        identity_limitation="macOS process start token currently has one-second resolution"
        ;;
      linux-*)
        platform_limitation="Linux x86_64 target; verify source cleanliness before final scoring"
        identity_limitation="Linux process start token uses /proc start time"
        ;;
      *)
        printf 'unsupported result platform: %s\n' "$platform" >&2
        exit 2
        ;;
    esac

    toolchain="$(printf 'rust=%s; go=%s; cxx=%s' \
      "$(rustc --version 2>/dev/null || printf unknown)" \
      "$(go version 2>/dev/null || printf unknown)" \
      "$(c++ --version 2>/dev/null | sed -n '1p' || printf unknown)")"

    write_runtime_result() {
      local case_id="$1"
      local status="$2"
      local checks="$3"
      local result_file="$result_dir/$case_id-$source_commit-$run_id.json"
      jq -n \
        --arg case_id "$case_id" \
        --arg language "$language" \
        --arg status "$status" \
        --arg source_commit "$source_commit" \
        --arg os "$os_name" \
        --arg architecture "$architecture" \
        --arg toolchain "$toolchain" \
        --arg command "runtime contract: daemon -> task-start/status/stop -> SIGKILL -> restart" \
        --arg started_at "$started_at" \
        --argjson duration_ms "$duration_ms" \
        --argjson metrics "$metrics" \
        --argjson checks "$checks" \
        --arg environment_record "poc/results/$platform/environment/runtime-$run_id.txt" \
        --arg platform_limitation "$platform_limitation" \
        --arg identity_limitation "$identity_limitation" \
        --arg initial "$evidence_initial" \
        --arg duplicate "$evidence_duplicate" \
        --arg before "$evidence_before" \
        --arg after "$evidence_after" \
        --arg stop "$evidence_stop" \
        --arg task_log "$evidence_task_log" \
        --arg daemon_log "$evidence_daemon_log" \
        --arg database "$evidence_database" \
        --arg concurrency "$evidence_concurrency" \
        --arg disconnect "$evidence_disconnect" \
        --arg corrupt_log "$evidence_corrupt_log" \
        --arg corrupt_db "$evidence_corrupt_db" \
        --arg persistence_log "$evidence_persistence_log" \
        --arg edge_log "$evidence_edge_log" \
        --arg long_run_log "$evidence_long_run_log" \
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
          evidence: [
            $initial, $duplicate, $before, $after, $stop,
            $task_log, $daemon_log, $database,
            $concurrency, $disconnect, $corrupt_log, $corrupt_db,
            $persistence_log, $edge_log, $long_run_log
          ],
          known_limitations: [
            $platform_limitation,
            $identity_limitation,
            "Async task exit codes are not yet persisted",
            "SQLite lock contention, disk-full, and upgrade compatibility are not covered"
          ]
        }' >"$result_file"
    }

    failed=0
    for language in rust go cpp; do
      result_dir="$results_root/$language"
      runtime_dir="$result_dir/runtime-$run_id"
      mkdir -p "$runtime_dir"
      socket="/tmp/qwe-runtime-$language-$$.sock"
      database="$runtime_dir/tasks.sqlite"
      task_log="$runtime_dir/task.log"
      daemon_log="$runtime_dir/daemon.log"
      binary="$root/build/$language/qwe-poc"
      daemon_pid=""
      task_pgid=""
      started_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
      start_ms="$(python3 -c 'import time; print(time.time_ns() // 1_000_000)')"

      cleanup_runtime() {
        if [ -n "$task_pgid" ]; then
          kill -TERM -- "-$task_pgid" 2>/dev/null || true
        fi
        if [ -n "$daemon_pid" ]; then
          kill -TERM "$daemon_pid" 2>/dev/null || true
          wait "$daemon_pid" 2>/dev/null || true
        fi
        rm -f "$socket"
      }
      trap cleanup_runtime EXIT

      wait_ready() {
        for _ in $(seq 1 100); do
          if [ -n "$("$binary" task-status "$socket" __ready__ 2>/dev/null || true)" ]; then
            return 0
          fi
          sleep 0.05
        done
        return 1
      }

      QWE_PROCESS_TEST_ENV=expected "$binary" daemon "$socket" "$database" \
        >"$daemon_log" 2>&1 &
      daemon_pid=$!
      wait_ready
      "$binary" task-start \
        "$socket" recovery-key "$task_log" \
        "$root/test-data/scripts/spawn_tree.sh" 3 300 \
        >"$runtime_dir/start-initial.jsonl"
      initial="$(cat "$runtime_dir/start-initial.jsonl")"
      task_id="$(printf '%s' "$initial" | jq -r '.task_id')"
      task_pid="$(printf '%s' "$initial" | jq -r '.pid')"
      task_pgid="$(printf '%s' "$initial" | jq -r '.pgid')"
      initial_pgid="$task_pgid"
      start_token="$(printf '%s' "$initial" | jq -r '.start_token')"

      "$binary" task-start \
        "$socket" recovery-key "$task_log" \
        "$root/test-data/scripts/spawn_tree.sh" 3 300 \
        >"$runtime_dir/start-duplicate.jsonl"
      duplicate="$(cat "$runtime_dir/start-duplicate.jsonl")"

      "$binary" task-status "$socket" "$task_id" \
        >"$runtime_dir/status-before.jsonl"
      before="$(cat "$runtime_dir/status-before.jsonl")"

      for _ in $(seq 1 100); do
        [ "$(grep -c '^QWE-PROCESS' "$task_log" 2>/dev/null || true)" -ge 3 ] && break
        sleep 0.02
      done
      process_tree_lines="$(
        grep -c '^QWE-PROCESS' "$task_log" 2>/dev/null || true
      )"
      [ "$process_tree_lines" -ge 3 ]

      kill -KILL "$daemon_pid"
      set +e
      wait "$daemon_pid" 2>/dev/null
      set -e
      daemon_pid=""
      alive_after_crash=false
      if kill -0 -- "-$task_pgid" 2>/dev/null; then
        alive_after_crash=true
      fi

      QWE_PROCESS_TEST_ENV=expected "$binary" daemon "$socket" "$database" \
        >>"$daemon_log" 2>&1 &
      daemon_pid=$!
      wait_ready
      recovered=false
      if "$binary" task-status "$socket" "$task_id" \
        >"$runtime_dir/status-after.jsonl" &&
         jq -e '.ok == true and .state == "running"' \
           "$runtime_dir/status-after.jsonl" >/dev/null; then
        recovered=true
      fi
      after="$(cat "$runtime_dir/status-after.jsonl")"
      identity_preserved=false
      if printf '%s' "$after" | jq -e \
        --arg task_id "$task_id" \
        --argjson pid "$task_pid" \
        --argjson pgid "$task_pgid" \
        --arg token "$start_token" \
        '.ok == true and
         .state == "running" and
         .task_id == $task_id and
         .pid == $pid and
         .pgid == $pgid and
         .start_token == $token' >/dev/null; then
        identity_preserved=true
      fi

      concurrency_dir="$runtime_dir/concurrency"
      mkdir -p "$concurrency_dir"
      client_pids=()
      for request_index in $(seq 1 20); do
        "$binary" task-status "$socket" "$task_id" \
          >"$concurrency_dir/status-$request_index.jsonl" 2>&1 &
        client_pids+=("$!")
        "$binary" task-start \
          "$socket" recovery-key "$task_log" \
          "$root/test-data/scripts/spawn_tree.sh" 3 300 \
          >"$concurrency_dir/start-$request_index.jsonl" 2>&1 &
        client_pids+=("$!")
      done
      concurrent_clients_ok=true
      for client_pid in "${client_pids[@]}"; do
        if ! wait "$client_pid"; then
          concurrent_clients_ok=false
        fi
      done
      concurrent_payload_ok=true
      for response_file in "$concurrency_dir"/*.jsonl; do
        if ! jq -e \
          --arg task_id "$task_id" \
          --argjson pid "$task_pid" \
          '.ok == true and .task_id == $task_id and .pid == $pid' \
          "$response_file" >/dev/null; then
          concurrent_payload_ok=false
        fi
      done
      concurrent_ok=false
      if [ "$concurrent_clients_ok" = true ] &&
         [ "$concurrent_payload_ok" = true ]; then
        concurrent_ok=true
      fi

      long_run_dir="$runtime_dir/long-run"
      mkdir -p "$long_run_dir"
      rss_before="$(ps -o rss= -p "$daemon_pid" | tr -d ' ')"
      long_run_requests_ok=true
      for request_index in $(seq 1 100); do
        "$binary" task-status "$socket" "$task_id" \
          >"$long_run_dir/status-$request_index.jsonl" 2>&1 || {
          long_run_requests_ok=false
          break
        }
        jq -e '.ok == true and .state == "running"' \
          "$long_run_dir/status-$request_index.jsonl" >/dev/null || {
          long_run_requests_ok=false
          break
        }
        sleep 0.02
      done
      rss_after="$(ps -o rss= -p "$daemon_pid" | tr -d ' ')"
      long_run_daemon_alive=false
      kill -0 "$daemon_pid" 2>/dev/null && long_run_daemon_alive=true
      long_run_ok=false
      if [ "$long_run_requests_ok" = true ] &&
         [ "$long_run_daemon_alive" = true ]; then
        long_run_ok=true
      fi

      disconnect_sent=false
      if python3 -c 'import socket,sys; s=socket.socket(socket.AF_UNIX,socket.SOCK_STREAM); s.connect(sys.argv[1]); s.sendall(b"STATUS\tpartial"); s.close()' "$socket"; then
        disconnect_sent=true
      fi
      disconnect_after_ok=false
      "$binary" task-status "$socket" "$task_id" \
        >"$runtime_dir/client-disconnect.log" 2>&1 || true
      if jq -e '.ok == true and .state == "running"' \
          "$runtime_dir/client-disconnect.log" >/dev/null; then
        disconnect_after_ok=true
      fi
      disconnect_ok=false
      if [ "$disconnect_sent" = true ] &&
         [ "$disconnect_after_ok" = true ]; then
        disconnect_ok=true
      fi

      "$binary" task-stop "$socket" "$task_id" \
        >"$runtime_dir/stop.jsonl"
      stop="$(cat "$runtime_dir/stop.jsonl")"
      group_stopped=false
      for _ in $(seq 1 100); do
        if ! kill -0 -- "-$task_pgid" 2>/dev/null; then
          group_stopped=true
          break
        fi
        sleep 0.02
      done
      [ "$group_stopped" = true ]
      task_pgid=""

      edge_dir="$runtime_dir/edge"
      mkdir -p "$edge_dir"

      "$binary" task-start \
        "$socket" p01-key "$edge_dir/p01.log" \
        sh -c 'printf "QWE_SINGLE_OK\n"; exit 0' \
        >"$edge_dir/p01-start.jsonl"
      p01_id="$(jq -r '.task_id' "$edge_dir/p01-start.jsonl")"
      p01_state=""
      for _ in $(seq 1 100); do
        "$binary" task-status "$socket" "$p01_id" \
          >"$edge_dir/p01-status.jsonl" 2>&1 || true
        p01_state="$(jq -r '.state // empty' "$edge_dir/p01-status.jsonl")"
        [ "$p01_state" = "exited" ] && break
        sleep 0.02
      done
      p01_ok=false
      if [ "$p01_state" = "exited" ] &&
         grep -Fq 'QWE_SINGLE_OK' "$edge_dir/p01.log"; then
        p01_ok=true
      fi

      "$binary" task-start \
        "$socket" p05-key "$edge_dir/p05.log" \
        "$root/test-data/scripts/child_crash.sh" fail \
        >"$edge_dir/p05-start.jsonl"
      p05_id="$(jq -r '.task_id' "$edge_dir/p05-start.jsonl")"
      p05_state=""
      for _ in $(seq 1 100); do
        "$binary" task-status "$socket" "$p05_id" \
          >"$edge_dir/p05-status.jsonl" 2>&1 || true
        p05_state="$(jq -r '.state // empty' "$edge_dir/p05-status.jsonl")"
        [ "$p05_state" = "failed" ] && break
        sleep 0.02
      done
      p05_ok=false
      if [ "$p05_state" = "failed" ] &&
         grep -Fq 'QWE-CHILD-Crash' "$edge_dir/p05.log"; then
        p05_ok=true
      fi

      "$binary" task-start \
        "$socket" p06-key "$edge_dir/p06.log" \
        sh -c 'sleep 30 & echo "QWE_CHILD_PID=$!"; exit 0' \
        >"$edge_dir/p06-start.jsonl"
      p06_id="$(jq -r '.task_id' "$edge_dir/p06-start.jsonl")"
      p06_pgid="$(jq -r '.pgid' "$edge_dir/p06-start.jsonl")"
      p06_state=""
      for _ in $(seq 1 100); do
        "$binary" task-status "$socket" "$p06_id" \
          >"$edge_dir/p06-status.jsonl" 2>&1 || true
        p06_state="$(jq -r '.state // empty' "$edge_dir/p06-status.jsonl")"
        [ "$p06_state" = "exited" ] && break
        sleep 0.02
      done
      p06_child_pid="$(
        sed -n 's/^QWE_CHILD_PID=//p' "$edge_dir/p06.log" | tail -n 1
      )"
      p06_child_alive=false
      if [ -n "$p06_child_pid" ] &&
         kill -0 "$p06_child_pid" 2>/dev/null; then
        p06_child_alive=true
      fi
      p06_ok=false
      if [ "$p06_state" = "exited" ] &&
         [ "$p06_child_alive" = true ]; then
        p06_ok=true
      fi
      kill -TERM -- "-$p06_pgid" 2>/dev/null || true
      for _ in $(seq 1 100); do
        kill -0 -- "-$p06_pgid" 2>/dev/null || break
        sleep 0.02
      done
      kill -KILL -- "-$p06_pgid" 2>/dev/null || true

      "$binary" task-start \
        "$socket" p11-key "$edge_dir/p11.log" \
        sh -c 'printf "QWE_CWD=%s\n" "$(pwd)"; printf "QWE_ENV=%s\n" "$QWE_PROCESS_TEST_ENV"' \
        >"$edge_dir/p11-start.jsonl"
      p11_id="$(jq -r '.task_id' "$edge_dir/p11-start.jsonl")"
      p11_state=""
      for _ in $(seq 1 100); do
        "$binary" task-status "$socket" "$p11_id" \
          >"$edge_dir/p11-status.jsonl" 2>&1 || true
        p11_state="$(jq -r '.state // empty' "$edge_dir/p11-status.jsonl")"
        [ "$p11_state" = "exited" ] && break
        sleep 0.02
      done
      p11_ok=false
      if [ "$p11_state" = "exited" ] &&
         grep -Fq "QWE_CWD=$repo_root" "$edge_dir/p11.log" &&
         grep -Fq 'QWE_ENV=expected' "$edge_dir/p11.log"; then
        p11_ok=true
      fi

      sqlite3 "$database" \
        "CREATE TRIGGER reject_insert BEFORE INSERT ON tasks BEGIN SELECT RAISE(ABORT, 'injected persistence failure'); END;"
      "$binary" task-start \
        "$socket" persistence-failure-key "$runtime_dir/persistence-failure-task.log" \
        "$root/test-data/scripts/spawn_tree.sh" 2 300 \
        >"$runtime_dir/sqlite-write-failure.jsonl" 2>&1 || true
      sqlite3 "$database" "DROP TRIGGER reject_insert;"
      persistence_error_explicit=false
      if jq -e '.ok == false and (.message | contains("injected persistence failure"))' \
          "$runtime_dir/sqlite-write-failure.jsonl" >/dev/null; then
        persistence_error_explicit=true
      fi
      persistence_recovered=false
      "$binary" task-status "$socket" "$task_id" \
        >"$runtime_dir/sqlite-write-recovery.jsonl" 2>&1 || true
      if jq -e '.ok == true and .state == "stopped"' \
          "$runtime_dir/sqlite-write-recovery.jsonl" >/dev/null; then
        persistence_recovered=true
      fi

      idempotent=false
      if printf '%s' "$duplicate" | jq -e \
        --arg task_id "$task_id" \
        --argjson pid "$task_pid" \
        '.ok == true and .created == false and
         .task_id == $task_id and .pid == $pid' >/dev/null; then
        idempotent=true
      fi
      ipc_ok=false
      if printf '%s' "$initial" | jq -e '.ok == true and .created == true' \
          >/dev/null &&
         printf '%s' "$before" | jq -e '.ok == true and .state == "running"' \
          >/dev/null &&
         printf '%s' "$stop" | jq -e '.ok == true and .state == "stopped"' \
          >/dev/null; then
        ipc_ok=true
      fi
      db_state="$(
        sqlite3 "$database" "select state from tasks where id='$task_id';"
      )"
      task_records="$(
        sqlite3 "$database" "select count(*) from tasks where idempotency_key='recovery-key';"
      )"
      history_ok=false
      if [ "$db_state" = "stopped" ] && [ "$task_records" -eq 1 ]; then
        history_ok=true
      fi

      corrupt_socket="/tmp/qwe-corrupt-$language-$$.sock"
      corrupt_database="$runtime_dir/corrupt.sqlite"
      corrupt_log="$runtime_dir/corrupt-daemon.log"
      printf 'not a sqlite database\n' >"$corrupt_database"
      "$binary" daemon "$corrupt_socket" "$corrupt_database" \
        >"$corrupt_log" 2>&1 &
      corrupt_pid=$!
      corrupt_rejected=false
      for _ in $(seq 1 100); do
        if ! kill -0 "$corrupt_pid" 2>/dev/null; then
          corrupt_rejected=true
          break
        fi
        sleep 0.02
      done
      if [ "$corrupt_rejected" != true ]; then
        kill -TERM "$corrupt_pid" 2>/dev/null || true
      fi
      set +e
      wait "$corrupt_pid" 2>/dev/null
      corrupt_exit=$?
      set -e
      rm -f "$corrupt_socket"
      if [ "$corrupt_exit" -eq 0 ] || [ ! -s "$corrupt_log" ]; then
        corrupt_rejected=false
      fi

      duration_ms=$((
        $(python3 -c 'import time; print(time.time_ns() // 1_000_000)') -
        start_ms
      ))
      metrics="$(
        jq -nc \
          --argjson pid "$task_pid" \
          --argjson pgid "$initial_pgid" \
          --argjson depth "$process_tree_lines" \
          --argjson records "$task_records" \
          --argjson rss_before "$rss_before" \
          --argjson rss_after "$rss_after" \
          '{pid: $pid, pgid: $pgid, process_tree_lines: $depth,
            task_records: $records, concurrent_requests: 40,
            daemon_rss_kib_before: $rss_before, daemon_rss_kib_after: $rss_after}'
      )"
      evidence_initial="poc/results/$platform/$language/runtime-$run_id/start-initial.jsonl"
      evidence_duplicate="poc/results/$platform/$language/runtime-$run_id/start-duplicate.jsonl"
      evidence_before="poc/results/$platform/$language/runtime-$run_id/status-before.jsonl"
      evidence_after="poc/results/$platform/$language/runtime-$run_id/status-after.jsonl"
      evidence_stop="poc/results/$platform/$language/runtime-$run_id/stop.jsonl"
      evidence_task_log="poc/results/$platform/$language/runtime-$run_id/task.log"
      evidence_daemon_log="poc/results/$platform/$language/runtime-$run_id/daemon.log"
      evidence_database="poc/results/$platform/$language/runtime-$run_id/tasks.sqlite"
      evidence_concurrency="poc/results/$platform/$language/runtime-$run_id/concurrency"
      evidence_disconnect="poc/results/$platform/$language/runtime-$run_id/client-disconnect.log"
      evidence_corrupt_log="poc/results/$platform/$language/runtime-$run_id/corrupt-daemon.log"
      evidence_corrupt_db="poc/results/$platform/$language/runtime-$run_id/corrupt.sqlite"
      evidence_persistence_log="poc/results/$platform/$language/runtime-$run_id/sqlite-write-failure.jsonl"
      evidence_edge_log="poc/results/$platform/$language/runtime-$run_id/edge"
      evidence_long_run_log="poc/results/$platform/$language/runtime-$run_id/long-run"

      checks="$(jq -nc --argjson value "$ipc_ok" '{request_responses:$value}')"
      status=pass; [ "$ipc_ok" = true ] || { status=fail; failed=1; }
      write_runtime_result D01 "$status" "$checks"

      checks="$(jq -nc --argjson value "$idempotent" '{idempotent_start:$value}')"
      status=pass; [ "$idempotent" = true ] || { status=fail; failed=1; }
      write_runtime_result D07 "$status" "$checks"

      tree_ok=false
      [ "$process_tree_lines" -ge 3 ] && tree_ok=true
      checks="$(jq -nc --argjson value "$tree_ok" '{process_tree:$value}')"
      status=pass; [ "$tree_ok" = true ] || { status=fail; failed=1; }
      write_runtime_result P02 "$status" "$checks"

      checks="$(jq -nc \
        --argjson crash "$alive_after_crash" \
        --argjson recovery "$recovered" \
        '{task_survived_daemon_crash:$crash, daemon_restarted:$recovery}')"
      status=pass
      { [ "$alive_after_crash" = true ] && [ "$recovered" = true ]; } ||
        { status=fail; failed=1; }
      write_runtime_result P08 "$status" "$checks"

      checks="$(jq -nc \
        --argjson identity "$identity_preserved" \
        '{pid_and_start_token_preserved:$identity}')"
      status=pass; [ "$identity_preserved" = true ] || { status=fail; failed=1; }
      write_runtime_result P09 "$status" "$checks"

      signal_ok=false
      if printf '%s' "$stop" | jq -e '.ok == true and .state == "stopped"' \
          >/dev/null && [ "$group_stopped" = true ]; then
        signal_ok=true
      fi
      checks="$(jq -nc \
        --argjson stop "$signal_ok" \
        --argjson group "$group_stopped" \
        '{termination_request:$stop, process_group_stopped:$group}')"
      status=pass; [ "$signal_ok" = true ] || { status=fail; failed=1; }
      write_runtime_result P03 "$status" "$checks"

      checks="$(jq -nc \
        --argjson recovered "$recovered" \
        --argjson identity "$identity_preserved" \
        '{restart_ready:$recovered, identity_preserved:$identity}')"
      status=pass
      { [ "$recovered" = true ] && [ "$identity_preserved" = true ]; } ||
        { status=fail; failed=1; }
      write_runtime_result D03 "$status" "$checks"

      checks="$(jq -nc \
        --argjson history "$history_ok" \
        '{sqlite_history:$history}')"
      status=pass; [ "$history_ok" = true ] || { status=fail; failed=1; }
      write_runtime_result P12 "$status" "$checks"

      checks="$(jq -nc \
        --argjson alive "$long_run_daemon_alive" \
        --argjson stable "$long_run_requests_ok" \
        '{daemon_alive:$alive, requests_stable:$stable}')"
      status=pass; [ "$long_run_ok" = true ] || { status=fail; failed=1; }
      write_runtime_result D08 "$status" "$checks"

      checks="$(jq -nc --argjson value "$p01_ok" '{single_process_exit:$value}')"
      status=pass; [ "$p01_ok" = true ] || { status=fail; failed=1; }
      write_runtime_result P01 "$status" "$checks"

      checks="$(jq -nc --argjson value "$p05_ok" '{child_crash_detected:$value}')"
      status=pass; [ "$p05_ok" = true ] || { status=fail; failed=1; }
      write_runtime_result P05 "$status" "$checks"

      checks="$(jq -nc --argjson value "$p06_ok" \
        '{parent_exit_and_child_observed:$value}')"
      status=pass; [ "$p06_ok" = true ] || { status=fail; failed=1; }
      write_runtime_result P06 "$status" "$checks"

      checks="$(jq -nc --argjson value "$p11_ok" '{cwd_and_env_inherited:$value}')"
      status=pass; [ "$p11_ok" = true ] || { status=fail; failed=1; }
      write_runtime_result P11 "$status" "$checks"

      checks="$(jq -nc \
        --argjson clients "$concurrent_clients_ok" \
        --argjson state "$concurrent_payload_ok" \
        '{concurrent_clients:$clients, state_consistent:$state}')"
      status=pass; [ "$concurrent_ok" = true ] || { status=fail; failed=1; }
      write_runtime_result D02 "$status" "$checks"

      checks="$(jq -nc \
        --argjson sent "$disconnect_sent" \
        --argjson alive "$disconnect_after_ok" \
        '{partial_request_closed:$sent, daemon_still_serving:$alive}')"
      status=pass; [ "$disconnect_ok" = true ] || { status=fail; failed=1; }
      write_runtime_result D05 "$status" "$checks"

      checks="$(jq -nc \
        --argjson rejected "$corrupt_rejected" \
        --argjson write_error "$persistence_error_explicit" \
        --argjson recovered "$persistence_recovered" \
        '{corrupt_database_rejected:$rejected,
          write_error_explicit:$write_error,
          daemon_recovered_after_write_error:$recovered}')"
      status=pass
      { [ "$corrupt_rejected" = true ] &&
        [ "$persistence_error_explicit" = true ] &&
        [ "$persistence_recovered" = true ]; } ||
        { status=fail; failed=1; }
      write_runtime_result D04 "$status" "$checks"

      cleanup_runtime
      trap - EXIT
      printf 'runtime %-8s %s\n' "$language" "$status"
    done

    if [ "$failed" -ne 0 ]; then
      printf 'runtime verification FAIL\n' >&2
      exit 1
    fi

    runtime_result_count=0
    for language in rust go cpp; do
      for case_id in P01 P02 P03 P05 P06 P08 P09 P11 P12 D01 D02 D03 D04 D05 D07 D08; do
        result_file="$results_root/$language/$case_id-$source_commit-$run_id.json"
        jq -e \
          --arg case_id "$case_id" \
          --arg language "$language" \
          '.case_id == $case_id and
           .language == $language and
           (.status | IN("pass", "fail", "blocked", "unsupported", "not-run")) and
           ((.checks // {}) | type == "object" and all(.[]; type == "boolean")) and
           (.evidence | type == "array" and length > 0)' \
          "$result_file" >/dev/null
        while IFS= read -r evidence_path; do
          [ -e "$evidence_path" ]
        done < <(jq -r '.evidence[]' "$result_file")
        runtime_result_count=$((runtime_result_count + 1))
      done
    done
    printf 'runtime result verification PASS (%s results)\n' "$runtime_result_count"
    printf 'runtime verification PASS\n'

fmt:
    CARGO_HOME="{{ poc }}/build/rust/cargo-home" \
      cargo fmt --manifest-path "{{ poc }}/rust/Cargo.toml" -- --check
    @test -z "$(gofmt -d {{ poc }}/go/cmd/qwe-poc)"

lint:
    #!/usr/bin/env bash
    set -euo pipefail
    CARGO_HOME="{{ poc }}/build/rust/cargo-home" \
      cargo clippy --manifest-path "{{ poc }}/rust/Cargo.toml" \
      --release -- -D warnings
    (
      cd "{{ poc }}/go"
      GOPATH="{{ poc }}/build/go/gopath" \
      GOCACHE="{{ poc }}/build/go/cache" \
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
    just --justfile "{{ justfile() }}" runtime-smoke-macos
    just --justfile "{{ justfile() }}" lease-smoke all
    just --justfile "{{ justfile() }}" attach-smoke all
    just --justfile "{{ justfile() }}" terminal-interaction-smoke all
    just --justfile "{{ justfile() }}" timeout-smoke all
    just --justfile "{{ justfile() }}" restart-smoke all
    just --justfile "{{ justfile() }}" upgrade-smoke all
    just --justfile "{{ justfile() }}" process-acceptance-smoke all
    just --justfile "{{ justfile() }}" resource-smoke all
    just --justfile "{{ justfile() }}" daemon-depth-smoke all
    just --justfile "{{ justfile() }}" developer-experience-smoke all
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
    just --justfile "{{ justfile() }}" runtime-smoke-linux
    just --justfile "{{ justfile() }}" lease-smoke all
    just --justfile "{{ justfile() }}" attach-smoke all
    just --justfile "{{ justfile() }}" terminal-interaction-smoke all
    just --justfile "{{ justfile() }}" timeout-smoke all
    just --justfile "{{ justfile() }}" restart-smoke all
    just --justfile "{{ justfile() }}" upgrade-smoke all
    just --justfile "{{ justfile() }}" process-acceptance-smoke all
    just --justfile "{{ justfile() }}" resource-smoke all
    just --justfile "{{ justfile() }}" daemon-depth-smoke all
    just --justfile "{{ justfile() }}" developer-experience-smoke all
    printf 'Linux POC tests completed\n'
