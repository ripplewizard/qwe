#include "runtime.hpp"

#include <sqlite3.h>

#include <algorithm>
#include <cerrno>
#include <chrono>
#include <csignal>
#include <cstdio>
#include <cstring>
#include <cstdlib>
#include <fcntl.h>
#include <fstream>
#include <iostream>
#include <memory>
#include <mutex>
#include <optional>
#include <sstream>
#include <stdexcept>
#include <string_view>
#include <sys/socket.h>
#include <sys/ioctl.h>
#include <sys/time.h>
#include <sys/un.h>
#include <sys/wait.h>
#include <thread>
#include <unordered_map>
#include <utility>
#include <unistd.h>

extern char** environ;

namespace {

constexpr int kCurrentSchemaVersion = 2;

struct Task {
    std::string id;
    std::string key;
    std::string command;
    std::string log_path;
    int pid = 0;
    int pgid = 0;
    std::string start_token;
    std::string state;
};

struct RequestError : std::runtime_error {
    std::string code;

    RequestError(std::string code_value, const std::string& message)
        : std::runtime_error(message), code(std::move(code_value)) {}
};

class Database {
  public:
    explicit Database(const std::string& path) {
        if (sqlite3_open(path.c_str(), &handle_) != SQLITE_OK) {
            const std::string message =
                handle_ == nullptr ? "sqlite open failed" : sqlite3_errmsg(handle_);
            sqlite3_close(handle_);
            handle_ = nullptr;
            throw std::runtime_error(message);
        }
        sqlite3_busy_timeout(handle_, 2000);
        sqlite3_stmt* version_statement = nullptr;
        if (sqlite3_prepare_v2(handle_, "PRAGMA user_version", -1,
                               &version_statement, nullptr) != SQLITE_OK ||
            sqlite3_step(version_statement) != SQLITE_ROW) {
            const std::string message = sqlite3_errmsg(handle_);
            sqlite3_finalize(version_statement);
            sqlite3_close(handle_);
            handle_ = nullptr;
            throw std::runtime_error(message);
        }
        const int version = sqlite3_column_int(version_statement, 0);
        sqlite3_finalize(version_statement);
        if (version > kCurrentSchemaVersion) {
            sqlite3_close(handle_);
            handle_ = nullptr;
            throw std::runtime_error(
                "database schema version " + std::to_string(version) +
                " is newer than supported version " +
                std::to_string(kCurrentSchemaVersion));
        }
        execute(
            "PRAGMA journal_mode=WAL;"
            "CREATE TABLE IF NOT EXISTS tasks ("
            "id TEXT PRIMARY KEY,"
            "idempotency_key TEXT UNIQUE NOT NULL,"
            "command TEXT NOT NULL,"
            "log_path TEXT NOT NULL,"
            "pid INTEGER NOT NULL,"
            "pgid INTEGER NOT NULL,"
            "start_token TEXT NOT NULL,"
            "state TEXT NOT NULL,"
            "created_at TEXT NOT NULL,"
            "updated_at TEXT NOT NULL"
            ");"
            "CREATE TABLE IF NOT EXISTS restart_tasks ("
            "task_key TEXT PRIMARY KEY,"
            "policy TEXT NOT NULL,"
            "max_attempts INTEGER NOT NULL,"
            "log_prefix TEXT NOT NULL,"
            "command TEXT NOT NULL,"
            "state TEXT NOT NULL,"
            "created_at INTEGER NOT NULL,"
            "updated_at INTEGER NOT NULL"
            ");"
            "CREATE TABLE IF NOT EXISTS task_instances ("
            "id TEXT PRIMARY KEY,"
            "task_key TEXT NOT NULL,"
            "attempt INTEGER NOT NULL,"
            "pid INTEGER NOT NULL,"
            "pgid INTEGER NOT NULL,"
            "start_token TEXT NOT NULL,"
            "state TEXT NOT NULL,"
            "exit_code INTEGER,"
            "log_path TEXT NOT NULL,"
            "started_at INTEGER NOT NULL,"
            "ended_at INTEGER,"
            "UNIQUE(task_key, attempt)"
            ");");
        execute("PRAGMA user_version = " +
                std::to_string(kCurrentSchemaVersion) + ";");
        sqlite3_stmt* migrated_statement = nullptr;
        if (sqlite3_prepare_v2(handle_, "PRAGMA user_version", -1,
                               &migrated_statement, nullptr) != SQLITE_OK ||
            sqlite3_step(migrated_statement) != SQLITE_ROW ||
            sqlite3_column_int(migrated_statement, 0) !=
                kCurrentSchemaVersion) {
            const std::string message = sqlite3_errmsg(handle_);
            sqlite3_finalize(migrated_statement);
            sqlite3_close(handle_);
            handle_ = nullptr;
            throw std::runtime_error(
                "database schema migration did not reach version " +
                std::to_string(kCurrentSchemaVersion) + ": " + message);
        }
        sqlite3_finalize(migrated_statement);
    }

    Database(const Database&) = delete;
    Database& operator=(const Database&) = delete;

    ~Database() { sqlite3_close(handle_); }

    sqlite3* get() const { return handle_; }

    void execute(const std::string& sql) {
        char* error = nullptr;
        if (sqlite3_exec(handle_, sql.c_str(), nullptr, nullptr, &error) != SQLITE_OK) {
            const std::string message = error == nullptr ? "sqlite error" : error;
            sqlite3_free(error);
            throw std::runtime_error(message);
        }
    }

  private:
    sqlite3* handle_ = nullptr;
};

constexpr std::string_view kTaskColumns =
    "id, idempotency_key, command, log_path, pid, pgid, start_token, state";

std::string json_escape(const std::string& value) {
    std::string escaped;
    for (char character : value) {
        switch (character) {
            case '\\':
                escaped += "\\\\";
                break;
            case '"':
                escaped += "\\\"";
                break;
            case '\n':
                escaped += "\\n";
                break;
            case '\r':
                escaped += "\\r";
                break;
            default:
                escaped += character;
                break;
        }
    }
    return escaped;
}

std::string resolve_executable(const std::string& program) {
    if (program.find('/') != std::string::npos) {
        if (access(program.c_str(), X_OK) == 0) {
            return program;
        }
        throw std::runtime_error("program is not executable: " + program);
    }
    const char* path_value = std::getenv("PATH");
    std::istringstream entries(path_value == nullptr ? "" : path_value);
    std::string directory;
    while (std::getline(entries, directory, ':')) {
        const std::string candidate =
            directory + "/" + program;
        if (access(candidate.c_str(), X_OK) == 0) {
            return candidate;
        }
    }
    throw std::runtime_error("program not found in PATH: " + program);
}

std::string now_string() {
    return std::to_string(
        std::chrono::system_clock::now().time_since_epoch().count());
}

Task read_task(sqlite3_stmt* statement) {
    Task task;
    task.id = reinterpret_cast<const char*>(sqlite3_column_text(statement, 0));
    task.key = reinterpret_cast<const char*>(sqlite3_column_text(statement, 1));
    task.command = reinterpret_cast<const char*>(sqlite3_column_text(statement, 2));
    task.log_path =
        reinterpret_cast<const char*>(sqlite3_column_text(statement, 3));
    task.pid = sqlite3_column_int(statement, 4);
    task.pgid = sqlite3_column_int(statement, 5);
    task.start_token =
        reinterpret_cast<const char*>(sqlite3_column_text(statement, 6));
    task.state = reinterpret_cast<const char*>(sqlite3_column_text(statement, 7));
    return task;
}

bool load_task(sqlite3* database, const std::string& column,
               const std::string& value, Task& task) {
    const std::string sql =
        "SELECT " + std::string(kTaskColumns) + " FROM tasks WHERE " +
        column + " = ?1";
    sqlite3_stmt* statement = nullptr;
    if (sqlite3_prepare_v2(database, sql.c_str(), -1, &statement, nullptr) !=
        SQLITE_OK) {
        throw std::runtime_error(sqlite3_errmsg(database));
    }
    sqlite3_bind_text(statement, 1, value.c_str(), -1, SQLITE_TRANSIENT);
    const int step = sqlite3_step(statement);
    if (step == SQLITE_ROW) {
        task = read_task(statement);
        sqlite3_finalize(statement);
        return true;
    }
    sqlite3_finalize(statement);
    if (step != SQLITE_DONE) {
        throw std::runtime_error(sqlite3_errmsg(database));
    }
    return false;
}

void update_state(sqlite3* database, const std::string& id,
                  const std::string& state) {
    sqlite3_stmt* statement = nullptr;
    const char* sql =
        "UPDATE tasks SET state = ?1, updated_at = ?2 WHERE id = ?3";
    if (sqlite3_prepare_v2(database, sql, -1, &statement, nullptr) != SQLITE_OK) {
        throw std::runtime_error(sqlite3_errmsg(database));
    }
    sqlite3_bind_text(statement, 1, state.c_str(), -1, SQLITE_TRANSIENT);
    sqlite3_bind_text(statement, 2, now_string().c_str(), -1, SQLITE_TRANSIENT);
    sqlite3_bind_text(statement, 3, id.c_str(), -1, SQLITE_TRANSIENT);
    const int step = sqlite3_step(statement);
    sqlite3_finalize(statement);
    if (step != SQLITE_DONE) {
        throw std::runtime_error(sqlite3_errmsg(database));
    }
}

bool transition_state(sqlite3* database, const std::string& id,
                      const std::string& from, const std::string& to) {
    sqlite3_stmt* statement = nullptr;
    const char* sql =
        "UPDATE tasks SET state = ?1, updated_at = ?2"
        " WHERE id = ?3 AND state = ?4";
    if (sqlite3_prepare_v2(database, sql, -1, &statement, nullptr) != SQLITE_OK) {
        throw std::runtime_error(sqlite3_errmsg(database));
    }
    sqlite3_bind_text(statement, 1, to.c_str(), -1, SQLITE_TRANSIENT);
    sqlite3_bind_text(statement, 2, now_string().c_str(), -1, SQLITE_TRANSIENT);
    sqlite3_bind_text(statement, 3, id.c_str(), -1, SQLITE_TRANSIENT);
    sqlite3_bind_text(statement, 4, from.c_str(), -1, SQLITE_TRANSIENT);
    const int step = sqlite3_step(statement);
    sqlite3_finalize(statement);
    if (step != SQLITE_DONE) {
        throw std::runtime_error(sqlite3_errmsg(database));
    }
    return sqlite3_changes(database) == 1;
}

std::string process_start_token(int pid) {
#ifdef __linux__
    std::ifstream stream("/proc/" + std::to_string(pid) + "/stat");
    std::string text((std::istreambuf_iterator<char>(stream)),
                     std::istreambuf_iterator<char>());
    const size_t end = text.rfind(')');
    if (end == std::string::npos) {
        throw std::runtime_error("invalid process stat");
    }
    std::istringstream fields(text.substr(end + 1));
    std::string value;
    for (int index = 0; index <= 19; ++index) {
        if (!(fields >> value)) {
            throw std::runtime_error("missing process start time");
        }
    }
    return value;
#else
    const std::string command = "ps -o lstart= -p " + std::to_string(pid);
    FILE* pipe = popen(command.c_str(), "r");
    if (pipe == nullptr) {
        throw std::runtime_error("ps failed");
    }
    char buffer[256];
    std::string token;
    if (fgets(buffer, sizeof(buffer), pipe) != nullptr) {
        token = buffer;
    }
    const int status = pclose(pipe);
    while (!token.empty() &&
           (token.back() == '\n' || token.back() == '\r' || token.back() == ' ')) {
        token.pop_back();
    }
    if (status != 0 || token.empty()) {
        throw std::runtime_error("process not found");
    }
    return token;
#endif
}

bool process_alive(int pid) {
    const int result = kill(pid, 0);
    return result == 0 || errno == EPERM;
}

bool group_alive(int pgid) {
    const int result = kill(-pgid, 0);
    return result == 0 || errno == EPERM;
}

bool task_alive(const Task& task) {
    if (!process_alive(task.pid)) {
        return false;
    }
    try {
        return process_start_token(task.pid) == task.start_token;
    } catch (const std::exception&) {
        return false;
    }
}

std::string task_response(const std::string& request_id, const Task& task,
                          const char* created = nullptr) {
    std::ostringstream response;
    response << R"({"ok":true,"request_id":")" << json_escape(request_id)
             << R"(","task_id":")" << json_escape(task.id)
             << R"(","pid":)" << task.pid << R"(,"pgid":)" << task.pgid
             << R"(,"start_token":")" << json_escape(task.start_token)
             << R"(","state":")" << json_escape(task.state) << '"';
    if (created != nullptr) {
        response << R"(,"created":)" << created;
    }
    response << '}';
    return response.str();
}

