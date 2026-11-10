package com.codrixo.droidmirror;

import android.content.Context;
import android.hardware.display.DisplayManager;
import android.hardware.display.VirtualDisplay;
import android.media.MediaCodec;
import android.media.MediaCodecInfo;
import android.media.MediaFormat;
import android.os.Bundle;
import android.util.DisplayMetrics;
import android.view.Display;
import android.view.Surface;

import java.io.IOException;
import java.io.OutputStream;
import java.nio.ByteBuffer;

/**
 * Encodes the screen (through the MediaProjection virtual display) to H.264 and writes it in
 * the scrcpy video stream format. Restarts the encoder when the screen size changes
 * (rotation); the Mac then sees a new session packet and new SPS/PPS.
 */
final class ScreenEncoder {
    static final int MAX_SIZE = 1024;
    private static final int BIT_RATE = 3_000_000;
    private static final int MAX_FPS = 30;

    private final Context context;
    private final VirtualDisplay display;
    private final OutputStream out;
    private volatile boolean restart;
    private volatile boolean stopped;
    private volatile MediaCodec codec;

    ScreenEncoder(Context context, VirtualDisplay display, OutputStream out) {
        this.context = context;
        this.display = display;
        this.out = out;
    }

    /** Current real screen size and density (getRealMetrics is deprecated but exact). */
    @SuppressWarnings("deprecation")
    static DisplayMetrics realMetrics(Context context) {
        DisplayManager dm = context.getSystemService(DisplayManager.class);
        Display d = dm.getDisplay(Display.DEFAULT_DISPLAY);
        DisplayMetrics m = new DisplayMetrics();
        d.getRealMetrics(m);
        return m;
    }

    void requestRestart() {
        restart = true;
    }

    void stop() {
        stopped = true;
    }

    void requestKeyFrame() {
        MediaCodec c = codec;
        if (c == null) {
            return;
        }
        Bundle b = new Bundle();
        b.putInt(MediaCodec.PARAMETER_KEY_REQUEST_SYNC_FRAME, 0);
        try {
            c.setParameters(b);
        } catch (IllegalStateException ignored) {
            // encoder restarting
        }
    }

    /** Streams until stopped or the connection fails. */
    void run() throws IOException {
        byte[] header = new byte[12];
        byte[] payload = new byte[256 * 1024];
        Protocol.writeCodecId(out);
        while (!stopped) {
            DisplayMetrics m = realMetrics(context);
            int[] size = Protocol.encodedSize(m.widthPixels, m.heightPixels, MAX_SIZE);
            MediaFormat format = MediaFormat.createVideoFormat(MediaFormat.MIMETYPE_VIDEO_AVC, size[0], size[1]);
            format.setInteger(MediaFormat.KEY_COLOR_FORMAT, MediaCodecInfo.CodecCapabilities.COLOR_FormatSurface);
            format.setInteger(MediaFormat.KEY_BIT_RATE, BIT_RATE);
            format.setInteger(MediaFormat.KEY_FRAME_RATE, MAX_FPS);
            format.setFloat(MediaFormat.KEY_MAX_FPS_TO_ENCODER, MAX_FPS);
            format.setInteger(MediaFormat.KEY_I_FRAME_INTERVAL, 10);
            // Keep sending frames (and so key frames on request) on a still screen.
            format.setLong(MediaFormat.KEY_REPEAT_PREVIOUS_FRAME_AFTER, 100_000);

            MediaCodec c = MediaCodec.createEncoderByType(MediaFormat.MIMETYPE_VIDEO_AVC);
            c.configure(format, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE);
            Surface surface = c.createInputSurface();
            display.resize(size[0], size[1], m.densityDpi);
            display.setSurface(surface);
            c.start();
            codec = c;
            restart = false;
            try {
                out.write(Protocol.sessionHeader(size[0], size[1]));
                out.flush();
                MediaCodec.BufferInfo info = new MediaCodec.BufferInfo();
                while (!restart && !stopped) {
                    int index = c.dequeueOutputBuffer(info, 100_000);
                    if (index < 0) {
                        continue;
                    }
                    try {
                        if (info.size > 0) {
                            ByteBuffer buf = c.getOutputBuffer(index);
                            if (payload.length < info.size) {
                                payload = new byte[info.size];
                            }
                            buf.position(info.offset);
                            buf.get(payload, 0, info.size);
                            boolean config = (info.flags & MediaCodec.BUFFER_FLAG_CODEC_CONFIG) != 0;
                            boolean key = (info.flags & MediaCodec.BUFFER_FLAG_KEY_FRAME) != 0;
                            Protocol.mediaHeader(header, config, key, info.presentationTimeUs, info.size);
                            out.write(header);
                            out.write(payload, 0, info.size);
                            out.flush();
                        }
                    } finally {
                        c.releaseOutputBuffer(index, false);
                    }
                }
            } finally {
                codec = null;
                display.setSurface(null);
                try {
                    c.stop();
                } catch (IllegalStateException ignored) {
                    // already stopped
                }
                c.release();
                surface.release();
            }
        }
    }
}
