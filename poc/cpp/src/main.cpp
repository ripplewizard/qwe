#include <cerrno>
#include <chrono>
#include <cstdlib>
#include <cstring>
#include <fcntl.h>
#include <fstream>
#include <iostream>
#include <poll.h>
#include <signal.h>
#include <sstream>
#include <string>
#include <string_view>
#include <sys/ioctl.h>
#include <sys/wait.h>
#include <termios.h>
#include <unistd.h>
#include <vector>
#include <vterm.h>

#include "runtime.hpp"

namespace {

struct PtyRunResult {
    int exit_code = 1;
    std::string output;
};

bool run_pty_session(std::string_view command, const std::string& log_path,
                     PtyRunResult& result) {
    const int master = posix_openpt(O_RDWR | O_NOCTTY);
    if (master < 0) {
        std::cerr << "posix_openpt failed: " << std::strerror(errno) << '\n';
        return false;
    }

    if (grantpt(master) != 0 || unlockpt(master) != 0) {
        std::cerr << "PTY grant/unlock failed: " << std::strerror(errno) << '\n';
        close(master);
        return false;
    }

    const char* slave_name = ptsname(master);
    if (slave_name == nullptr) {
        std::cerr << "ptsname failed: " << std::strerror(errno) << '\n';
        close(master);
        return false;
    }

    const int slave = open(slave_name, O_RDWR | O_NOCTTY);
    if (slave < 0) {
        std::cerr << "open PTY slave failed: " << std::strerror(errno) << '\n';
        close(master);
        return false;
    }

    winsize size{};
    size.ws_row = 24;
    size.ws_col = 80;
    if (ioctl(master, TIOCSWINSZ, &size) != 0) {
        std::cerr << "TIOCSWINSZ failed: " << std::strerror(errno) << '\n';
        close(slave);
        close(master);
        return false;
    }

    const pid_t child = fork();
    if (child < 0) {
        std::cerr << "fork failed: " << std::strerror(errno) << '\n';
        close(slave);
        close(master);
        return false;
    }

    if (child == 0) {
        close(master);
        if (setsid() < 0 || ioctl(slave, TIOCSCTTY, 0) != 0) {
            _exit(126);
        }
        if (dup2(slave, STDIN_FILENO) < 0 ||
            dup2(slave, STDOUT_FILENO) < 0 ||
            dup2(slave, STDERR_FILENO) < 0) {
            _exit(126);
        }
        if (slave > STDERR_FILENO) {
            close(slave);
        }
        setenv("TERM", "xterm-256color", 1);
        setenv("NVIM_TEST_MARKER", "QWE_NVIM_OK", 1);
        execlp("sh", "sh", static_cast<char*>(nullptr));
        _exit(127);
    }

    close(slave);
    ssize_t written = write(master, command.data(), command.size());
    if (written < 0 || static_cast<size_t>(written) != command.size()) {
        std::cerr << "PTY write failed: " << std::strerror(errno) << '\n';
        close(master);
        return false;
    }

    char buffer[4096];
    while (true) {
        const ssize_t count = read(master, buffer, sizeof(buffer));
        if (count > 0) {
            result.output.append(buffer, static_cast<size_t>(count));
            continue;
        }
        if (count == 0 || errno == EIO) {
            break;
        }
        std::cerr << "PTY read failed: " << std::strerror(errno) << '\n';
        close(master);
        return false;
    }
    close(master);

    int status = 0;
    if (waitpid(child, &status, 0) < 0) {
        std::cerr << "waitpid failed: " << std::strerror(errno) << '\n';
        return false;
    }

    if (!log_path.empty()) {
        std::ofstream log(log_path, std::ios::binary);
        log.write(result.output.data(),
                  static_cast<std::streamsize>(result.output.size()));
        if (!log) {
            std::cerr << "write PTY log failed\n";
            return false;
        }
    }

    result.exit_code = WIFEXITED(status) ? WEXITSTATUS(status) : 1;
    return true;
}

bool wait_readable(int descriptor) {
    pollfd event{};
    event.fd = descriptor;
    event.events = POLLIN;
    int result;
    do {
        result = poll(&event, 1, 10000);
    } while (result < 0 && errno == EINTR);
    return result > 0;
}

bool read_until_marker(int descriptor, std::string& output,
                       const std::string& marker) {
    while (output.find(marker) == std::string::npos) {
        if (!wait_readable(descriptor)) {
            return false;
        }
        char buffer[4096];
        const ssize_t count = read(descriptor, buffer, sizeof(buffer));
        if (count > 0) {
            output.append(buffer, static_cast<size_t>(count));
            continue;
        }
        if (count == 0 || errno == EIO) {
            return false;
        }
        std::cerr << "PTY read failed: " << std::strerror(errno) << '\n';
        return false;
    }
    return true;
}

bool read_until_markers(int descriptor, std::string& output,
                        const std::vector<std::string>& markers) {
    while (true) {
        bool complete = true;
        for (const auto& marker : markers) {
            if (output.find(marker) == std::string::npos) {
                complete = false;
                break;
            }
        }
        if (complete) {
            return true;
        }
        if (!wait_readable(descriptor)) {
            return false;
        }
        char buffer[4096];
        const ssize_t count = read(descriptor, buffer, sizeof(buffer));
        if (count > 0) {
            output.append(buffer, static_cast<size_t>(count));
            continue;
        }
        if (count == 0 || errno == EIO) {
            return false;
        }
        std::cerr << "PTY read failed: " << std::strerror(errno) << '\n';
        return false;
    }
}

bool read_to_eof(int descriptor, std::string& output) {
    while (true) {
        if (!wait_readable(descriptor)) {
            return false;
        }
        char buffer[4096];
        const ssize_t count = read(descriptor, buffer, sizeof(buffer));
        if (count > 0) {
            output.append(buffer, static_cast<size_t>(count));
            continue;
        }
        if (count == 0 || errno == EIO) {
            return true;
        }
        std::cerr << "PTY read failed: " << std::strerror(errno) << '\n';
        return false;
    }
}

int run_pty_smoke(const std::string& log_path) {
    constexpr std::string_view command =
        "printf 'QWE_PTY_READY\\n'; printf 'QWE_PTY_SIZE '; stty size; "
        "printf 'QWE_PTY_END\\n'; exit 0\r";
    PtyRunResult result;
    if (!run_pty_session(command, log_path, result)) {
        return 1;
    }

    const std::string& output = result.output;
    const bool shell_check = output.find("QWE_PTY_READY") != std::string::npos &&
                             output.find("QWE_PTY_END") != std::string::npos;
    const bool size_check =
        output.find("QWE_PTY_SIZE 24 80") != std::string::npos;
    const bool passed = result.exit_code == 0 && shell_check && size_check;

    std::cout << R"({"contract":"qwe-poc/0.1","feature":"pty-smoke","implementation":"cpp","exit_code":)"
              << result.exit_code << R"(,"bytes":)" << output.size()
              << R"(,"checks":{"shell":)" << (shell_check ? "true" : "false")
              << R"(,"window_size":)" << (size_check ? "true" : "false")
              << "}}\n";
    return passed ? 0 : 1;
}

int run_editor_smoke(const std::string& log_path) {
    constexpr std::string_view command =
        "printf 'QWE_EDITOR_SIZE '; stty size; "
        "nvim --clean -u NONE -c 'call setline(1, $NVIM_TEST_MARKER)' "
        "-c 'redraw' -c 'sleep 200m' -c 'qa!'; exit $?\r";
    PtyRunResult result;
    if (!run_pty_session(command, log_path, result)) {
        return 1;
    }

    const bool process_check = result.exit_code == 0;
    const bool screen_check =
        result.output.find("QWE_NVIM_OK") != std::string::npos;
    const bool size_check =
        result.output.find("QWE_EDITOR_SIZE 24 80") != std::string::npos;
    const bool passed = process_check && screen_check && size_check;

    std::cout
        << R"({"contract":"qwe-poc/0.1","feature":"editor-smoke","implementation":"cpp","exit_code":)"
        << result.exit_code << R"(,"bytes":)" << result.output.size()
        << R"(,"checks":{"process":)" << (process_check ? "true" : "false")
        << R"(,"screen_text":)" << (screen_check ? "true" : "false")
        << R"(,"window_size":)" << (size_check ? "true" : "false")
        << "}}\n";
    return passed ? 0 : 1;
}

int run_long_output_smoke(const std::string& log_path,
                          const std::string& script_path) {
    char* resolved_script = realpath(script_path.c_str(), nullptr);
    if (resolved_script == nullptr) {
        std::cerr << "resolve long output script failed: "
                  << std::strerror(errno) << '\n';
        return 1;
    }
    const std::string command =
        "exec '" + std::string(resolved_script) + "' 50000\r";
    free(resolved_script);
    const auto started = std::chrono::steady_clock::now();
    PtyRunResult result;
    if (!run_pty_session(command, log_path, result)) {
        return 1;
    }
    const auto elapsed_ms =
        std::chrono::duration_cast<std::chrono::milliseconds>(
            std::chrono::steady_clock::now() - started)
            .count();

    constexpr std::string_view marker = "QWE-LINE-";
    size_t count = 0;
    size_t position = 0;
    while ((position = result.output.find(marker, position)) !=
           std::string::npos) {
        ++count;
        position += marker.size();
    }
    const bool count_ok = count == 50000;
    const bool first_ok =
        result.output.find("QWE-LINE-00050000") != std::string::npos;
    const bool last_ok =
        result.output.find("QWE-LINE-00000001") != std::string::npos;
    const bool passed =
        result.exit_code == 0 && count_ok && first_ok && last_ok;
    std::cout
        << R"({"contract":"qwe-poc/0.1","feature":"long-output","implementation":"cpp","exit_code":)"
        << result.exit_code << R"(,"bytes":)" << result.output.size()
        << R"(,"lines":)" << count << R"(,"elapsed_ms":)" << elapsed_ms
        << R"(,"checks":{"count":)" << (count_ok ? "true" : "false")
        << R"(,"first":)" << (first_ok ? "true" : "false")
        << R"(,"last":)" << (last_ok ? "true" : "false") << "}}\n";
    return passed ? 0 : 1;
}

int run_vt_smoke(const std::string& log_path,
                 const std::string& fixture_path) {
    char* resolved_fixture = realpath(fixture_path.c_str(), nullptr);
    if (resolved_fixture == nullptr) {
        std::cerr << "resolve VT fixture failed: " << std::strerror(errno)
                  << '\n';
        return 1;
    }
    const std::string command =
        "exec python3 '" + std::string(resolved_fixture) + "'\r";
    free(resolved_fixture);
    PtyRunResult result;
    if (!run_pty_session(command, log_path, result)) {
        return 1;
    }

    VTerm* terminal = vterm_new(24, 80);
    vterm_set_utf8(terminal, 1);
    VTermScreen* screen = vterm_obtain_screen(terminal);
    vterm_screen_reset(screen, 1);
    (void)vterm_input_write(terminal, result.output.data(), result.output.size());

    std::vector<char> text(24 * 80 * 8 + 1);
    const VTermRect full_rect{0, 24, 0, 80};
    const size_t text_size =
        vterm_screen_get_text(screen, text.data(), text.size() - 1, full_rect);
    text[text_size] = '\0';
    const std::string screen_text(text.data(), text_size);

    VTermScreenCell red_cell{};
    VTermScreenCell blue_cell{};
    VTermScreenCell erase_cell{};
    VTermScreenCell wide_cell{};
    (void)vterm_screen_get_cell(screen, VTermPos{1, 0}, &red_cell);
    (void)vterm_screen_get_cell(screen, VTermPos{1, 4}, &blue_cell);
    (void)vterm_screen_get_cell(screen, VTermPos{2, 1}, &erase_cell);
    (void)vterm_screen_get_cell(screen, VTermPos{3, 0}, &wide_cell);

    VTermPos cursor{};
    vterm_state_get_cursorpos(vterm_obtain_state(terminal), &cursor);
    const bool base_text = screen_text.find("QWE_BASE") != std::string::npos;
    const bool colors =
        VTERM_COLOR_IS_INDEXED(&red_cell.fg) && red_cell.fg.indexed.idx == 1 &&
        VTERM_COLOR_IS_INDEXED(&blue_cell.fg) &&
        blue_cell.fg.indexed.idx == 4;
    const bool erase =
        erase_cell.chars[0] == 0 || erase_cell.chars[0] == ' ';
    const bool cursor_ok = cursor.row == 5 && cursor.col == 10;
    const bool unicode =
        screen_text.find("中文") != std::string::npos &&
        screen_text.find("😀") != std::string::npos &&
        screen_text.find("é") != std::string::npos;
    const bool wide = wide_cell.chars[0] == 0x4e2d && wide_cell.width == 2;
    const bool t05 =
        result.exit_code == 0 && base_text && colors && erase && cursor_ok;
    const bool t06 = result.exit_code == 0 && unicode && wide;
    vterm_free(terminal);

    std::cout
        << R"({"contract":"qwe-poc/0.1","feature":"vt-state","implementation":"cpp","exit_code":)"
        << result.exit_code << R"(,"checks":{"base_text":)"
        << (base_text ? "true" : "false") << R"(,"colors":)"
        << (colors ? "true" : "false") << R"(,"erase":)"
        << (erase ? "true" : "false") << R"(,"cursor":)"
        << (cursor_ok ? "true" : "false") << R"(,"unicode":)"
        << (unicode ? "true" : "false") << R"(,"wide":)"
        << (wide ? "true" : "false") << "}}\n";
    return t05 && t06 ? 0 : 1;
}

int run_resize_smoke(const std::string& log_path,
                     const std::string& watcher_path) {
    char* resolved_watcher = realpath(watcher_path.c_str(), nullptr);
    if (resolved_watcher == nullptr) {
        std::cerr << "resolve resize watcher failed: " << std::strerror(errno)
                  << '\n';
        return 1;
    }
    const std::string absolute_watcher(resolved_watcher);
    free(resolved_watcher);
    const int master = posix_openpt(O_RDWR | O_NOCTTY);
    if (master < 0 || grantpt(master) != 0 || unlockpt(master) != 0) {
        std::cerr << "PTY create failed: " << std::strerror(errno) << '\n';
        if (master >= 0) {
            close(master);
        }
        return 1;
    }

    const char* slave_name = ptsname(master);
    const int slave =
        slave_name == nullptr ? -1 : open(slave_name, O_RDWR | O_NOCTTY);
    if (slave < 0) {
        std::cerr << "PTY slave open failed: " << std::strerror(errno) << '\n';
        close(master);
        return 1;
    }

    winsize initial_size{};
    initial_size.ws_row = 24;
    initial_size.ws_col = 80;
    if (ioctl(master, TIOCSWINSZ, &initial_size) != 0) {
        close(slave);
        close(master);
        return 1;
    }

    const pid_t child = fork();
    if (child < 0) {
        close(slave);
        close(master);
        return 1;
    }

    if (child == 0) {
        close(master);
        if (setsid() < 0 || ioctl(slave, TIOCSCTTY, 0) != 0 ||
            dup2(slave, STDIN_FILENO) < 0 ||
            dup2(slave, STDOUT_FILENO) < 0 ||
            dup2(slave, STDERR_FILENO) < 0) {
            _exit(126);
        }
        if (slave > STDERR_FILENO) {
            close(slave);
        }
        setenv("TERM", "xterm-256color", 1);
        setenv("QWE_WATCHER_PATH", absolute_watcher.c_str(), 1);
        execlp("sh", "sh", static_cast<char*>(nullptr));
        _exit(127);
    }

    close(slave);
    constexpr std::string_view watcher_command =
        "exec python3 \"$QWE_WATCHER_PATH\"\r";
    if (write(master, watcher_command.data(), watcher_command.size()) < 0) {
        close(master);
        return 1;
    }
    std::string output;
    const bool ready =
        read_until_marker(master, output, "QWE_RESIZE_READY 24 80");
    const struct {
        int cols;
        int rows;
    } sizes[] = {{120, 40}, {80, 24}, {160, 50}, {40, 15}};
    bool size_checks = true;
    bool winch_checks = true;
    std::string sizes_json;

    for (const auto& target : sizes) {
        winsize size{};
        size.ws_row = static_cast<unsigned short>(target.rows);
        size.ws_col = static_cast<unsigned short>(target.cols);
        if (ioctl(master, TIOCSWINSZ, &size) != 0) {
            close(master);
            return 1;
        }

        constexpr std::string_view check = "check\r";
        if (write(master, check.data(), check.size()) < 0) {
            close(master);
            return 1;
        }
        usleep(10000);
        if (kill(-child, SIGWINCH) != 0) {
            close(master);
            return 1;
        }
        usleep(10000);
        if (kill(-child, SIGWINCH) != 0) {
            close(master);
            return 1;
        }

        const std::string size_marker =
            "QWE_SIZE " + std::to_string(target.rows) + " " +
            std::to_string(target.cols);
        const bool observed_size =
            read_until_marker(master, output, size_marker);
        const std::string winch_marker =
            "QWE_WINCH " + std::to_string(target.rows) + " " +
            std::to_string(target.cols);
        const bool observed =
            observed_size &&
            read_until_markers(master, output, {size_marker, winch_marker});
        size_checks = size_checks && observed;
        winch_checks = winch_checks && observed;

        if (!sizes_json.empty()) {
            sizes_json += ',';
        }
        sizes_json += '"' + std::to_string(target.rows) + "x" +
                      std::to_string(target.cols) + '"';
    }

    constexpr std::string_view exit_command = "exit\r";
    if (write(master, exit_command.data(), exit_command.size()) < 0) {
        close(master);
        return 1;
    }
    const bool completed = read_to_eof(master, output);
    close(master);

    int status = 0;
    if (waitpid(child, &status, 0) < 0) {
        return 1;
    }
    const int exit_code = WIFEXITED(status) ? WEXITSTATUS(status) : 1;

    if (!log_path.empty()) {
        std::ofstream log(log_path, std::ios::binary);
        log.write(output.data(), static_cast<std::streamsize>(output.size()));
        if (!log) {
            return 1;
        }
    }

    const bool passed = ready && size_checks && winch_checks && completed &&
                        exit_code == 0;
    std::cout
        << R"({"contract":"qwe-poc/0.1","feature":"pty-resize","implementation":"cpp","exit_code":)"
        << exit_code << R"(,"bytes":)" << output.size()
        << R"(,"checks":{"ready":)" << (ready ? "true" : "false")
        << R"(,"size":)" << (size_checks ? "true" : "false")
        << R"(,"winch":)" << (winch_checks ? "true" : "false")
        << R"(},"sizes":[)" << sizes_json << "]}\n";
    return passed ? 0 : 1;
}

}  // namespace