std::string error_response(const std::string& request_id,
                           const std::string& code,
                           const std::string& message) {
    return R"({"ok":false,"request_id":")" + json_escape(request_id) +
           R"(","error":")" + json_escape(code) + R"(","message":")" +
           json_escape(message) + R"("})";
}

struct LeaseRecord {
    std::string owner;
    std::chrono::steady_clock::time_point deadline;
};

class LeaseManager {
  public:
    LeaseManager() : idle_timeout_(std::chrono::seconds(600)) {
        if (const char* value = std::getenv("QWE_LEASE_MS")) {
            try {
                const long long milliseconds = std::stoll(value);
                if (milliseconds > 0) {
                    idle_timeout_ = std::chrono::milliseconds(milliseconds);
                }
            } catch (const std::exception&) {
            }
        }
    }

    std::string claim(const std::string& request_id,
                      const std::string& session_id,
                      const std::string& client_id) {
        const std::lock_guard<std::mutex> lock(mutex_);
        const auto now = std::chrono::steady_clock::now();
        const auto current = state(session_id, now);
        if (current.first == "owned") {
            return lease_error(
                request_id, "already_owned",
                "session is already owned by " + current.second.value_or(""));
        }
        sessions_[session_id] = {client_id, now + idle_timeout_};
        output_bytes_[session_id] = 0;
        return lease_response(request_id, session_id, "owned", client_id,
                              idle_timeout_);
    }

    std::string touch(const std::string& request_id,
                      const std::string& session_id,
                      const std::string& client_id) {
        const std::lock_guard<std::mutex> lock(mutex_);
        const auto now = std::chrono::steady_clock::now();
        const auto current = state(session_id, now);
        if (current.first != "owned" || current.second.value_or("") != client_id) {
            return lease_error(
                request_id, "not_owner",
                "client does not own session " + session_id);
        }
        sessions_[session_id] = {client_id, now + idle_timeout_};
        return lease_response(request_id, session_id, "owned", client_id,
                              idle_timeout_);
    }

    std::string release(const std::string& request_id,
                        const std::string& session_id,
                        const std::string& client_id) {
        const std::lock_guard<std::mutex> lock(mutex_);
        const auto now = std::chrono::steady_clock::now();
        const auto current = state(session_id, now);
        if (current.first != "owned" || current.second.value_or("") != client_id) {
            return lease_error(
                request_id, "not_owner",
                "client does not own session " + session_id);
        }
        sessions_.erase(session_id);
        return lease_response(request_id, session_id, "idle", std::nullopt,
                              std::chrono::milliseconds(0));
    }

    std::string status(const std::string& request_id,
                       const std::string& session_id) {
        const std::lock_guard<std::mutex> lock(mutex_);
        const auto now = std::chrono::steady_clock::now();
        const auto current = state(session_id, now);
        auto remaining = std::chrono::milliseconds(0);
        if (const auto found = sessions_.find(session_id);
            found != sessions_.end() && found->second.deadline > now) {
            remaining = std::chrono::duration_cast<std::chrono::milliseconds>(
                found->second.deadline - now);
        }
        return lease_response(request_id, session_id, current.first,
                              current.second, remaining);
    }

    std::string takeover(const std::string& request_id,
                         const std::string& session_id,
                         const std::string& client_id) {
        const std::lock_guard<std::mutex> lock(mutex_);
        const auto now = std::chrono::steady_clock::now();
        const auto current = state(session_id, now);
        if (current.first != "expired") {
            return lease_error(request_id, "not_expired",
                               "session lease has not expired");
        }
        sessions_[session_id] = {client_id, now + idle_timeout_};
        output_bytes_[session_id] = 0;
        return lease_response(request_id, session_id, "owned", client_id,
                              idle_timeout_);
    }

    void record_output(const std::string& session_id, long long bytes) {
        const std::lock_guard<std::mutex> lock(mutex_);
        output_bytes_[session_id] += bytes;
    }

    bool is_owner(const std::string& session_id,
                  const std::string& client_id) {
        const std::lock_guard<std::mutex> lock(mutex_);
        const auto current = state(session_id, std::chrono::steady_clock::now());
        return current.first == "owned" &&
               current.second.value_or("") == client_id;
    }

    bool authorize_input(const std::string& session_id,
                         const std::string& client_id) {
        const std::lock_guard<std::mutex> lock(mutex_);
        const auto now = std::chrono::steady_clock::now();
        const auto current = state(session_id, now);
        if (current.first != "owned" ||
            current.second.value_or("") != client_id) {
            return false;
        }
        sessions_[session_id] = {client_id, now + idle_timeout_};
        return true;
    }

  private:
    std::pair<std::string, std::optional<std::string>> state(
        const std::string& session_id,
        std::chrono::steady_clock::time_point now) const {
        const auto found = sessions_.find(session_id);
        if (found == sessions_.end()) {
            return {"idle", std::nullopt};
        }
        if (now < found->second.deadline) {
            return {"owned", found->second.owner};
        }
        return {"expired", std::nullopt};
    }

    std::string lease_response(
        const std::string& request_id,
        const std::string& session_id,
        const std::string& state,
        const std::optional<std::string>& owner,
        std::chrono::milliseconds remaining) const {
        const std::string owner_json =
            owner.has_value() ? R"(")" + json_escape(*owner) + R"(")" : "null";
        const auto remaining_ms =
            std::max<long long>(0, remaining.count());
        const auto output = output_bytes_.find(session_id);
        const auto output_bytes =
            output == output_bytes_.end() ? 0 : output->second;
        return R"({"ok":true,"request_id":")" + json_escape(request_id) +
               R"(","session_id":")" + json_escape(session_id) +
               R"(","state":")" + state + R"(","owner":)" + owner_json +
               R"(,"remaining_ms":)" + std::to_string(remaining_ms) +
               R"(,"output_bytes":)" + std::to_string(output_bytes) + "}";
    }

    static std::string lease_error(const std::string& request_id,
                                   const std::string& code,
                                   const std::string& message) {
        return R"({"ok":false,"request_id":")" + json_escape(request_id) +
               R"(","error":")" + code + R"(","message":")" +
               json_escape(message) + R"("})";
    }

    std::chrono::milliseconds idle_timeout_;
    std::unordered_map<std::string, LeaseRecord> sessions_;
    std::unordered_map<std::string, long long> output_bytes_;
    std::mutex mutex_;
};

struct Session {
    int master = -1;
    pid_t pid = -1;
    bool running = true;
    bool attached = false;
    std::string output;
    std::mutex mutex;
};

std::string encode_hex(const std::string& input) {
    static constexpr char digits[] = "0123456789abcdef";
    std::string result;
    result.reserve(input.size() * 2);
    for (const unsigned char value : input) {
        result.push_back(digits[value >> 4]);
        result.push_back(digits[value & 0x0f]);
    }
    return result;
}

