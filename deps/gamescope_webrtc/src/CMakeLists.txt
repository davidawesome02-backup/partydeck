cmake_minimum_required(VERSION 3.13)
project(gamescope-webrtc)

set(CMAKE_PREFIX_PATH ${CMAKE_PREFIX_PATH} /home/david/local/libdatachannel-install)
set(CMAKE_PREFIX_PATH ${CMAKE_PREFIX_PATH} /home/david/git/gamescope-webrtc/deps)
set(CMAKE_PREFIX_PATH ${CMAKE_PREFIX_PATH} /home/david/git/gamescope-webrtc/deps/install)
set(CMAKE_PREFIX_PATH ${CMAKE_PREFIX_PATH} /home/david/git/gamescope-webrtc/deps/libs)



find_package(PkgConfig REQUIRED) 
pkg_check_modules(FFMPEG REQUIRED libavformat libavcodec libavutil libswscale ) # Include directories from pkg-config 
include_directories(${FFMPEG_INCLUDE_DIRS}) 
link_directories(${FFMPEG_LIBRARY_DIRS})


# PipeWire (the 0.3 version is most widespread now)
pkg_check_modules(PIPEWIRE REQUIRED libpipewire-0.3)
include_directories(${PIPEWIRE_INCLUDE_DIRS})
link_directories(${PIPEWIRE_LIBRARY_DIRS})


# add_executable(gamescope-webrtc main.cpp uinput_helper.cpp recording.cpp webrtc.cpp)


# set_target_properties(gamescope-webrtc PROPERTIES
#     CXX_STANDARD 17
#     OUTPUT_NAME gamescope-webrtc)


#set_target_properties(gamescope-webrtc PROPERTIES
#	XCODE_ATTRIBUTE_PRODUCT_BUNDLE_IDENTIFIER com.github.paullouisageneau.libdatachannel.examples.media-sender)

find_package(Threads REQUIRED)
find_package(LibDataChannel REQUIRED)
# target_link_libraries(gamescope-webrtc LibDataChannel::LibDataChannel Threads::Threads ${FFMPEG_LIBRARIES} ${PIPEWIRE_LIBRARIES})

# # Add include dirs for the project (if you keep a local include folder) 
# target_include_directories(gamescope-webrtc PRIVATE . ${FFMPEG_INCLUDE_DIRS} ${PIPEWIRE_INCLUDE_DIRS}) 

# # Propagate pkg-config cflags to the target (safe) 
# target_compile_options(gamescope-webrtc PRIVATE ${FFMPEG_CFLAGS_OTHER} ${PIPEWIRE_CFLAGS_OTHER})



add_library(gamescope-webrtc-lib lib.cpp uinput_helper.cpp recording.cpp webrtc.cpp)

set_target_properties(gamescope-webrtc-lib PROPERTIES
    CXX_STANDARD 17
    OUTPUT_NAME gamescope-webrtc-lib
    CXX_VISIBILITY_PRESET hidden
    VISIBILITY_INLINES_HIDDEN YES
    POSITION_INDEPENDENT_CODE ON
)

target_link_libraries(gamescope-webrtc-lib LibDataChannel::LibDataChannel Threads::Threads ${FFMPEG_LIBRARIES} ${PIPEWIRE_LIBRARIES})

# Add include dirs for the project (if you keep a local include folder) 
target_include_directories(gamescope-webrtc-lib PRIVATE . ${FFMPEG_INCLUDE_DIRS} ${PIPEWIRE_INCLUDE_DIRS}) 

# Propagate pkg-config cflags to the target (safe) 
target_compile_options(gamescope-webrtc-lib PRIVATE ${FFMPEG_CFLAGS_OTHER} ${PIPEWIRE_CFLAGS_OTHER})

target_include_directories(gamescope-webrtc-lib PUBLIC ./lib.h) # Define external libs.

install(TARGETS gamescope-webrtc-lib)