#pragma once

#include <string>
#include <vector>

int run_runtime_daemon(const std::string& socket_path,
                       const std::string& database_path);
int run_runtime_client(const std::string& operation,
                       const std::vector<std::string>& arguments);
int run_dashboard(const std::string& database_path);