std::string decode_hex(const std::string& input) {
    if (input.size() % 2 != 0) {
        throw std::runtime_error(
            "hex input must contain an even number of characters");
    }
    const auto value = [](char character) -> unsigned int {
        if (character >= '0' && character <= '9') {
            return static_cast<unsigned int>(character - '0');
        }
        if (character >= 'a' && character <= 'f') {
            return static_cast<unsigned int>(character - 'a' + 10);
        }
        if (character >= 'A' && character <= 'F') {
            return static_cast<unsigned int>(character - 'A' + 10);
        }
        throw std::runtime_error("invalid hexadecimal input");
    };
    std::string result;
    result.reserve(input.size() / 2);
    for (size_t index = 0; index < input.size(); index += 2) {
        result.push_back(static_cast<char>(
            (value(input[index]) << 4) | value(input[index + 1])));
    }
    return result;
}

class SessionManager {
  public:
    explicit SessionManager(std::shared_ptr<LeaseManager> leases)
        : leases_(std::move(leases)) {}

    std::string start(const std::string& request_id,
                      const std::string& session_id,
                      const std::string& log_path,
                      const std::string& program,
                      const std::vector<std::string>& arguments) {
        std::unique_lock<std::mutex> sessions_lock(mutex_);
        if (sessions_.contains(session_id)) {
            return session_error(request_id, "already_exists",
                                 "session already exists: " + session_id);
        }
        const std::string executable = resolve_executable(program);
        std::vector<char*> argv;
        argv.push_back(const_cast<char*>(executable.c_str()));
        for (const auto& argument : arguments) {
            argv.push_back(const_cast<char*>(argument.c_str()));
        }
        argv.push_back(nullptr);

        const int output =
            open(log_path.c_str(), O_CREAT | O_TRUNC | O_WRONLY | O_CLOEXEC, 0644);
        if (output < 0) {
            throw std::runtime_error(std::strerror(errno));
        }
        const int master = posix_openpt(O_RDWR | O_NOCTTY);
        if (master < 0) {
            close(output);
            throw std::runtime_error(std::strerror(errno));
        }
        if (grantpt(master) != 0 || unlockpt(master) != 0) {
            close(master);
            close(output);
            throw std::runtime_error(std::strerror(errno));
        }
        const char* slave_name = ptsname(master);
        if (slave_name == nullptr) {
            close(master);
            close(output);
            throw std::runtime_error("ptsname failed");
        }
        const int slave = open(slave_name, O_RDWR | O_NOCTTY);
        if (slave < 0) {
            close(master);
            close(output);
            throw std::runtime_error(std::strerror(errno));
        }
        winsize size{};
        size.ws_row = 24;
        size.ws_col = 80;
        if (ioctl(master, TIOCSWINSZ, &size) != 0) {
            close(slave);
            close(master);
            close(output);
            throw std::runtime_error(std::strerror(errno));
        }

        const pid_t child = fork();
        if (child < 0) {
            close(slave);
            close(master);
            close(output);
            throw std::runtime_error(std::strerror(errno));
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
            execve(executable.c_str(), argv.data(), environ);
            _exit(127);
        }
        close(slave);
        auto session = std::make_shared<Session>();
        session->master = master;
        session->pid = child;
        sessions_[session_id] = session;
        std::thread([master, output, child, session_id, session, leases = leases_]() {
            char buffer[4096];
            while (true) {
                const ssize_t count = read(master, buffer, sizeof(buffer));
                if (count <= 0) {
                    break;
                }
                size_t offset = 0;
                while (offset < static_cast<size_t>(count)) {
                    const ssize_t written =
                        write(output, buffer + offset,
                              static_cast<size_t>(count) - offset);
                    if (written <= 0) {
                        break;
                    }
                    offset += static_cast<size_t>(written);
                }
                {
                    const std::lock_guard<std::mutex> lock(session->mutex);
                    session->output.append(buffer, static_cast<size_t>(count));
                }
                leases->record_output(session_id, count);
            }
            close(master);
            close(output);
            int status = 0;
            (void)waitpid(child, &status, 0);
            const std::lock_guard<std::mutex> lock(session->mutex);
            session->running = false;
            session->attached = false;
        }).detach();

        sessions_lock.unlock();
        const std::lock_guard<std::mutex> lock(session->mutex);
        return session_response(request_id, session_id, *session);
    }

    std::string attach(const std::string& request_id,
                       const std::string& session_id,
                       const std::string& client_id) {
        if (!leases_->is_owner(session_id, client_id)) {
            return session_error(request_id, "not_owner",
                                 "client does not own session " + session_id);
        }
        const auto session = find(session_id);
        if (!session) {
            return session_error(request_id, "not_found",
                                 "session not found: " + session_id);
        }
        const std::lock_guard<std::mutex> lock(session->mutex);
        session->attached = true;
        return session_response(request_id, session_id, *session);
    }

    std::string detach(const std::string& request_id,
                       const std::string& session_id,
                       const std::string& client_id) {
        if (!leases_->is_owner(session_id, client_id)) {
            return session_error(request_id, "not_owner",
                                 "client does not own session " + session_id);
        }
        const auto session = find(session_id);
        if (!session) {
            return session_error(request_id, "not_found",
                                 "session not found: " + session_id);
        }
        const std::lock_guard<std::mutex> lock(session->mutex);
        session->attached = false;
        return session_response(request_id, session_id, *session);
    }

    std::string input(const std::string& request_id,
                      const std::string& session_id,
                      const std::string& client_id,
                      const std::string& encoded) {
        std::string data;
        try {
            data = decode_hex(encoded);
        } catch (const std::exception& error) {
            return session_error(request_id, "invalid_request", error.what());
        }
        if (!leases_->authorize_input(session_id, client_id)) {
            return session_error(request_id, "not_owner",
                                 "client does not own session " + session_id);
        }
        const auto session = find(session_id);
        if (!session) {
            return session_error(request_id, "not_found",
                                 "session not found: " + session_id);
        }
        const std::lock_guard<std::mutex> lock(session->mutex);
        if (!session->running) {
            return session_error(request_id, "not_running",
                                 "session is not running: " + session_id);
        }
        if (!session->attached) {
            return session_error(request_id, "not_attached",
                                 "session is detached: " + session_id);
        }
        size_t offset = 0;
        while (offset < data.size()) {
            const ssize_t written =
                write(session->master, data.data() + offset, data.size() - offset);
            if (written <= 0) {
                throw std::runtime_error(std::strerror(errno));
            }
            offset += static_cast<size_t>(written);
        }
        const std::string response =
            session_response(request_id, session_id, *session);
        return response;
    }

    std::string status(const std::string& request_id,
                       const std::string& session_id) {
        const auto session = find(session_id);
        if (!session) {
            return session_error(request_id, "not_found",
                                 "session not found: " + session_id);
        }
        const std::lock_guard<std::mutex> lock(session->mutex);
        return session_response(request_id, session_id, *session);
    }

  private:
    std::shared_ptr<Session> find(const std::string& session_id) {
        const std::lock_guard<std::mutex> lock(mutex_);
        const auto found = sessions_.find(session_id);
        return found == sessions_.end() ? nullptr : found->second;
    }

    static std::string session_response(const std::string& request_id,
                                        const std::string& session_id,
                                        const Session& session) {
        return R"({"ok":true,"request_id":")" + json_escape(request_id) +
               R"(","session_id":")" + json_escape(session_id) +
               R"(","pid":)" + std::to_string(session.pid) +
               R"(,"running":)" + (session.running ? "true" : "false") +
               R"(,"attached":)" + (session.attached ? "true" : "false") +
               R"(,"output_bytes":)" + std::to_string(session.output.size()) +
               R"(,"snapshot_hex":")" + encode_hex(session.output) + R"("})";
    }

    static std::string session_error(const std::string& request_id,
                                     const std::string& code,
                                     const std::string& message) {
        return R"({"ok":false,"request_id":")" + json_escape(request_id) +
               R"(","error":")" + code + R"(","message":")" +
               json_escape(message) + R"("})";
    }

    std::shared_ptr<LeaseManager> leases_;
    std::mutex mutex_;
    std::unordered_map<std::string, std::shared_ptr<Session>> sessions_;
};

void recover(Database& database) {
    sqlite3_stmt* statement = nullptr;
    const std::string sql =
        "SELECT " + std::string(kTaskColumns) +
        " FROM tasks WHERE state = 'running'";
    if (sqlite3_prepare_v2(database.get(), sql.c_str(), -1, &statement, nullptr) !=
        SQLITE_OK) {
        throw std::runtime_error(sqlite3_errmsg(database.get()));
    }
    std::vector<Task> tasks;
    int step;
    while ((step = sqlite3_step(statement)) == SQLITE_ROW) {
        tasks.push_back(read_task(statement));
    }
    sqlite3_finalize(statement);
    if (step != SQLITE_DONE) {
        throw std::runtime_error(sqlite3_errmsg(database.get()));
    }
    for (const auto& task : tasks) {
        if (!task_alive(task)) {
            update_state(database.get(), task.id, "exited");
        }
    }
}

std::string now_millis() {
    return std::to_string(
        std::chrono::duration_cast<std::chrono::milliseconds>(
            std::chrono::system_clock::now().time_since_epoch())
            .count());
}

