#pragma once

#ifdef __cplusplus
extern "C" {
#endif

typedef struct {
    const char* crl_path;
    const char* kbm_path;

    const char* ICE_offer;
    const char* join_code;
    bool WEBRTC_connection_failed;

    int result_err; // If not 0, error

    void* opaque_internal_ctx;
} gamescopeWebrtcCtx;



gamescopeWebrtcCtx* gamescopeWebrtc_INIT(bool, bool);

void gamescopeWebrtc_create_webrtc(gamescopeWebrtcCtx*, int, bool, char*);
void gamescopeWebrtc_check_webrtc(gamescopeWebrtcCtx*);
void gamescopeWebrtc_start_recording(gamescopeWebrtcCtx*, int);

#ifdef __cplusplus
}
#endif
