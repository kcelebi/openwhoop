cmake_minimum_required(VERSION 3.13)

set(CMAKE_BUILD_TYPE Release)

include(${PICO_SDK_PATH}/pico_sdk_init.cmake)
project(pico_w C CXX)

pico_sdk_init()

add_executable(pico_w
    main.c
)

target_compile_options(pico_w PRIVATE -Wall -Wextra -O2)
target_compile_definitions(pico_w PRIVATE PICO_W)
target_include_directories(pico_w PRIVATE ${CMAKE_SOURCE_DIR})
target_link_libraries(pico_w pico_cyw43_arch_lwip_threadsafe_background pico_status_led hardware_timer)
target_link_libraries(pico_w pico_runtime)
target_link_options(pico_w PRIVATE -lrdimon)