void register_task_instance(Database& database, const Task& task) {
    if (task.state != "running") {
        return;
    }
    sqlite3_stmt* statement = nullptr;
    const char* sql =
        "INSERT INTO task_instances"
        "(id, task_key, attempt, pid, pgid, start_token, state, exit_code,"
        " log_path, started_at)"
        " VALUES (?1, ?2, 1, ?3, ?4, ?5, 'running', NULL, ?6, ?7)";
    if (sqlite3_prepare_v2(database.get(), sql, -1, &statement, nullptr) !=
        SQLITE_OK) {
        throw std::runtime_error(sqlite3_errmsg(database.get()));
    }
    const std::string instance_id = task.id + "-instance-1";
    sqlite3_bind_text(statement, 1, instance_id.c_str(), -1, SQLITE_TRANSIENT);
    sqlite3_bind_text(statement, 2, task.id.c_str(), -1, SQLITE_TRANSIENT);
    sqlite3_bind_int(statement, 3, task.pid);
    sqlite3_bind_int(statement, 4, task.pgid);
    sqlite3_bind_text(statement, 5, task.start_token.c_str(), -1,
                      SQLITE_TRANSIENT);
    sqlite3_bind_text(statement, 6, task.log_path.c_str(), -1, SQLITE_TRANSIENT);
    sqlite3_bind_text(statement, 7, now_millis().c_str(), -1, SQLITE_TRANSIENT);
    const int step = sqlite3_step(statement);
    sqlite3_finalize(statement);
    if (step != SQLITE_DONE) {
        throw std::runtime_error(sqlite3_errmsg(database.get()));
    }
}

void finish_task_instance(const std::shared_ptr<Database>& database,
                          const std::shared_ptr<std::mutex>& state_mutex,
                          const std::string& task_id, int exit_code) {
    const std::lock_guard<std::mutex> lock(*state_mutex);
    const std::string state = exit_code == 0 ? "exited" : "failed";
    sqlite3_stmt* statement = nullptr;
    const char* sql =
        "UPDATE task_instances SET state = ?1, exit_code = ?2, ended_at = ?3"
        " WHERE task_key = ?4 AND attempt = 1";
    if (sqlite3_prepare_v2(database->get(), sql, -1, &statement, nullptr) ==
        SQLITE_OK) {
        sqlite3_bind_text(statement, 1, state.c_str(), -1, SQLITE_TRANSIENT);
        sqlite3_bind_int(statement, 2, exit_code);
        sqlite3_bind_text(statement, 3, now_millis().c_str(), -1,
                          SQLITE_TRANSIENT);
        sqlite3_bind_text(statement, 4, task_id.c_str(), -1, SQLITE_TRANSIENT);
        (void)sqlite3_step(statement);
    }
    sqlite3_finalize(statement);
    transition_state(database->get(), task_id, "running", state);
}

std::string task_history_response(Database& database,
                                  const std::string& request_id,
                                  const std::string& task_id) {
    Task task;
    if (!load_task(database.get(), "id", task_id, task)) {
        return error_response(request_id, "not_found",
                              "task not found: " + task_id);
    }
    sqlite3_stmt* statement = nullptr;
    const char* sql =
        "SELECT attempt, pid, pgid, start_token, state, exit_code, started_at,"
        " ended_at, log_path FROM task_instances"
        " WHERE task_key = ?1 ORDER BY attempt";
    if (sqlite3_prepare_v2(database.get(), sql, -1, &statement, nullptr) !=
        SQLITE_OK) {
        throw std::runtime_error(sqlite3_errmsg(database.get()));
    }
    sqlite3_bind_text(statement, 1, task_id.c_str(), -1, SQLITE_TRANSIENT);
    std::string instances;
    int count = 0;
    int step;
    while ((step = sqlite3_step(statement)) == SQLITE_ROW) {
        if (count++ != 0) {
            instances += ',';
        }
        const int attempt = sqlite3_column_int(statement, 0);
        const int pid = sqlite3_column_int(statement, 1);
        const int pgid = sqlite3_column_int(statement, 2);
        const std::string token = reinterpret_cast<const char*>(
            sqlite3_column_text(statement, 3));
        const std::string state = reinterpret_cast<const char*>(
            sqlite3_column_text(statement, 4));
        const bool has_exit = sqlite3_column_type(statement, 5) != SQLITE_NULL;
        const int exit_code = sqlite3_column_int(statement, 5);
        const long long started =
            std::stoll(reinterpret_cast<const char*>(
                sqlite3_column_text(statement, 6)));
        const bool has_ended = sqlite3_column_type(statement, 7) != SQLITE_NULL;
        const long long ended =
            has_ended
                ? std::stoll(reinterpret_cast<const char*>(
                      sqlite3_column_text(statement, 7)))
                : 0;
        const std::string log_path = reinterpret_cast<const char*>(
            sqlite3_column_text(statement, 8));
        instances +=
            R"({"attempt":)" + std::to_string(attempt) +
            R"(,"pid":)" + std::to_string(pid) +
            R"(,"pgid":)" + std::to_string(pgid) +
            R"(,"start_token":")" + json_escape(token) +
            R"(","state":")" + json_escape(state) +
            R"(","exit_code":)" + (has_exit ? std::to_string(exit_code) : "null") +
            R"(,"started_at":)" + std::to_string(started) +
            R"(,"ended_at":)" + (has_ended ? std::to_string(ended) : "null") +
            R"(,"duration_ms":)" +
            (has_ended ? std::to_string(ended - started) : "null") +
            R"(,"log_path":")" + json_escape(log_path) + R"("})";
    }
    sqlite3_finalize(statement);
    if (step != SQLITE_DONE) {
        throw std::runtime_error(sqlite3_errmsg(database.get()));
    }
    return R"({"ok":true,"request_id":")" + json_escape(request_id) +
           R"(","task_id":")" + json_escape(task_id) +
           R"(","attempts":)" + std::to_string(count) +
           R"(,"instances":[)" + instances + "]}";
}

std::string signal_task_response(Database& database,
                                 const std::string& request_id,
                                 const std::string& task_id,
                                 const std::string& signal_name) {
    Task task;
    if (!load_task(database.get(), "id", task_id, task)) {
        return error_response(request_id, "not_found",
                              "task not found: " + task_id);
    }
    if (task.state != "running" || !task_alive(task)) {
        return error_response(request_id, "invalid_request",
                              "task is not running: " + task_id);
    }
    int signal_number = 0;
    if (signal_name == "SIGINT") {
        signal_number = SIGINT;
    } else if (signal_name == "SIGTERM") {
        signal_number = SIGTERM;
    } else if (signal_name == "SIGKILL") {
        signal_number = SIGKILL;
    } else {
        return error_response(request_id, "invalid_request",
                              "signal must be SIGINT, SIGTERM, or SIGKILL");
    }
    if (kill(-task.pgid, signal_number) != 0) {
        return error_response(request_id, "internal",
                              std::strerror(errno));
    }
    return task_response(request_id, task);
}

std::pair<Task, bool> start_task(
    Database& database, const std::shared_ptr<Database>& shared_database,
    const std::shared_ptr<std::mutex>& state_mutex, const std::string& key,
                                const std::string& log_path,
                                const std::string& program,
                                const std::vector<std::string>& arguments) {
    Task existing;
    if (load_task(database.get(), "idempotency_key", key, existing)) {
        if (existing.state == "running" && task_alive(existing)) {
            return {existing, false};
        }
        throw RequestError(
            "internal",
            "idempotency key already used by a non-running task");
    }

    const int log = open(log_path.c_str(), O_CREAT | O_APPEND | O_WRONLY, 0644);
    if (log < 0) {
        throw std::runtime_error(std::strerror(errno));
    }
    const pid_t child = fork();
    if (child < 0) {
        close(log);
        throw std::runtime_error(std::strerror(errno));
    }
    if (child == 0) {
        if (setsid() < 0 || dup2(log, STDOUT_FILENO) < 0 ||
            dup2(log, STDERR_FILENO) < 0) {
            _exit(126);
        }
        const int null_fd = open("/dev/null", O_RDONLY);
        if (null_fd >= 0) {
            dup2(null_fd, STDIN_FILENO);
            close(null_fd);
        }
        close(log);
        std::vector<char*> argv;
        argv.push_back(const_cast<char*>(program.c_str()));
        for (const auto& argument : arguments) {
            argv.push_back(const_cast<char*>(argument.c_str()));
        }
        argv.push_back(nullptr);
        execvp(program.c_str(), argv.data());
        _exit(127);
    }
    close(log);

    Task task;
    task.id = "task-" + std::to_string(child) + "-" + now_string();
    task.key = key;
    task.command = program;
    for (const auto& argument : arguments) {
        task.command += " " + argument;
    }
    task.log_path = log_path;
    task.pid = child;
    task.pgid = child;
    for (int attempt = 0; attempt < 10; ++attempt) {
        std::this_thread::sleep_for(std::chrono::milliseconds(10));
        try {
            task.start_token = process_start_token(child);
            task.state = "running";
            break;
        } catch (const std::exception&) {
            if (!process_alive(child)) {
                task.state = "exited";
                break;
            }
        }
    }
    if (task.state.empty()) {
        kill(-child, SIGKILL);
        throw std::runtime_error(
            "task identity unavailable for a running process");
    }

    sqlite3_stmt* statement = nullptr;
    const char* sql =
        "INSERT INTO tasks"
        "(id, idempotency_key, command, log_path, pid, pgid, start_token,"
        " state, created_at, updated_at)"
        " VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)";
    if (sqlite3_prepare_v2(database.get(), sql, -1, &statement, nullptr) !=
        SQLITE_OK) {
        kill(-child, SIGKILL);
        throw std::runtime_error(sqlite3_errmsg(database.get()));
    }
    sqlite3_bind_text(statement, 1, task.id.c_str(), -1, SQLITE_TRANSIENT);
    sqlite3_bind_text(statement, 2, task.key.c_str(), -1, SQLITE_TRANSIENT);
    sqlite3_bind_text(statement, 3, task.command.c_str(), -1, SQLITE_TRANSIENT);
    sqlite3_bind_text(statement, 4, task.log_path.c_str(), -1, SQLITE_TRANSIENT);
    sqlite3_bind_int(statement, 5, task.pid);
    sqlite3_bind_int(statement, 6, task.pgid);
    sqlite3_bind_text(statement, 7, task.start_token.c_str(), -1,
                      SQLITE_TRANSIENT);
    sqlite3_bind_text(statement, 8, task.state.c_str(), -1, SQLITE_TRANSIENT);
    sqlite3_bind_text(statement, 9, now_string().c_str(), -1, SQLITE_TRANSIENT);
    const int step = sqlite3_step(statement);
    sqlite3_finalize(statement);
    if (step != SQLITE_DONE) {
        kill(-child, SIGKILL);
        int ignored_status = 0;
        (void)waitpid(child, &ignored_status, 0);
        throw std::runtime_error(sqlite3_errmsg(database.get()));
    }
    try {
        register_task_instance(database, task);
    } catch (const std::exception&) {
        kill(-child, SIGKILL);
        int ignored_status = 0;
        (void)waitpid(child, &ignored_status, 0);
        update_state(database.get(), task.id, "failed");
        throw;
    }
    std::thread([child, shared_database, state_mutex, task_id = task.id]() {
        int status = 0;
        const pid_t result = waitpid(child, &status, 0);
        const int exit_code =
            result < 0
                ? 1
                : (WIFEXITED(status)
                       ? WEXITSTATUS(status)
                       : (WIFSIGNALED(status) ? 128 + WTERMSIG(status) : 1));
        finish_task_instance(
            shared_database, state_mutex, task_id, exit_code);
    }).detach();
    return {task, true};
}

