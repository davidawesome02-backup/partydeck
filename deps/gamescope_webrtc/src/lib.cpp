#include "lib.h"
#include "main.hpp"

static gamescopeWebrtcCtx* int_gamescopeWebrtc_INIT(bool kbm, bool ctrl) {
    gamescopeWebrtcCtx* allocated_ctx = (gamescopeWebrtcCtx*) calloc(1, sizeof(gamescopeWebrtcCtx));
    // stateData* internal_ctx = (stateData*) calloc(1, sizeof(stateData));
    stateData* internal_ctx = new stateData();
    allocated_ctx->opaque_internal_ctx = (void*) internal_ctx;

    allocated_ctx->result_err = 0;

    allocated_ctx->kbm_path = setup_uinput_keyboard_mouse(internal_ctx).c_str();
    // std::string ctrl_path = setup_uinput_controler(internal_ctx);


    return allocated_ctx;
}

static void int_gamescopeWebrtc_create_webrtc(gamescopeWebrtcCtx* allocated_ctx, int fps, bool request_code, char* URL) {
    stateData* internal_ctx = (stateData*)allocated_ctx->opaque_internal_ctx;
    internal_ctx->fps = fps;

    std::string URL_str = URL;

    setup_RTC(internal_ctx, request_code, URL_str);
}

static void int_gamescopeWebrtc_check_webrtc(gamescopeWebrtcCtx* allocated_ctx) {
    stateData* internal_ctx = (stateData*)allocated_ctx->opaque_internal_ctx;

    auto rtc_connection_obj = internal_ctx->pc_connection.get();
    if (rtc_connection_obj) {
        auto rtc_connection_state = rtc_connection_obj->state();
        switch (rtc_connection_state) {
            case rtc::PeerConnection::State::New:
            case rtc::PeerConnection::State::Connecting:
            case rtc::PeerConnection::State::Connected:
                break;
            default:
                allocated_ctx->WEBRTC_connection_failed = true;
                std::cerr << "Web rtc died" << std::endl;
                return;
        }
    }

    auto websock_connection_obj = internal_ctx->connection_open_socket.get();
    if (websock_connection_obj) {
        if (websock_connection_obj->isClosed() && internal_ctx->connection_code && internal_ctx->connection_code->empty()) {
            allocated_ctx->WEBRTC_connection_failed = true;
            std::cerr << "Websocket code connection died" << std::endl;
        }
    }

    
    if (internal_ctx->ICE_offer_str && !internal_ctx->ICE_offer_str->empty()) allocated_ctx->ICE_offer = internal_ctx->ICE_offer_str->c_str();
    if (internal_ctx->connection_code && !internal_ctx->connection_code->empty()) allocated_ctx->join_code = internal_ctx->connection_code->c_str();
}

static void int_gamescopeWebrtc_start_recording(gamescopeWebrtcCtx* allocated_ctx, int gamescope_pid) {
    stateData* internal_ctx = (stateData*)allocated_ctx->opaque_internal_ctx;

}



extern "C" gamescopeWebrtcCtx* gamescopeWebrtc_INIT(bool kbm, bool ctrl) {
    return int_gamescopeWebrtc_INIT(kbm, ctrl);
}

extern "C" 
void    gamescopeWebrtc_create_webrtc(gamescopeWebrtcCtx* allocated_ctx, int fps, bool request_code, char* URL) {
    int_gamescopeWebrtc_create_webrtc(allocated_ctx, fps, request_code, URL);
}

extern "C"
void    gamescopeWebrtc_check_webrtc(gamescopeWebrtcCtx* allocated_ctx) {
    int_gamescopeWebrtc_check_webrtc(allocated_ctx);
}

extern "C" 
void    gamescopeWebrtc_start_recording(gamescopeWebrtcCtx* allocated_ctx, int gamescope_pid) {
    int_gamescopeWebrtc_start_recording(allocated_ctx, gamescope_pid);
}
