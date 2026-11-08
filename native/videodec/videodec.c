/*
 * videodec: 3zT Mirroring's HEVC decoder DLL.
 *
 * Windows only decodes HEVC through Media Foundation when Microsoft's "HEVC
 * Video Extensions" are installed. The graphics driver itself can decode HEVC
 * (DXVA / D3D11 video), so this DLL wraps FFmpeg's HEVC decoder with its
 * Direct3D 11 hardware path on the window's own D3D11 device, and falls back
 * to FFmpeg's multithreaded software decoder when the GPU cannot.
 *
 * Input: Annex-B access units (parameter sets in-band). Output: a D3D11 NV12
 * texture + array slice (hardware) or YUV 4:2:0 planes (software).
 *
 * Copyright (C) 2026 3zT. GPL-3.0-or-later.
 * FFmpeg is LGPL-2.1-or-later (see THIRD_PARTY_NOTICES.md).
 */
#define COBJMACROS
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#include <d3d11.h>

#include <libavcodec/avcodec.h>
#include <libavutil/hwcontext.h>
#include <libavutil/hwcontext_d3d11va.h>
#include <libavutil/pixdesc.h>

#define VD_API __declspec(dllexport)

/* What vd_receive hands out. Valid until the next vd_receive / vd_close. */
typedef struct VdFrame {
    int width;
    int height;
    /* 1: texture + slice are set (NV12, D3D11). 0: planes are set (8-bit 4:2:0). */
    int hardware;
    void *texture;           /* ID3D11Texture2D* (not AddRef'd) */
    int slice;
    const uint8_t *plane[3]; /* Y, U, V  (or Y, UV for NV12) */
    int stride[3];
    int nv12;                /* software planes are NV12 (2 planes) */
} VdFrame;

typedef struct Vd {
    AVCodecContext *ctx;
    AVPacket *pkt;
    AVFrame *frame;
    AVBufferRef *hw;
    int hw_ok;
} Vd;

static enum AVPixelFormat pick_format(AVCodecContext *ctx, const enum AVPixelFormat *fmts)
{
    Vd *d = ctx->opaque;
    const enum AVPixelFormat *p;
    if (d && d->hw_ok) {
        for (p = fmts; *p != AV_PIX_FMT_NONE; p++)
            if (*p == AV_PIX_FMT_D3D11)
                return *p;
    }
    /* No usable hardware format: the first software one. */
    for (p = fmts; *p != AV_PIX_FMT_NONE; p++) {
        const AVPixFmtDescriptor *desc = av_pix_fmt_desc_get(*p);
        if (desc && !(desc->flags & AV_PIX_FMT_FLAG_HWACCEL))
            return *p;
    }
    return AV_PIX_FMT_NONE;
}

VD_API void vd_close(Vd *d)
{
    if (!d)
        return;
    av_frame_free(&d->frame);
    av_packet_free(&d->pkt);
    avcodec_free_context(&d->ctx);
    av_buffer_unref(&d->hw);
    free(d);
}

/* device: the window's ID3D11Device (multithread-protected), or NULL for
 * software only. Returns NULL on failure. *hardware tells which path is set. */
VD_API Vd *vd_open(void *device, int *hardware)
{
    const AVCodec *codec = avcodec_find_decoder(AV_CODEC_ID_HEVC);
    Vd *d;

    if (hardware)
        *hardware = 0;
    if (!codec)
        return NULL;
    d = calloc(1, sizeof(*d));
    if (!d)
        return NULL;
    d->ctx = avcodec_alloc_context3(codec);
    d->pkt = av_packet_alloc();
    d->frame = av_frame_alloc();
    if (!d->ctx || !d->pkt || !d->frame)
        goto fail;
    d->ctx->opaque = d;
    d->ctx->get_format = pick_format;
    d->ctx->flags |= AV_CODEC_FLAG_LOW_DELAY;
    d->ctx->flags2 |= AV_CODEC_FLAG2_FAST;

    if (device) {
        AVBufferRef *hw = av_hwdevice_ctx_alloc(AV_HWDEVICE_TYPE_D3D11VA);
        if (hw) {
            AVHWDeviceContext *dc = (AVHWDeviceContext *)hw->data;
            AVD3D11VADeviceContext *dx = dc->hwctx;
            dx->device = (ID3D11Device *)device;
            ID3D11Device_AddRef(dx->device);
            if (av_hwdevice_ctx_init(hw) >= 0) {
                d->hw = hw;
                d->ctx->hw_device_ctx = av_buffer_ref(hw);
                /* Frames stay in the decoder's pool until copied out. */
                d->ctx->extra_hw_frames = 4;
                d->hw_ok = d->ctx->hw_device_ctx != NULL;
            } else {
                av_buffer_unref(&hw);
            }
        }
    }
    if (!d->hw_ok) {
        /* Software: all cores, slices and frames. */
        d->ctx->thread_count = 0;
        d->ctx->thread_type = FF_THREAD_FRAME | FF_THREAD_SLICE;
        d->ctx->flags &= ~AV_CODEC_FLAG_LOW_DELAY;
    }
    if (avcodec_open2(d->ctx, codec, NULL) < 0)
        goto fail;
    if (hardware)
        *hardware = d->hw_ok;
    return d;
fail:
    vd_close(d);
    return NULL;
}

/* One Annex-B access unit. 0 = accepted, 1 = decoder full (call vd_receive
 * first, then send the same data again), <0 = error. */
VD_API int vd_send(Vd *d, const uint8_t *data, int len)
{
    int r;
    if (!d || !data || len <= 0)
        return -1;
    d->pkt->data = (uint8_t *)data;
    d->pkt->size = len;
    r = avcodec_send_packet(d->ctx, d->pkt);
    d->pkt->data = NULL;
    d->pkt->size = 0;
    if (r == AVERROR(EAGAIN))
        return 1;
    return r < 0 ? r : 0;
}

/* 1 = a frame is in *out, 0 = none yet, <0 = error. */
VD_API int vd_receive(Vd *d, VdFrame *out)
{
    int r;
    if (!d || !out)
        return -1;
    av_frame_unref(d->frame);
    r = avcodec_receive_frame(d->ctx, d->frame);
    if (r == AVERROR(EAGAIN) || r == AVERROR_EOF)
        return 0;
    if (r < 0)
        return r;
    memset(out, 0, sizeof(*out));
    out->width = d->frame->width;
    out->height = d->frame->height;
    if (d->frame->format == AV_PIX_FMT_D3D11) {
        out->hardware = 1;
        out->texture = d->frame->data[0];
        out->slice = (int)(intptr_t)d->frame->data[1];
        return 1;
    }
    switch (d->frame->format) {
    case AV_PIX_FMT_YUV420P:
    case AV_PIX_FMT_YUVJ420P:
        out->plane[0] = d->frame->data[0];
        out->plane[1] = d->frame->data[1];
        out->plane[2] = d->frame->data[2];
        out->stride[0] = d->frame->linesize[0];
        out->stride[1] = d->frame->linesize[1];
        out->stride[2] = d->frame->linesize[2];
        return 1;
    case AV_PIX_FMT_NV12:
        out->nv12 = 1;
        out->plane[0] = d->frame->data[0];
        out->plane[1] = d->frame->data[1];
        out->stride[0] = d->frame->linesize[0];
        out->stride[1] = d->frame->linesize[1];
        return 1;
    default:
        /* 10-bit or 4:4:4: not something an iPhone screen stream sends. */
        return -2;
    }
}

VD_API void vd_flush(Vd *d)
{
    if (d && d->ctx)
        avcodec_flush_buffers(d->ctx);
}
