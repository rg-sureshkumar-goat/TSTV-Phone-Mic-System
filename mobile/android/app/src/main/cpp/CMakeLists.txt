# Builds the vendored libopus (same files as the desktop and iPhone apps) plus a small JNI wrapper.
cmake_minimum_required(VERSION 3.22)
project(opusjni C)

set(OPUS "${CMAKE_CURRENT_SOURCE_DIR}/../../../../../../third_party/opus/libopus")
file(GLOB OPUS_SOURCES "${OPUS}/celt/*.c" "${OPUS}/silk/*.c" "${OPUS}/silk/float/*.c" "${OPUS}/src/*.c")

add_library(opusjni SHARED opus_jni.c ${OPUS_SOURCES})
target_include_directories(opusjni PRIVATE "${OPUS}/include" "${OPUS}" "${OPUS}/celt" "${OPUS}/silk" "${OPUS}/silk/float")
target_compile_definitions(opusjni PRIVATE OPUS_BUILD HAVE_LRINTF HAVE_LRINT VAR_ARRAYS)
target_compile_options(opusjni PRIVATE -O2 -w)
target_link_libraries(opusjni m)
