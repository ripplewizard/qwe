cmake_minimum_required(VERSION 3.20)

project(qwe_poc LANGUAGES CXX)

find_package(SQLite3 REQUIRED)
find_package(PkgConfig REQUIRED)
pkg_check_modules(VTERM REQUIRED IMPORTED_TARGET vterm)

add_executable(qwe-poc src/main.cpp src/runtime.cpp)
target_compile_features(qwe-poc PRIVATE cxx_std_20)
if(TARGET SQLite3::SQLite3)
  target_link_libraries(qwe-poc PRIVATE SQLite3::SQLite3)
else()
  target_link_libraries(qwe-poc PRIVATE SQLite::SQLite3)
endif()
target_link_libraries(qwe-poc PRIVATE PkgConfig::VTERM)

if(CMAKE_CXX_COMPILER_ID MATCHES "Clang|GNU")
  target_compile_options(qwe-poc PRIVATE -Wall -Wextra -Wpedantic -Werror)
endif()

