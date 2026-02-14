

#include "recording.hpp"



#define PW_THROW_IF(cond, msg) if (cond) throw std::runtime_error(msg);


static bool setup_libav_buffers(stateData *data) {
    printf("REALLOCING BUFFERS\n");
    int width = data->width;
    int height = data->height;
    int streamWidth = data->streamWidth;
    int streamHeight = data->streamHeight;

    int ret = 0;

    data->latest_frame = av_frame_alloc();
    data->latest_frame->format = AV_PIX_FMT_BGR0;
    data->latest_frame->width  = width;
    data->latest_frame->height = height;
    ret = av_frame_get_buffer(data->latest_frame, 32);
    PW_THROW_IF(ret < 0, "Failed to get latest_frame frame buffer");

    data->enc_frame = av_frame_alloc();
    data->enc_frame->format = AV_PIX_FMT_YUV420P;
    data->enc_frame->width = streamWidth;
    data->enc_frame->height = streamHeight;
    ret = av_frame_get_buffer(data->enc_frame, 32);
    PW_THROW_IF(ret < 0, "Failed to get enc_frame frame buffer");
    

    data->encPkt = av_packet_alloc();



    int TARGET_FPS = data->fps; // DOSNT MATTER AT ALL


    const AVCodec *encoder = avcodec_find_encoder(AV_CODEC_ID_H264);
    data->encCtx = avcodec_alloc_context3(encoder);
    data->encCtx->width = streamWidth;
    data->encCtx->height = streamHeight;
    data->encCtx->pix_fmt = AV_PIX_FMT_YUV420P;
    data->encCtx->time_base = AVRational{1, TARGET_FPS};
    data->encCtx->framerate = AVRational{TARGET_FPS, 1};
    data->encCtx->gop_size = TARGET_FPS;
    data->encCtx->max_b_frames = 0;


    // new
    data->encCtx->has_b_frames = 0;
    data->encCtx->flags |= AV_CODEC_FLAG_LOW_DELAY;

    AVDictionary *codec_opts = NULL;
    av_dict_set(&codec_opts, "preset", "ultrafast", 0);
    av_dict_set(&codec_opts, "tune", "zerolatency", 0);


    ret = avcodec_open2(data->encCtx, encoder, &codec_opts);
    av_dict_free(&codec_opts);

    PW_THROW_IF(ret < 0, "avcodec_open2 failed for encoder");

    // Create swscale conversion context (RGBA -> YUV420P)
    data->sws = sws_getContext(
        width, height, AV_PIX_FMT_BGR0,
        streamWidth, streamHeight, AV_PIX_FMT_YUV420P,
        SWS_FAST_BILINEAR, nullptr, nullptr, nullptr
    );
    PW_THROW_IF(!data->sws, "sws_getContext failed");

    // RTP FFmpeg output setup (same as your code)
    data->rtpFmt = nullptr;
    ret = avformat_alloc_output_context2(&data->rtpFmt, nullptr, "rtp", nullptr);
    PW_THROW_IF(ret < 0 || !data->rtpFmt, "avformat_alloc_output_context2 failed");

    AVStream *rtpStream = avformat_new_stream(data->rtpFmt, nullptr);
    PW_THROW_IF(!rtpStream, "avformat_new_stream failed");
    avcodec_parameters_from_context(rtpStream->codecpar, data->encCtx);
    rtpStream->time_base = data->encCtx->time_base;

    int avio_buf_size = RTP_MTU + 256;
    uint8_t *avioBuffer = (uint8_t *)av_malloc(avio_buf_size);

    AVIOContext *avio = avio_alloc_context(
        avioBuffer, avio_buf_size, 1, data->track.get(), nullptr, rtp_avio_write, nullptr);

    data->rtpFmt->pb = avio;
    data->rtpFmt->packet_size = RTP_MTU;
    data->rtpFmt->flags |= AVFMT_FLAG_CUSTOM_IO;

    AVDictionary *opts = nullptr;
    av_dict_set(&opts, "payload_type", "96", 0);
    av_dict_set(&opts, "ssrc", std::to_string(SSRC).c_str(), 0);
    ret = avformat_write_header(data->rtpFmt, &opts);
    av_dict_free(&opts);
    PW_THROW_IF(ret < 0, "avformat_write_header failed");

    printf("ALL BUFFERS ALLOCATED\n");


    return true;
}