Task stop_task(Database& database, const std::string& id) {
    Task task;
    if (!load_task(database.get(), "id", id, task)) {
        throw RequestError("not_found", "task not found: " + id);
    }
    if (task.state != "running") {
        return task;
    }
    if (group_alive(task.pgid)) {
        kill(-task.pgid, SIGTERM);
        const auto deadline =
            std::chrono::steady_clock::now() + std::chrono::seconds(2);
        while (group_alive(task.pgid) &&
               std::chrono::steady_clock::now() < deadline) {
            std::this_thread::sleep_for(std::chrono::milliseconds(50));
        }
        if (group_alive(task.pgid)) {
            kill(-task.pgid, SIGKILL);
        }
    }
    update_state(database.get(), id, "stopped");
    task.state = "stopped";
    return task;
}

void enforce_task_timeout(const std::shared_ptr<Database>& database,
                          const std::shared_ptr<std::mutex>& state_mutex,
                          const Task& task,
                          std::chrono::milliseconds timeout) {
    std::this_thread::sleep_for(timeout);
    {
        const std::lock_guard<std::mutex> lock(*state_mutex);
        Task current;
        if (!load_task(database->get(), "id", task.id, current) ||
            current.state != "running" || !task_alive(current)) {
            return;
        }
        if (!transition_state(database->get(), task.id, "running", "timing_out")) {
            return;
        }
    }
    if (group_alive(task.pgid)) {
        kill(-task.pgid, SIGTERM);
        const auto deadline =
            std::chrono::steady_clock::now() + std::chrono::milliseconds(200);
        while (group_alive(task.pgid) &&
               std::chrono::steady_clock::now() < deadline) {
            std::this_thread::sleep_for(std::chrono::milliseconds(20));
        }
        if (group_alive(task.pgid)) {
            kill(-task.pgid, SIGKILL);
        }
    }
    for (int attempt = 0; attempt < 50 && group_alive(task.pgid); ++attempt) {
        std::this_thread::sleep_for(std::chrono::milliseconds(10));
    }
    const std::lock_guard<std::mutex> lock(*state_mutex);
    transition_state(database->get(), task.id, "timing_out",
                     group_alive(task.pgid) ? "timeout_failed" : "timed_out");
}

std::pair<Task, bool> start_task_with_timeout(
    Database& database, const std::shared_ptr<Database>& shared_database,
    const std::shared_ptr<std::mutex>& state_mutex,
    const std::string& key, const std::string& log_path,
    const std::string& timeout_text, const std::string& program,
    const std::vector<std::string>& arguments) {
    long long timeout_ms = 0;
    try {
        size_t consumed = 0;
        timeout_ms = std::stoll(timeout_text, &consumed);
        if (consumed != timeout_text.size()) {
            timeout_ms = 0;
        }
    } catch (const std::exception&) {
    }
    if (timeout_ms <= 0) {
        throw RequestError("invalid_request",
                           "timeout must be a positive integer");
    }
    const auto started = std::chrono::steady_clock::now();
    auto [task, created] = start_task(
        database, shared_database, state_mutex, key, log_path, program,
        arguments);
    if (created && task.state == "running") {
        const auto elapsed = std::chrono::duration_cast<std::chrono::milliseconds>(
            std::chrono::steady_clock::now() - started);
        const auto remaining =
            std::max(std::chrono::milliseconds(0),
                     std::chrono::milliseconds(timeout_ms) - elapsed);
        std::thread([shared_database, state_mutex, task, remaining]() {
            enforce_task_timeout(
                shared_database, state_mutex, task, remaining);
        }).detach();
    }
    return {task, created};
}

struct RestartTask {
    std::string key;
    std::string policy;
    int max_attempts = 0;
    std::string log_prefix;
    std::string state;
};

struct RestartInstance {
    int attempt = 0;
    int pid = 0;
    int pgid = 0;
    std::string start_token;
    std::string state;
    bool has_exit_code = false;
    int exit_code = 0;
    std::string log_path;
};

bool load_restart_task(Database& database, const std::string& key,
                       RestartTask& task) {
    sqlite3_stmt* statement = nullptr;
    const char* sql =
        "SELECT task_key, policy, max_attempts, log_prefix, state"
        " FROM restart_tasks WHERE task_key = ?1";
    if (sqlite3_prepare_v2(database.get(), sql, -1, &statement, nullptr) !=
        SQLITE_OK) {
        throw std::runtime_error(sqlite3_errmsg(database.get()));
    }
    sqlite3_bind_text(statement, 1, key.c_str(), -1, SQLITE_TRANSIENT);
    const int step = sqlite3_step(statement);
    if (step == SQLITE_ROW) {
        task.key = reinterpret_cast<const char*>(
            sqlite3_column_text(statement, 0));
        task.policy = reinterpret_cast<const char*>(
            sqlite3_column_text(statement, 1));
        task.max_attempts = sqlite3_column_int(statement, 2);
        task.log_prefix = reinterpret_cast<const char*>(
            sqlite3_column_text(statement, 3));
        task.state = reinterpret_cast<const char*>(
            sqlite3_column_text(statement, 4));
    }
    sqlite3_finalize(statement);
    if (step != SQLITE_ROW && step != SQLITE_DONE) {
        throw std::runtime_error(sqlite3_errmsg(database.get()));
    }
    return step == SQLITE_ROW;
}

std::vector<RestartInstance> load_restart_instances(
    Database& database, const std::string& key) {
    sqlite3_stmt* statement = nullptr;
    const char* sql =
        "SELECT attempt, pid, pgid, start_token, state, exit_code, log_path"
        " FROM task_instances WHERE task_key = ?1 ORDER BY attempt";
    if (sqlite3_prepare_v2(database.get(), sql, -1, &statement, nullptr) !=
        SQLITE_OK) {
        throw std::runtime_error(sqlite3_errmsg(database.get()));
    }
    sqlite3_bind_text(statement, 1, key.c_str(), -1, SQLITE_TRANSIENT);
    std::vector<RestartInstance> instances;
    int step;
    while ((step = sqlite3_step(statement)) == SQLITE_ROW) {
        RestartInstance instance;
        instance.attempt = sqlite3_column_int(statement, 0);
        instance.pid = sqlite3_column_int(statement, 1);
        instance.pgid = sqlite3_column_int(statement, 2);
        instance.start_token = reinterpret_cast<const char*>(
            sqlite3_column_text(statement, 3));
        instance.state = reinterpret_cast<const char*>(
            sqlite3_column_text(statement, 4));
        instance.has_exit_code = sqlite3_column_type(statement, 5) != SQLITE_NULL;
        instance.exit_code = sqlite3_column_int(statement, 5);
        instance.log_path = reinterpret_cast<const char*>(
            sqlite3_column_text(statement, 6));
        instances.push_back(instance);
    }
    sqlite3_finalize(statement);
    if (step != SQLITE_DONE) {
        throw std::runtime_error(sqlite3_errmsg(database.get()));
    }
    return instances;
}

std::string restart_status_response(
    const std::string& request_id, Database& database, const std::string& key,
    std::optional<bool> created = std::nullopt) {
    RestartTask task;
    if (!load_restart_task(database, key, task)) {
        return error_response(request_id, "not_found",
                              "restart task not found: " + key);
    }
    const auto instances = load_restart_instances(database, key);
    std::string instance_json;
    for (size_t index = 0; index < instances.size(); ++index) {
        const auto& instance = instances[index];
        if (index != 0) {
            instance_json += ',';
        }
        instance_json +=
            R"({"attempt":)" + std::to_string(instance.attempt) +
            R"(,"pid":)" + std::to_string(instance.pid) +
            R"(,"pgid":)" + std::to_string(instance.pgid) +
            R"(,"start_token":")" + json_escape(instance.start_token) +
            R"(","state":")" + json_escape(instance.state) +
            R"(","exit_code":)" +
            (instance.has_exit_code ? std::to_string(instance.exit_code) : "null") +
            R"(,"log_path":")" + json_escape(instance.log_path) + R"("})";
    }
    const std::string created_json =
        created.has_value()
            ? R"(,"created":)" + std::string(*created ? "true" : "false")
            : "";
    return R"({"ok":true,"request_id":")" + json_escape(request_id) +
           R"(","task_key":")" + json_escape(task.key) +
           R"(","policy":")" + json_escape(task.policy) +
           R"(","state":")" + json_escape(task.state) + '"' + created_json +
           R"(,"attempts":)" + std::to_string(instances.size()) +
           R"(,"instances":[)" + instance_json + "]}";
}

