package com.codrixo.droidmirror;

import java.io.IOException;
import java.io.OutputStream;
import java.nio.ByteBuffer;
import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.util.Locale;

/**
 * Wire format shared with the Mac app (src/companion/mod.rs there).
 *
 * <p>Every connection starts, after TLS, with {@code "DMC1" | channel | payload} from the Mac,
 * answered by a status byte. The video channel then carries the scrcpy video stream format
 * (64-byte device name, codec id, 12-byte session and media headers), the control channel
 * carries scrcpy control messages, and the events channel carries device messages.
 */
final class Protocol {
    private Protocol() {}

    static final byte[] MAGIC = {'D', 'M', 'C', '1'};

    static final int CHANNEL_VIDEO = 0;
    static final int CHANNEL_CONTROL = 1;
    static final int CHANNEL_EVENTS = 2;
    static final int CHANNEL_PAIR = 3;

    static final int STATUS_OK = 0;
    static final int STATUS_DENIED = 1;
    static final int STATUS_UNKNOWN_TOKEN = 2;
    static final int STATUS_NOT_SHARING = 3;

    static final int TOKEN_LENGTH = 32;
    static final int NONCE_LENGTH = 16;
    static final int DEVICE_NAME_LENGTH = 64;

    /** "h264", as in scrcpy. */
    static final int CODEC_ID_H264 = 0x68323634;

    private static final long FLAG_CONFIG = 1L << 62;
    private static final long FLAG_KEY_FRAME = 1L << 61;

    /** Device message type for call state (an extension; scrcpy uses 0-2). */
    static final int MSG_CALL_STATE = 100;
    /** Input can't be performed: the Accessibility service isn't running. */
    static final int MSG_CONTROL_UNAVAILABLE = 101;

    /** Code both devices show while pairing: SHA-256(fingerprint || nonce), 6 digits. */
    static int pairingCode(byte[] fingerprint, byte[] nonce) {
        byte[] h = sha256(concat(fingerprint, nonce));
        long v = ((h[0] & 0xffL) << 24) | ((h[1] & 0xffL) << 16) | ((h[2] & 0xffL) << 8) | (h[3] & 0xffL);
        return (int) (v % 1_000_000);
    }

    static String formatCode(int code) {
        return String.format(Locale.ROOT, "%03d %03d", code / 1000, code % 1000);
    }

    static byte[] sha256(byte[] data) {
        try {
            return MessageDigest.getInstance("SHA-256").digest(data);
        } catch (NoSuchAlgorithmException e) {
            throw new IllegalStateException(e);
        }
    }

    static byte[] concat(byte[] a, byte[] b) {
        byte[] out = new byte[a.length + b.length];
        System.arraycopy(a, 0, out, 0, a.length);
        System.arraycopy(b, 0, out, a.length, b.length);
        return out;
    }

    /** UTF-8, truncated on a character boundary to 63 bytes, NUL-padded to 64. */
    static byte[] deviceNameField(String name) {
        byte[] out = new byte[DEVICE_NAME_LENGTH];
        byte[] raw = name.getBytes(StandardCharsets.UTF_8);
        int len = Math.min(raw.length, DEVICE_NAME_LENGTH - 1);
        while (len > 0 && len < raw.length && (raw[len] & 0xc0) == 0x80) {
            len--; // don't cut a multi-byte character
        }
        System.arraycopy(raw, 0, out, 0, len);
        return out;
    }

    static void writeCodecId(OutputStream out) throws IOException {
        out.write(ByteBuffer.allocate(4).putInt(CODEC_ID_H264).array());
    }

    /** 12-byte session packet: MSB set, then width and height. */
    static byte[] sessionHeader(int width, int height) {
        return ByteBuffer.allocate(12).putInt(0x80000000).putInt(width).putInt(height).array();
    }

    /** 12-byte media packet header (scrcpy Streamer.writeFrameMeta). */
    static void mediaHeader(byte[] dst, boolean config, boolean keyFrame, long ptsUs, int size) {
        long flags = config ? FLAG_CONFIG : (ptsUs & (FLAG_KEY_FRAME - 1)) | (keyFrame ? FLAG_KEY_FRAME : 0);
        ByteBuffer.wrap(dst).putLong(flags).putInt(size);
    }

    /** state: 0 idle, 1 ringing, 2 in a call. */
    static byte[] callStateMessage(int state, String number) {
        byte[] n = number == null ? new byte[0] : number.getBytes(StandardCharsets.UTF_8);
        return ByteBuffer.allocate(6 + n.length).put((byte) MSG_CALL_STATE).put((byte) state)
                .putInt(n.length).put(n).array();
    }

    static byte[] controlUnavailableMessage() {
        return new byte[] {(byte) MSG_CONTROL_UNAVAILABLE};
    }

    /** Scales the screen so its long side is at most {@code max}, multiples of 8. */
    static int[] encodedSize(int width, int height, int max) {
        int longSide = Math.max(width, height);
        if (longSide > max) {
            double s = (double) max / longSide;
            width = (int) Math.round(width * s);
            height = (int) Math.round(height * s);
        }
        return new int[] {Math.max(8, width & ~7), Math.max(8, height & ~7)};
    }
}
