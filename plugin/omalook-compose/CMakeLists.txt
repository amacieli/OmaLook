cmake_minimum_required(VERSION 3.21)
project(omalook_compose LANGUAGES CXX)
set(CMAKE_CXX_STANDARD 20)
set(CMAKE_AUTOMOC ON)
find_package(Qt6 REQUIRED COMPONENTS Core Gui Qml Quick)
qt_policy(SET QTP0001 NEW)
# Output layout: <build>/qml/OmaLook/Compose/{qmldir,libomalook_compose.so,...};
# point QML_IMPORT_PATH at <build>/qml.
qt_add_qml_module(omalook_compose
    URI OmaLook.Compose
    VERSION 1.0
    PLUGIN_TARGET omalook_compose_plugin
    OUTPUT_DIRECTORY ${CMAKE_BINARY_DIR}/qml/OmaLook/Compose
    SOURCES documenthandler.h documenthandler.cpp)
target_link_libraries(omalook_compose PRIVATE Qt6::Core Qt6::Gui Qt6::Qml Qt6::Quick)