static void send_latest_data(stateData *data) {
    // Convert RGBA -> YUV420P in-place
    sws_scale(
        data->sws,
        data->latest_frame->data, data->latest_frame->linesize, 0, data->height,
        data->enc_frame->data, data->enc_frame->linesize
    );
    data->enc_frame->pts = data->frame_pts++;

#if send_instantly
    int64_t now_us = av_gettime();
    data->enc_frame->pts =
        av_rescale_q(now_us, AVRational{1,1000000}, data->encCtx->time_base);

#endif

    // Encode and stream as before:
    avcodec_send_frame(data->encCtx, data->enc_frame);
    while (avcodec_receive_packet(data->encCtx, data->encPkt) == 0) {
        // printf("Sending frame remote\n");
        try {
            if (data->encPkt) // !just_cleared && 
                av_write_frame(data->rtpFmt, data->encPkt);
        
            if (data->rtpFmt && data->rtpFmt->pb)
                avio_flush(data->rtpFmt->pb); // force immediate write to AVIO callback
            av_packet_unref(data->encPkt);
        }
        catch (const std::runtime_error& e) {
            // Catch the specific exception type
            std::cerr << "Caught a runtime error: " << e.what() << std::endl;
        }
    }
}

static void send_frame_timer(void *userdata, long unsigned int a) {
    stateData *data = (stateData *)userdata;

    // Call on_process to send a frame (this could be a duplicate frame or an empty frame if no new content)
    if (data->latest_frame)
        send_latest_data(data);  // This sends a frame, whether new or not
}



static void on_param_changed(void *userdata, uint32_t id, const struct spa_pod *param) {


        stateData *data = (stateData *)userdata;


        printf("Format requested!\n");
        if (param == NULL || id != SPA_PARAM_Format) {
                printf("\tRETURNA, %d  %p\n", id, param);
                return;
        }

        if (spa_format_parse(param,
                        &data->format.media_type,
                        &data->format.media_subtype) < 0) {
                printf("\tRETURNB\n");
                return;
        }

        if (data->format.media_type != SPA_MEDIA_TYPE_video ||
            data->format.media_subtype != SPA_MEDIA_SUBTYPE_raw) {
                printf("\tRETURNC\n");
                return;
        }

        if (spa_format_video_raw_parse(param, &data->format.info.raw) < 0) {
                printf("\tRETURND\n");
                return;
        }

        printf("got video format:\n");
        printf("  format: %d (%s)\n", data->format.info.raw.format,
                        spa_debug_type_find_name(spa_type_video_format,
                                data->format.info.raw.format));
        printf("  size: %dx%d\n", data->format.info.raw.size.width,
                        data->format.info.raw.size.height);

        int prescaled_height = data->format.info.raw.size.height&(~1); // Make even line count, rounds down to prevent errors in h264 
        int prescaled_width = data->format.info.raw.size.width&(~1); // Make even line count, rounds down to prevent errors in h264 
        data->real_width = data->format.info.raw.size.width;
        data->height = prescaled_height;
        data->width = prescaled_width;
        data->streamHeight = prescaled_height;
        data->streamWidth = prescaled_width;
                    
        printf("  framerate: %d/%d\n", data->format.info.raw.framerate.num,
                        data->format.info.raw.framerate.denom);

}


/* [on_process] */
static void on_process(void *userdata) {
        stateData *data = (stateData *) userdata;
        
        struct pw_buffer *b;

        if ((b = pw_stream_dequeue_buffer(data->stream)) == NULL) {
                pw_log_warn("out of buffers: %m");
                return;
        }

        if (data->width == 0) {
            std::cout << "TODO FIX: Zero width process?" << std::endl;
            pw_stream_queue_buffer(data->stream, b);
            return;
        };


        auto spa_buffer = b->buffer;

        // printf("got a frame of size %d type: %d\n", spa_buffer->datas[0].chunk->size, spa_buffer->datas[0].type);
        if (!spa_buffer || !spa_buffer->datas[0].data) {
            // Return the buffer to the stream and exit
            // Currently this fails on PID for some reason??
            std::cout << "TODO FIX: No data supplied; type: " << std::endl;
            pw_stream_queue_buffer(data->stream, b);
            return;
        }

        // Source pointer and stride (assume RGBA)
        uint8_t *src = (uint8_t*)spa_buffer->datas[0].data;
        int stride = data->real_width * 4;

        int just_cleared = false;


        // Allocate latest_frame if needed
        if (data->latest_frame && (data->latest_frame->width != data->width || data->latest_frame->height != data->height)) {
            printf("FREEING BUFFERS\n");

            if (data->encCtx) {
                avcodec_send_frame(data->encCtx, nullptr); // flush
                while (avcodec_receive_packet(data->encCtx, data->encPkt) == 0) {
                    if (data->encPkt)
                        av_packet_unref(data->encPkt);
                }
                avcodec_free_context(&data->encCtx);
                data->encCtx = nullptr;
            }


            av_frame_free(&data->latest_frame); // idk may work :P // May not, @grok is this correct
            av_frame_free(&data->enc_frame); 
            av_packet_free(&data->encPkt);
            data->latest_frame = nullptr;

            just_cleared = true;


            if (data->rtpFmt) {
                avformat_free_context(data->rtpFmt);
                data->rtpFmt = nullptr;
            }
            if (data->sws) {
                sws_freeContext(data->sws);
                data->sws = nullptr;
            }

            printf("BUFFERS FREED\n");

        }
        if (!data->latest_frame) { // Lifetime of enc_frame, latest_frame, and encPkt are synced.
            if (setup_libav_buffers(data)) {
                pw_stream_queue_buffer(data->stream, b);
                return;
            }
        }

        // Copy lines into latest_frame
        for (int y = 0; y < data->height; ++y) {
            memcpy(data->latest_frame->data[0] + y * data->latest_frame->linesize[0],
                src + y * stride, stride);
        }

        #if send_instantly
        send_latest_data(data);
        #endif

        pw_stream_queue_buffer(data->stream, b);
}



