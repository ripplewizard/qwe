cmake_minimum_required(VERSION 3.20)

project(qwe_poc LANGUAGES CXX)

add_executable(qwe-poc src/main.cpp)
target_compile_features(qwe-poc PRIVATE cxx_std_20)

if(CMAKE_CXX_COMPILER_ID MATCHES "Clang|GNU")
  target_compile_options(qwe-poc PRIVATE -Wall -Wextra -Wpedantic -Werror)
endif()

