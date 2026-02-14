// #include <uinput_helper.cpp>
#pragma once

#include "main.hpp" // Not needed for compile, but vscode wants it for regocnition

std::string setup_uinput_keyboard_mouse(stateData*) ;

void process_remote_message(stateData*, std::vector<std::byte>);