static const struct pw_stream_events stream_events = {
        PW_VERSION_STREAM_EVENTS,
        .param_changed = on_param_changed,
        .process = on_process,
};


void print_spa_dict(const struct spa_dict *props) {
    if (!props) {
        std::cout << "\t\t\t[spa_dict] (null)\n";
        return;
    }
    for (uint32_t i = 0; i < props->n_items; ++i) {
        std::cout << "\t\t\t" << props->items[i].key << " = " << props->items[i].value << std::endl;
    }
}

static void registry_event_global(void *data_raw,
                                  uint32_t id,
                                  uint32_t permissions,
                                  const char *type,
                                  uint32_t version,
                                  const struct spa_dict *props)
{
    stateData *data = (stateData *)data_raw;

    std::cout << "\t\tOffered: " << id << std::endl;
    print_spa_dict(props);

    const char* pid = spa_dict_lookup(props, PW_KEY_SEC_PID);
    // if (pid) std::cout << "Comparing pid: "<< pid << " to target pid: " << data->pw_target_search_pid << std::endl;
    if (pid && atoi(pid) == data->pw_target_search_pid) {
        data->pw_target_client_id = id;
        std::cout << "Updated client target ID: " << id << std::endl;
        return;
    }

    const char* client_id = spa_dict_lookup(props, PW_KEY_CLIENT_ID);
    // if (client_id) std::cout << "Comparing client_id: "<< client_id << " to target client: " << data->pw_target_client_id << std::endl;
    if (client_id && atoi(client_id) == data->pw_target_client_id) {
        std::cout << "Updated target ID: " << id << std::endl;
        const char* media_class = spa_dict_lookup(props, "media.class");
    std::cout << "Connecting to node id: " << data->pw_target_id << " with media.class: " << (media_class ?: "(none)") << std::endl;
        data->pw_target_id = id;
        return;
    }

    return;
}

static const struct pw_registry_events registry_events = {
    PW_VERSION_REGISTRY_EVENTS,
    .global = registry_event_global,
    // .global_remove = registry_global_remove,
};

static void core_done(void *data_raw, uint32_t id, int seq)
{
    stateData *data = (stateData *)data_raw;

    std::cout << "Core sync done; using stream ID: " << data->pw_target_id << std::endl;
    if (data->pw_target_id > 0) {
        int re = pw_stream_connect(data->stream,
            PW_DIRECTION_INPUT,
            data->pw_target_id,
            (enum pw_stream_flags) (PW_STREAM_FLAG_AUTOCONNECT | PW_STREAM_FLAG_MAP_BUFFERS),
            data->pw_connect_params.pw_target_connect_helper_params, 1);
    }
}

static const pw_core_events core_events = {
    PW_VERSION_CORE_EVENTS,
    .done = core_done,
};


