#pragma once


#include "main.hpp"


#include <nlohmann/json.hpp>
#include "recording.hpp"

int rtp_avio_write(void *opaque, const uint8_t *buf, int buf_size);
void setup_RTC(stateData *data, bool create_code, std::string url_base);