std::optional<int> run_restart_attempt(
    const std::shared_ptr<Database>& database,
    const std::shared_ptr<std::mutex>& state_mutex, const RestartTask& task,
    int attempt, const std::string& program,
    const std::vector<std::string>& arguments) {
    const std::string executable = resolve_executable(program);
    std::vector<char*> argv;
    argv.push_back(const_cast<char*>(executable.c_str()));
    for (const auto& argument : arguments) {
        argv.push_back(const_cast<char*>(argument.c_str()));
    }
    argv.push_back(nullptr);
    const std::string log_path =
        task.log_prefix + "." + std::to_string(attempt) + ".log";
    const int log =
        open(log_path.c_str(), O_CREAT | O_APPEND | O_WRONLY | O_CLOEXEC, 0644);
    if (log < 0) {
        return std::nullopt;
    }
    const pid_t child = fork();
    if (child < 0) {
        close(log);
        return std::nullopt;
    }
    if (child == 0) {
        if (setsid() < 0 || dup2(log, STDOUT_FILENO) < 0 ||
            dup2(log, STDERR_FILENO) < 0) {
            _exit(126);
        }
        const int null_fd = open("/dev/null", O_RDONLY);
        if (null_fd >= 0) {
            dup2(null_fd, STDIN_FILENO);
            close(null_fd);
        }
        close(log);
        execve(executable.c_str(), argv.data(), environ);
        _exit(127);
    }
    close(log);
    std::string start_token;
    for (int retry = 0; retry < 10; ++retry) {
        std::this_thread::sleep_for(std::chrono::milliseconds(10));
        try {
            start_token = process_start_token(child);
            break;
        } catch (const std::exception&) {
            if (!process_alive(child)) {
                break;
            }
        }
    }
    if (start_token.empty() && process_alive(child)) {
        kill(-child, SIGKILL);
        int ignored_status = 0;
        (void)waitpid(child, &ignored_status, 0);
        return std::nullopt;
    }
    const std::string instance_id =
        task.key + "-" + std::to_string(attempt) + "-" + now_string();
    {
        const std::lock_guard<std::mutex> lock(*state_mutex);
        sqlite3_stmt* statement = nullptr;
        const char* sql =
            "INSERT INTO task_instances"
            "(id, task_key, attempt, pid, pgid, start_token, state, exit_code,"
            " log_path, started_at)"
            " VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'running', NULL, ?7, ?8)";
        if (sqlite3_prepare_v2(database->get(), sql, -1, &statement, nullptr) !=
            SQLITE_OK) {
            kill(-child, SIGKILL);
            int ignored_status = 0;
            (void)waitpid(child, &ignored_status, 0);
            return std::nullopt;
        }
        sqlite3_bind_text(statement, 1, instance_id.c_str(), -1, SQLITE_TRANSIENT);
        sqlite3_bind_text(statement, 2, task.key.c_str(), -1, SQLITE_TRANSIENT);
        sqlite3_bind_int(statement, 3, attempt);
        sqlite3_bind_int(statement, 4, child);
        sqlite3_bind_int(statement, 5, child);
        sqlite3_bind_text(statement, 6, start_token.c_str(), -1, SQLITE_TRANSIENT);
        sqlite3_bind_text(statement, 7, log_path.c_str(), -1, SQLITE_TRANSIENT);
        sqlite3_bind_text(statement, 8, now_string().c_str(), -1, SQLITE_TRANSIENT);
        const int step = sqlite3_step(statement);
        sqlite3_finalize(statement);
        if (step != SQLITE_DONE) {
            kill(-child, SIGKILL);
            int ignored_status = 0;
            (void)waitpid(child, &ignored_status, 0);
            return std::nullopt;
        }
    }
    int status = 0;
    if (waitpid(child, &status, 0) < 0) {
        return std::nullopt;
    }
    const int exit_code = WIFEXITED(status)
                              ? WEXITSTATUS(status)
                              : (WIFSIGNALED(status) ? 128 + WTERMSIG(status) : 1);
    {
        const std::lock_guard<std::mutex> lock(*state_mutex);
        sqlite3_stmt* statement = nullptr;
        const char* sql =
            "UPDATE task_instances SET state = 'exited', exit_code = ?1,"
            " ended_at = ?2 WHERE id = ?3";
        if (sqlite3_prepare_v2(database->get(), sql, -1, &statement, nullptr) !=
            SQLITE_OK) {
            return std::nullopt;
        }
        sqlite3_bind_int(statement, 1, exit_code);
        sqlite3_bind_text(statement, 2, now_string().c_str(), -1, SQLITE_TRANSIENT);
        sqlite3_bind_text(statement, 3, instance_id.c_str(), -1, SQLITE_TRANSIENT);
        const int step = sqlite3_step(statement);
        sqlite3_finalize(statement);
        if (step != SQLITE_DONE) {
            return std::nullopt;
        }
    }
    return exit_code;
}

void run_restart_sequence(
    const std::shared_ptr<Database>& database,
    const std::shared_ptr<std::mutex>& state_mutex, RestartTask task,
    std::string program, std::vector<std::string> arguments) {
    bool failed = false;
    for (int attempt = 1; attempt <= task.max_attempts; ++attempt) {
        std::optional<int> exit_code;
        try {
            exit_code =
                run_restart_attempt(database, state_mutex, task, attempt,
                                    program, arguments);
        } catch (const std::exception&) {
            exit_code = std::nullopt;
        }
        if (!exit_code.has_value()) {
            failed = true;
            break;
        }
        const bool restart =
            task.policy == "always" ||
            (task.policy == "on-failure" && *exit_code != 0);
        if (!restart) {
            break;
        }
    }
    const std::lock_guard<std::mutex> lock(*state_mutex);
    sqlite3_stmt* statement = nullptr;
    const char* sql =
        "UPDATE restart_tasks SET state = ?1, updated_at = ?2"
        " WHERE task_key = ?3";
    if (sqlite3_prepare_v2(database->get(), sql, -1, &statement, nullptr) ==
        SQLITE_OK) {
        sqlite3_bind_text(statement, 1, failed ? "failed" : "completed", -1,
                          SQLITE_TRANSIENT);
        sqlite3_bind_text(statement, 2, now_string().c_str(), -1, SQLITE_TRANSIENT);
        sqlite3_bind_text(statement, 3, task.key.c_str(), -1, SQLITE_TRANSIENT);
        (void)sqlite3_step(statement);
    }
    sqlite3_finalize(statement);
}

std::string start_restart_task(
    Database& database, const std::shared_ptr<Database>& shared_database,
    const std::shared_ptr<std::mutex>& state_mutex, const std::string& request_id,
    const std::string& key, const std::string& log_prefix,
    const std::string& policy, const std::string& max_attempts_text,
    const std::string& program, const std::vector<std::string>& arguments) {
    if (policy != "never" && policy != "on-failure" && policy != "always") {
        return error_response(request_id, "invalid_request",
                              "policy must be never, on-failure, or always");
    }
    long long max_attempts = 0;
    try {
        size_t consumed = 0;
        max_attempts = std::stoll(max_attempts_text, &consumed);
        if (consumed != max_attempts_text.size()) {
            max_attempts = 0;
        }
    } catch (const std::exception&) {
    }
    if (max_attempts < 1 || max_attempts > 20) {
        return error_response(request_id, "invalid_request",
                              "max attempts must be between 1 and 20");
    }
    std::string command = program;
    for (const auto& argument : arguments) {
        command += " " + argument;
    }
    sqlite3_stmt* statement = nullptr;
    const char* sql =
        "INSERT OR IGNORE INTO restart_tasks"
        "(task_key, policy, max_attempts, log_prefix, command, state,"
        " created_at, updated_at)"
        " VALUES (?1, ?2, ?3, ?4, ?5, 'running', ?6, ?6)";
    if (sqlite3_prepare_v2(database.get(), sql, -1, &statement, nullptr) !=
        SQLITE_OK) {
        throw std::runtime_error(sqlite3_errmsg(database.get()));
    }
    sqlite3_bind_text(statement, 1, key.c_str(), -1, SQLITE_TRANSIENT);
    sqlite3_bind_text(statement, 2, policy.c_str(), -1, SQLITE_TRANSIENT);
    sqlite3_bind_int(statement, 3, static_cast<int>(max_attempts));
    sqlite3_bind_text(statement, 4, log_prefix.c_str(), -1, SQLITE_TRANSIENT);
    sqlite3_bind_text(statement, 5, command.c_str(), -1, SQLITE_TRANSIENT);
    sqlite3_bind_text(statement, 6, now_string().c_str(), -1, SQLITE_TRANSIENT);
    const int step = sqlite3_step(statement);
    sqlite3_finalize(statement);
    if (step != SQLITE_DONE) {
        throw std::runtime_error(sqlite3_errmsg(database.get()));
    }
    const bool created = sqlite3_changes(database.get()) == 1;
    const std::string response =
        restart_status_response(request_id, database, key, created);
    if (created) {
        RestartTask task;
        if (!load_restart_task(database, key, task)) {
            return error_response(request_id, "internal",
                                  "restart task disappeared after insert");
        }
        std::thread([shared_database, state_mutex, task, program, arguments]() {
            run_restart_sequence(shared_database, state_mutex, task, program,
                                 arguments);
        }).detach();
    }
    return response;
}