void prepare_recording(stateData *data, int targetPid) {
        struct pw_properties *props;


        pw_init(nullptr, nullptr);

        data->loop = pw_main_loop_new(NULL);

        props = pw_properties_new(PW_KEY_MEDIA_TYPE, "Video",
                        NULL);

        data->stream = pw_stream_new_simple(
                        pw_main_loop_get_loop(data->loop),
                        "video-capture",
                        props,
                        &stream_events,
                        data);



        data->pw_connect_params.b = SPA_POD_BUILDER_INIT(data->pw_connect_params.buffer, sizeof(data->pw_connect_params.buffer));

        data->pw_connect_params.min_size = SPA_RECTANGLE(1, 1);
        data->pw_connect_params.max_size = SPA_RECTANGLE(4096, 4096);
        data->pw_connect_params.default_size = SPA_RECTANGLE(1920, 1080);

        data->pw_connect_params.min_framerate = SPA_FRACTION(0, 1);
        data->pw_connect_params.max_framerate = SPA_FRACTION(1000, 1);
        data->pw_connect_params.default_framerate = SPA_FRACTION(data->fps, 1);

        data->pw_connect_params.pw_target_connect_helper_params[0] =
            (const struct spa_pod*) spa_pod_builder_add_object(
                &data->pw_connect_params.b,
                SPA_TYPE_OBJECT_Format, SPA_PARAM_EnumFormat,
                SPA_FORMAT_mediaType,       SPA_POD_Id(SPA_MEDIA_TYPE_video),
                SPA_FORMAT_mediaSubtype,    SPA_POD_Id(SPA_MEDIA_SUBTYPE_raw),
                SPA_FORMAT_VIDEO_format,    SPA_POD_CHOICE_ENUM_Id(2, SPA_VIDEO_FORMAT_BGRx, SPA_VIDEO_FORMAT_BGRx),
                SPA_FORMAT_VIDEO_size,      SPA_POD_CHOICE_RANGE_Rectangle(
                    &data->pw_connect_params.default_size,
                    &data->pw_connect_params.min_size,
                    &data->pw_connect_params.max_size
                ),
                SPA_FORMAT_VIDEO_framerate, SPA_POD_CHOICE_RANGE_Fraction(
                    &data->pw_connect_params.default_framerate,
                    &data->pw_connect_params.min_framerate,
                    &data->pw_connect_params.max_framerate
                )
            );
        

        if (targetPid > 0) {
            data->pw_target_search_pid = targetPid;
            data->pw_target_client_id = -1;
            data->pw_target_id = -1;

            pw_context *context = pw_context_new(
                pw_main_loop_get_loop(data->loop),
                nullptr, 0);
            if (!context) {
                std::cerr << "Failed to create context\n";
                return;// 1;
            }

            pw_core *core = pw_context_connect(context, nullptr, 0);
            if (!core) {
                std::cerr << "Failed to connect core\n";
                return;
            }

            pw_registry *registry = pw_core_get_registry(core, PW_VERSION_REGISTRY, 0);


            pw_core_add_listener(core,
                                &data->core_listener,
                                &core_events,
                                data);

            pw_registry_add_listener(registry, &data->registry_listener, &registry_events, data);
            
            pw_core_sync(core, PW_ID_CORE, 0);

            std::cout << "Should be listening ??" << std::endl;
        } else {
            int re = pw_stream_connect(data->stream,
                            PW_DIRECTION_INPUT,
                            PW_ID_ANY,
                            (enum pw_stream_flags) (PW_STREAM_FLAG_AUTOCONNECT | PW_STREAM_FLAG_MAP_BUFFERS),
                            data->pw_connect_params.pw_target_connect_helper_params, 1);
            printf("Hi me! GH, %d\n", re);
        }

        #if send_instantly
        #else
        const int interval_ms = 1000 / data->fps; // 16ms for 60 FPS
        auto frame_timer = pw_loop_add_timer(pw_main_loop_get_loop(data->loop), send_frame_timer, (void*) data);
        PW_THROW_IF(!frame_timer, "Failed to create timer for frame sending");
        // Set the timer to trigger every interval_ms milliseconds
        struct timespec interval = { .tv_sec = 0, .tv_nsec = 1000000 * (1000 / data->fps) };  // 60 FPS in ns
        struct timespec value = { .tv_sec = 0, .tv_nsec = 1000000 * (1000 / data->fps) };  // 60 FPS in ns

        // Set the initial value and interval for the timer
        pw_loop_update_timer(pw_main_loop_get_loop(data->loop), frame_timer, &value, &interval, true);

        // pw_timer_set(frame_timer, interval_ms, interval_ms);  // The same interval for both initial delay and periodicity
        #endif
}
void start_recording(stateData *data) {

    pw_main_loop_run(data->loop);

    pw_stream_destroy(data->stream);
    pw_main_loop_destroy(data->loop);
}