cmake_minimum_required(VERSION 3.21)
project(omalook_compose_spike LANGUAGES CXX)
set(CMAKE_CXX_STANDARD 20)
set(CMAKE_AUTOMOC ON)
find_package(Qt6 REQUIRED COMPONENTS Core Gui Qml Quick)
qt_policy(SET QTP0001 NEW)
qt_add_qml_module(compose_spike
    URI OmaLook.Compose
    VERSION 1.0
    PLUGIN_TARGET compose_spike_plugin
    OUTPUT_DIRECTORY ${CMAKE_BINARY_DIR}/qml/OmaLook/Compose
    SOURCES documenthandler.h documenthandler.cpp)
target_link_libraries(compose_spike PRIVATE Qt6::Core Qt6::Gui Qt6::Qml Qt6::Quick)
