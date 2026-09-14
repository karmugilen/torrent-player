cmake_minimum_required(VERSION 3.22)
project(webtor_transports LANGUAGES C CXX)
get_filename_component(REPO_ROOT "${CMAKE_CURRENT_SOURCE_DIR}/../.." ABSOLUTE)
set(MODULES "${REPO_ROOT}/engine/node_modules")
set(NODE_INCLUDE "${REPO_ROOT}/android/app/src/main/cpp/include")
set(NODE_LIBRARY "${REPO_ROOT}/android/app/src/main/jniLibs/arm64-v8a/libnode.so")
set(CMAKE_POSITION_INDEPENDENT_CODE ON)
set(CMAKE_CXX_STANDARD 17)
add_library(node SHARED IMPORTED)
set_target_properties(node PROPERTIES IMPORTED_LOCATION "${NODE_LIBRARY}")

set(UTP "${MODULES}/utp-native")
add_library(utp_native SHARED
  "${UTP}/binding.cc"
  "${UTP}/deps/libutp/utp_internal.cpp"
  "${UTP}/deps/libutp/utp_utils.cpp"
  "${UTP}/deps/libutp/utp_hash.cpp"
  "${UTP}/deps/libutp/utp_callbacks.cpp"
  "${UTP}/deps/libutp/utp_api.cpp"
  "${UTP}/deps/libutp/utp_packedsockaddr.cpp")
target_include_directories(utp_native PRIVATE "${NODE_INCLUDE}" "${MODULES}/napi-macros")
target_compile_definitions(utp_native PRIVATE POSIX NODE_GYP_MODULE_NAME=utp_native)
target_link_libraries(utp_native PRIVATE node)

# Reuse the exact OpenSSL implementation exported by the bundled Node runtime.
set(OPENSSL_INCLUDE_DIR "${NODE_INCLUDE}" CACHE PATH "" FORCE)
set(OPENSSL_SSL_LIBRARY "${NODE_LIBRARY}" CACHE FILEPATH "" FORCE)
set(OPENSSL_CRYPTO_LIBRARY "${NODE_LIBRARY}" CACHE FILEPATH "" FORCE)
set(NO_MEDIA ON CACHE BOOL "" FORCE)
set(NO_WEBSOCKET ON CACHE BOOL "" FORCE)
set(NO_EXAMPLES ON CACHE BOOL "" FORCE)
set(NO_TESTS ON CACHE BOOL "" FORCE)
set(BUILD_SHARED_LIBS OFF CACHE BOOL "" FORCE)
# Sources and their dependencies are pinned by Git submodules. Configuration
# and compilation must work without downloading anything.
add_subdirectory("${REPO_ROOT}/vendor/libdatachannel" libdatachannel EXCLUDE_FROM_ALL)

set(NDC "${MODULES}/node-datachannel/src/cpp")
add_library(node_datachannel SHARED
  "${NDC}/rtc-wrapper.cpp"
  "${NDC}/data-channel-wrapper.cpp"
  "${NDC}/ice-udp-mux-listener-wrapper.cpp"
  "${NDC}/peer-connection-wrapper.cpp"
  "${NDC}/thread-safe-callback.cpp"
  "${NDC}/main.cpp")
target_include_directories(node_datachannel PRIVATE "${NODE_INCLUDE}" "${MODULES}/node-addon-api")
target_compile_definitions(node_datachannel PRIVATE NAPI_VERSION=8)
target_link_libraries(node_datachannel PRIVATE node datachannel-static plog::plog)
foreach(addon utp_native node_datachannel)
  target_compile_options(${addon} PRIVATE -fvisibility=hidden -fvisibility-inlines-hidden)
  target_link_options(${addon} PRIVATE -Wl,--exclude-libs,ALL)
endforeach()
