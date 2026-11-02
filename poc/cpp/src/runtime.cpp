#include "runtime.hpp"

#include <sqlite3.h>

#include <cerrno>
#include <chrono>
#include <csignal>
#include <cstdio>
#include <cstring>
#include <fcntl.h>
#include <fstream>
#include <iostream>
#include <memory>
#include <mutex>
#include <sstream>
#include <stdexcept>
#include <string_view>
#include <sys/socket.h>
#include <sys/time.h>
#include <sys/un.h>
#include <sys/wait.h>
#include <thread>
#include <unistd.h>

namespace {

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
            ");");
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

std::pair<Task, bool> start_task(Database& database, const std::string& key,
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
    std::thread([child]() {
        int status = 0;
        (void)waitpid(child, &status, 0);
    }).detach();

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
        throw std::runtime_error(sqlite3_errmsg(database.get()));
    }
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

std::string handle_request(Database& database, const std::string& line) {
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
                database, fields[2], fields[3], fields[4], arguments);
            return task_response(request_id, task, created ? "true" : "false");
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
        Database database(database_path);
        recover(database);
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

        std::mutex database_mutex;
        while (true) {
            const int client = accept(server, nullptr, nullptr);
            if (client < 0) {
                close(server);
                throw std::runtime_error(std::strerror(errno));
            }
            std::thread([client, &database, &database_mutex]() {
                const std::string request = read_line(client);
                std::string response;
                {
                    const std::lock_guard<std::mutex> lock(database_mutex);
                    response = handle_request(database, request);
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
    const size_t minimum = operation == "START" ? 4 : 2;
    if (arguments.size() < minimum ||
        (operation != "START" && arguments.size() != 2)) {
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