struct ProcessResourceSample {
    std::vector<int> pids;
    double cpu_percent = 0;
    long long memory_bytes = 0;
};

ProcessResourceSample collect_process_group(int pgid) {
    FILE* pipe = popen("ps -eo pid=,pgid=,pcpu=,rss=", "r");
    if (pipe == nullptr) {
        throw std::runtime_error("ps failed");
    }
    ProcessResourceSample sample;
    char line[512];
    while (fgets(line, sizeof(line), pipe) != nullptr) {
        std::istringstream fields(line);
        int pid = 0;
        int row_pgid = 0;
        double cpu = 0;
        long long rss_kib = 0;
        if (!(fields >> pid >> row_pgid >> cpu >> rss_kib) ||
            row_pgid != pgid) {
            continue;
        }
        sample.pids.push_back(pid);
        sample.cpu_percent += cpu;
        sample.memory_bytes += rss_kib * 1024;
    }
    if (pclose(pipe) != 0) {
        throw std::runtime_error("ps failed");
    }
    return sample;
}

std::string gpu_resource_json(const ProcessResourceSample& sample) {
    FILE* devices_pipe = popen(
        "nvidia-smi --query-gpu=index,name,utilization.gpu,memory.used "
        "--format=csv,noheader,nounits 2>/dev/null",
        "r");
    if (devices_pipe == nullptr) {
        return R"({"supported":false,"reason":"nvidia-smi unavailable","devices":[],"processes":[],"attributed_to_task":false})";
    }
    std::vector<std::string> devices;
    char line[512];
    while (fgets(line, sizeof(line), devices_pipe) != nullptr) {
        std::string value(line);
        while (!value.empty() &&
               (value.back() == '\n' || value.back() == '\r')) {
            value.pop_back();
        }
        if (!value.empty()) {
            devices.push_back(value);
        }
    }
    const int devices_status = pclose(devices_pipe);
    if (devices_status != 0 || devices.empty()) {
        return R"({"supported":false,"reason":"nvidia-smi unavailable or no supported GPU","devices":[],"processes":[],"attributed_to_task":false})";
    }
    FILE* processes_pipe = popen(
        "nvidia-smi --query-compute-apps=pid,used_memory "
        "--format=csv,noheader,nounits 2>/dev/null",
        "r");
    std::vector<std::string> processes;
    bool attributed = false;
    if (processes_pipe != nullptr) {
        while (fgets(line, sizeof(line), processes_pipe) != nullptr) {
            std::string value(line);
            while (!value.empty() &&
                   (value.back() == '\n' || value.back() == '\r')) {
                value.pop_back();
            }
            if (value.empty()) {
                continue;
            }
            processes.push_back(value);
            std::istringstream fields(value);
            int pid = 0;
            if (fields >> pid) {
                attributed = attributed ||
                             std::find(sample.pids.begin(), sample.pids.end(),
                                       pid) != sample.pids.end();
            }
        }
        (void)pclose(processes_pipe);
    }
    const auto array_json = [](const std::vector<std::string>& values) {
        std::string result;
        for (size_t index = 0; index < values.size(); ++index) {
            if (index != 0) {
                result += ',';
            }
            result += '"' + json_escape(values[index]) + '"';
        }
        return result;
    };
    return R"({"supported":true,"devices":[)" + array_json(devices) +
           R"(],"processes":[)" + array_json(processes) +
           R"(],"attributed_to_task":)" + (attributed ? "true" : "false") +
           R"(,"attribution_method":"process_pid_in_task_group"})";
}

std::string task_resource_response(Database& database,
                                   const std::string& request_id,
                                   const std::string& task_id) {
    Task task;
    if (!load_task(database.get(), "id", task_id, task)) {
        return error_response(request_id, "not_found",
                              "task not found: " + task_id);
    }
    if (task.state != "running" || !task_alive(task)) {
        return error_response(request_id, "invalid_request",
                              "task is not running: " + task_id);
    }
    const auto sample = collect_process_group(task.pgid);
    std::string pids;
    for (size_t index = 0; index < sample.pids.size(); ++index) {
        if (index != 0) {
            pids += ',';
        }
        pids += std::to_string(sample.pids[index]);
    }
    return R"({"ok":true,"request_id":")" + json_escape(request_id) +
           R"(","task_id":")" + json_escape(task_id) +
           R"(","process_count":)" + std::to_string(sample.pids.size()) +
           R"(,"pids":[)" + pids +
           R"(],"cpu_percent":)" + std::to_string(sample.cpu_percent) +
           R"(,"cpu_convention":"single_core_percent_sum","memory_bytes":)" +
           std::to_string(sample.memory_bytes) +
           R"(,"memory_method":"sum_rss_shared_pages_may_be_double_counted","gpu":)" +
           gpu_resource_json(sample) + "}";
}

std::string task_resource_samples(Database& database,
                                  const std::string& request_id,
                                  const std::string& task_id,
                                  const std::string& interval_text,
                                  const std::string& count_text) {
    long long interval = 0;
    long long count = 0;
    try {
        size_t consumed = 0;
        interval = std::stoll(interval_text, &consumed);
        if (consumed != interval_text.size()) {
            interval = 0;
        }
        consumed = 0;
        count = std::stoll(count_text, &consumed);
        if (consumed != count_text.size()) {
            count = 0;
        }
    } catch (const std::exception&) {
    }
    if (interval < 100 || interval > 5000 || count < 1 || count > 10) {
        return error_response(request_id, "invalid_request",
                              "invalid resource sample interval or count");
    }
    Task task;
    if (!load_task(database.get(), "id", task_id, task)) {
        return error_response(request_id, "not_found",
                              "task not found: " + task_id);
    }
    if (task.state != "running" || !task_alive(task)) {
        return error_response(request_id, "invalid_request",
                              "task is not running: " + task_id);
    }
    std::string samples;
    std::string overhead;
    for (long long index = 0; index < count; ++index) {
        if (index > 0) {
            std::this_thread::sleep_for(std::chrono::milliseconds(interval));
        }
        const auto started = std::chrono::steady_clock::now();
        const auto sample = collect_process_group(task.pgid);
        const auto elapsed =
            std::chrono::duration_cast<std::chrono::milliseconds>(
                std::chrono::steady_clock::now() - started)
                .count();
        std::string pids;
        for (size_t pid_index = 0; pid_index < sample.pids.size(); ++pid_index) {
            if (pid_index != 0) {
                pids += ',';
            }
            pids += std::to_string(sample.pids[pid_index]);
        }
        if (index != 0) {
            samples += ',';
            overhead += ',';
        }
        samples += R"({"process_count":)" +
                   std::to_string(sample.pids.size()) + R"(,"pids":[)" +
                   pids + R"(],"cpu_percent":)" +
                   std::to_string(sample.cpu_percent) +
                   R"(,"memory_bytes":)" +
                   std::to_string(sample.memory_bytes) + "}";
        overhead += std::to_string(elapsed);
    }
    return R"({"ok":true,"request_id":")" + json_escape(request_id) +
           R"(","task_id":")" + json_escape(task_id) +
           R"(","interval_ms":)" + std::to_string(interval) +
           R"(,"sample_count":)" + std::to_string(count) +
           R"(,"samples":[)" + samples + R"(],"overhead_ms":[)" + overhead +
           "]}";
}

std::string handle_request(Database& database,
                           const std::shared_ptr<Database>& shared_database,
                           const std::shared_ptr<std::mutex>& state_mutex,
                           const std::shared_ptr<LeaseManager>& leases,
                           SessionManager& sessions,
                           const std::string& line) {
    std::vector<std::string> fields;
    std::istringstream stream(line);
    std::string field;
    while (std::getline(stream, field, '\t')) {
        fields.push_back(field);
    }
    const std::string request_id =
        fields.size() > 1 ? fields[1] : "unknown";
    try {
        if (fields.size() >= 5 && fields[0] == "START") {
            std::vector<std::string> arguments(fields.begin() + 5, fields.end());
            auto [task, created] = start_task(
                database, shared_database, state_mutex, fields[2], fields[3],
                fields[4], arguments);
            return task_response(request_id, task, created ? "true" : "false");
        }
        if (fields.size() >= 6 && fields[0] == "TIMEOUT_START") {
            std::vector<std::string> arguments(fields.begin() + 6, fields.end());
            auto [task, created] = start_task_with_timeout(
                database, shared_database, state_mutex, fields[2], fields[3],
                fields[4], fields[5], arguments);
            return task_response(request_id, task, created ? "true" : "false");
        }
        if (fields.size() >= 7 && fields[0] == "RESTART_START") {
            std::vector<std::string> arguments(fields.begin() + 7, fields.end());
            return start_restart_task(
                database, shared_database, state_mutex, request_id, fields[2],
                fields[3], fields[4], fields[5], fields[6], arguments);
        }
        if (fields.size() == 3 && fields[0] == "RESTART_STATUS") {
            return restart_status_response(request_id, database, fields[2]);
        }
        if (fields.size() == 3 && fields[0] == "STATUS") {
            Task task;
            if (!load_task(database.get(), "id", fields[2], task)) {
                throw RequestError("not_found", "task not found: " + fields[2]);
            }
            if (task.state == "running" && !task_alive(task)) {
                update_state(database.get(), task.id, "exited");
                task.state = "exited";
            }
            return task_response(request_id, task);
        }
        if (fields.size() == 3 && fields[0] == "STOP") {
            return task_response(request_id, stop_task(database, fields[2]));
        }
        if (fields.size() == 3 && fields[0] == "TASK_HISTORY") {
            return task_history_response(database, request_id, fields[2]);
        }
        if (fields.size() == 4 && fields[0] == "TASK_SIGNAL") {
            return signal_task_response(
                database, request_id, fields[2], fields[3]);
        }
        if (fields.size() == 3 && fields[0] == "TASK_RESOURCE") {
            return task_resource_response(database, request_id, fields[2]);
        }
        if (fields.size() == 5 && fields[0] == "TASK_RESOURCE_SAMPLE") {
            return task_resource_samples(
                database, request_id, fields[2], fields[3], fields[4]);
        }
        if (fields.size() == 4 && fields[0] == "LEASE_CLAIM") {
            return leases->claim(request_id, fields[2], fields[3]);
        }
        if (fields.size() == 4 && fields[0] == "LEASE_TOUCH") {
            return leases->touch(request_id, fields[2], fields[3]);
        }
        if (fields.size() == 4 && fields[0] == "LEASE_RELEASE") {
            return leases->release(request_id, fields[2], fields[3]);
        }
        if (fields.size() == 3 && fields[0] == "LEASE_STATUS") {
            return leases->status(request_id, fields[2]);
        }
        if (fields.size() == 4 && fields[0] == "LEASE_TAKEOVER") {
            return leases->takeover(request_id, fields[2], fields[3]);
        }
        if (fields.size() >= 5 && fields[0] == "SESSION_START") {
            std::vector<std::string> arguments(fields.begin() + 5, fields.end());
            return sessions.start(
                request_id, fields[2], fields[3], fields[4], arguments);
        }
        if (fields.size() == 4 && fields[0] == "SESSION_ATTACH") {
            return sessions.attach(request_id, fields[2], fields[3]);
        }
        if (fields.size() == 4 && fields[0] == "SESSION_DETACH") {
            return sessions.detach(request_id, fields[2], fields[3]);
        }
        if (fields.size() == 5 && fields[0] == "SESSION_INPUT") {
            return sessions.input(request_id, fields[2], fields[3], fields[4]);
        }
        if (fields.size() == 3 && fields[0] == "SESSION_STATUS") {
            return sessions.status(request_id, fields[2]);
        }
        throw RequestError("invalid_request", "unsupported request");
    } catch (const RequestError& error) {
        return error_response(request_id, error.code, error.what());
    } catch (const std::exception& error) {
        return error_response(request_id, "internal", error.what());
    }
}

