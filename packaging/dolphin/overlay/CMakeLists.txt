cmake_minimum_required(VERSION 3.16)
project(onedrive-hydration-overlay LANGUAGES CXX VERSION 0.1)

# The one compiled component this product ships. Everything else in packaging/ is
# data (a servicemenu, shell wrappers, a QML plasmoid); an overlay emblem cannot
# be done that way — see docs/DOLPHIN-GROUNDWORK.md — so this is the first place
# a Qt6/KF6 toolchain is required to build. Kept self-contained under
# packaging/dolphin/overlay/ so a distro can build just this subtree.

find_package(ECM REQUIRED NO_MODULE)
list(APPEND CMAKE_MODULE_PATH ${ECM_MODULE_PATH})

# KDEInstallDirs gives KDE_INSTALL_PLUGINDIR (the Qt plugin root, e.g.
# /usr/lib/qt6/plugins); KDECMakeSettings turns on shared libs and the compiler
# settings a KF6 plugin expects. On some ECM versions KDECMakeSettings does not
# set the library output directory kcoreaddons_add_plugin needs, so it is set
# explicitly below (learned building the P2 gate plugin).
include(KDEInstallDirs)
include(KDECMakeSettings)
include(KDECompilerSettings NO_POLICY_SCOPE)

set(BUILD_SHARED_LIBS ON)
if(NOT CMAKE_LIBRARY_OUTPUT_DIRECTORY)
    set(CMAKE_LIBRARY_OUTPUT_DIRECTORY ${CMAKE_BINARY_DIR}/bin)
endif()

set(CMAKE_AUTOMOC ON)

find_package(Qt6 REQUIRED COMPONENTS Core)
find_package(KF6 REQUIRED COMPONENTS KIO)

# kcoreaddons_add_plugin builds the MODULE .so and installs it into
# ${KDE_INSTALL_PLUGINDIR}/kf6/overlayicon, exactly where Dolphin's
# KOverlayIconPlugin loader looks (confirmed: a user-scope ~/.local path is NOT
# searched, so this must land in the system Qt plugin dir).
kcoreaddons_add_plugin(onedrive-hydration-overlay
    SOURCES hydrationoverlay.cpp
    INSTALL_NAMESPACE "kf6/overlayicon")
target_link_libraries(onedrive-hydration-overlay KF6::KIOCore)