int main(int argc, char* argv[]) {
    if (argc < 2) {
        std::cout
            << "usage: qwe-poc <probe|pty-smoke|editor-smoke|resize-smoke> [log-path]\n";
        return 0;
    }

    const std::string_view command{argv[1]};
    if (command == "probe") {
        std::cout << R"({"contract":"qwe-poc/0.1","implementation":"cpp"})"
                  << '\n';
        return 0;
    }

    if (command == "pty-smoke") {
        const std::string log_path = argc > 2 ? argv[2] : "";
        return run_pty_smoke(log_path);
    }

    if (command == "editor-smoke") {
        const std::string log_path = argc > 2 ? argv[2] : "";
        return run_editor_smoke(log_path);
    }

    if (command == "long-output-smoke") {
        const std::string log_path = argc > 2 ? argv[2] : "";
        const std::string script_path =
            argc > 3 ? argv[3] : "poc/test-data/scripts/long_output.sh";
        return run_long_output_smoke(log_path, script_path);
    }

    if (command == "vt-smoke") {
        const std::string log_path = argc > 2 ? argv[2] : "";
        const std::string fixture_path =
            argc > 3 ? argv[3] : "poc/test-data/scripts/vt_fixture.py";
        return run_vt_smoke(log_path, fixture_path);
    }

    if (command == "resize-smoke") {
        const std::string log_path = argc > 2 ? argv[2] : "";
        const std::string watcher_path =
            argc > 3 ? argv[3] : "poc/test-data/scripts/resize_watcher.py";
        return run_resize_smoke(log_path, watcher_path);
    }

    if (command == "daemon") {
        if (argc != 4) {
            std::cerr << "usage: qwe-poc daemon <socket-path> <sqlite-path>\n";
            return 2;
        }
        return run_runtime_daemon(argv[2], argv[3]);
    }

    if (command == "task-start" || command == "task-status" ||
        command == "task-stop") {
        std::vector<std::string> arguments;
        for (int index = 2; index < argc; ++index) {
            arguments.emplace_back(argv[index]);
        }
        const std::string operation =
            command == "task-start"
                ? "START"
                : (command == "task-status" ? "STATUS" : "STOP");
        return run_runtime_client(operation, arguments);
    }

    if (command == "help") {
        std::cout
            << "usage: qwe-poc <probe|pty-smoke|editor-smoke|resize-smoke> [log-path]\n";
        return 0;
    }

    std::cerr << "unsupported command: " << command << '\n';
    return 2;
}