std::string read_line(int descriptor) {
    std::string result;
    char character;
    while (true) {
        const ssize_t count = read(descriptor, &character, 1);
        if (count <= 0 || character == '\n') {
            break;
        }
        result += character;
    }
    return result;
}

}  // namespace

int run_runtime_daemon(const std::string& socket_path,
                       const std::string& database_path) {
    try {
        signal(SIGPIPE, SIG_IGN);
        const auto database = std::make_shared<Database>(database_path);
        recover(*database);
        const auto leases = std::make_shared<LeaseManager>();
        SessionManager sessions(leases);
        unlink(socket_path.c_str());

        const int server = socket(AF_UNIX, SOCK_STREAM, 0);
        if (server < 0) {
            throw std::runtime_error(std::strerror(errno));
        }
        sockaddr_un address{};
        address.sun_family = AF_UNIX;
        if (socket_path.size() >= sizeof(address.sun_path)) {
            close(server);
            throw std::runtime_error("socket path too long");
        }
        std::strncpy(address.sun_path, socket_path.c_str(),
                     sizeof(address.sun_path) - 1);
        if (bind(server, reinterpret_cast<sockaddr*>(&address), sizeof(address)) !=
                0 ||
            listen(server, 16) != 0) {
            close(server);
            throw std::runtime_error(std::strerror(errno));
        }

        const auto state_mutex = std::make_shared<std::mutex>();
        while (true) {
            const int client = accept(server, nullptr, nullptr);
            if (client < 0) {
                close(server);
                throw std::runtime_error(std::strerror(errno));
            }
            std::thread([client, database, state_mutex, leases, &sessions]() {
                const std::string request = read_line(client);
                std::string response;
                {
                    const std::lock_guard<std::mutex> lock(*state_mutex);
                    response = handle_request(
                        *database, database, state_mutex, leases, sessions,
                        request);
                }
                const std::string output = response + "\n";
                (void)write(client, output.data(), output.size());
                close(client);
            }).detach();
        }
    } catch (const std::exception& error) {
        std::cerr << "daemon failed: " << error.what() << '\n';
        return 1;
    }
}

int run_runtime_client(const std::string& operation,
                       const std::vector<std::string>& arguments) {
    const bool lease_mutation =
        operation == "LEASE_CLAIM" || operation == "LEASE_TOUCH" ||
        operation == "LEASE_RELEASE" || operation == "LEASE_TAKEOVER";
    const bool variable_arguments =
        operation == "START" || operation == "TIMEOUT_START" ||
        operation == "RESTART_START" ||
        operation == "SESSION_START";
    const size_t minimum = variable_arguments
                               ? (operation == "TIMEOUT_START"
                                      ? 5
                                      : (operation == "RESTART_START" ? 6 : 4))
                               : (operation == "SESSION_INPUT"
                                      ? 4
                                      : ((operation == "SESSION_ATTACH" ||
                                          operation == "SESSION_DETACH" ||
                                          operation == "TASK_SIGNAL" ||
                                          operation == "TASK_RESOURCE_SAMPLE" ||
                                          lease_mutation)
                                             ? 3
                                             : 2));
    if (arguments.size() < minimum ||
        (!variable_arguments && arguments.size() != minimum)) {
        std::cerr << "invalid runtime client arguments\n";
        return 2;
    }
    std::vector<std::string> fields = {operation, "request-" + now_string()};
    fields.insert(fields.end(), arguments.begin() + 1, arguments.end());
    std::string request;
    for (size_t index = 0; index < fields.size(); ++index) {
        if (index != 0) {
            request += '\t';
        }
        request += fields[index];
    }
    request += '\n';

    const int connection = socket(AF_UNIX, SOCK_STREAM, 0);
    if (connection < 0) {
        std::cerr << "socket failed: " << std::strerror(errno) << '\n';
        return 1;
    }
    sockaddr_un address{};
    address.sun_family = AF_UNIX;
    if (arguments[0].size() >= sizeof(address.sun_path)) {
        close(connection);
        std::cerr << "socket path too long\n";
        return 1;
    }
    std::strncpy(address.sun_path, arguments[0].c_str(),
                 sizeof(address.sun_path) - 1);
    if (connect(connection, reinterpret_cast<sockaddr*>(&address),
                sizeof(address)) != 0) {
        close(connection);
        std::cerr << "IPC failed: " << std::strerror(errno) << '\n';
        return 1;
    }
    timeval timeout{};
    timeout.tv_sec = 1;
    setsockopt(connection, SOL_SOCKET, SO_RCVTIMEO, &timeout, sizeof(timeout));
    setsockopt(connection, SOL_SOCKET, SO_SNDTIMEO, &timeout, sizeof(timeout));
    (void)write(connection, request.data(), request.size());
    const std::string response = read_line(connection);
    close(connection);
    if (response.empty()) {
        std::cerr << "IPC response timed out\n";
        return 1;
    }
    std::cout << response << '\n';
    return 0;
}

int run_dashboard(const std::string& database_path) {
    sqlite3* database = nullptr;
    if (sqlite3_open_v2(database_path.c_str(), &database, SQLITE_OPEN_READONLY,
                        nullptr) != SQLITE_OK) {
        const std::string message =
            database == nullptr ? "sqlite open failed" : sqlite3_errmsg(database);
        std::cerr << "dashboard failed: " << message << '\n';
        sqlite3_close(database);
        return 1;
    }

    sqlite3_stmt* states_statement = nullptr;
    if (sqlite3_prepare_v2(
            database,
            "SELECT state, COUNT(*) FROM tasks GROUP BY state ORDER BY state",
            -1, &states_statement, nullptr) != SQLITE_OK) {
        std::cerr << "dashboard failed: " << sqlite3_errmsg(database) << '\n';
        sqlite3_close(database);
        return 1;
    }
    std::vector<std::string> states;
    int task_count = 0;
    while (sqlite3_step(states_statement) == SQLITE_ROW) {
        const auto* state_text = sqlite3_column_text(states_statement, 0);
        const std::string state = state_text == nullptr
                                      ? ""
                                      : reinterpret_cast<const char*>(state_text);
        const int count = sqlite3_column_int(states_statement, 1);
        states.push_back(state + "=" + std::to_string(count));
        task_count += count;
    }
    sqlite3_finalize(states_statement);
    if (states.empty()) {
        states.push_back("none");
    }

    sqlite3_stmt* latest_statement = nullptr;
    std::string latest = "latest: none";
    if (sqlite3_prepare_v2(
            database,
            "SELECT id, idempotency_key, state FROM tasks "
            "ORDER BY updated_at DESC LIMIT 1",
            -1, &latest_statement, nullptr) == SQLITE_OK &&
        sqlite3_step(latest_statement) == SQLITE_ROW) {
        const auto text = [latest_statement](int column) {
            const auto* value = sqlite3_column_text(latest_statement, column);
            return value == nullptr
                       ? std::string{}
                       : reinterpret_cast<const char*>(value);
        };
        latest = "latest: id=" + text(0) + " key=" + text(1) +
                 " state=" + text(2);
    }
    sqlite3_finalize(latest_statement);
    sqlite3_close(database);

    std::string states_text;
    for (const auto& state : states) {
        if (!states_text.empty()) {
            states_text += ' ';
        }
        states_text += state;
    }
    std::cout << "\x1b[?1049h\x1b[H\x1b[2JQWE POC DASHBOARD\r\n"
              << "database: " << database_path << "\r\n"
              << "tasks=" << task_count << ' ' << states_text << "\r\n"
              << latest << "\r\n\r\n"
              << "[q] quit (one-shot POC screen)\x1b[?1049l";
    return 0;
}
