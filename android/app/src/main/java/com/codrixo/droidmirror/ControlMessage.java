package com.codrixo.droidmirror;

import java.io.DataInputStream;
import java.io.IOException;
import java.nio.charset.StandardCharsets;

/**
 * A control message from the Mac, in scrcpy's format (ControlMessageReader.java in scrcpy
 * 5.0.1, which the Mac app also uses for adb mode).
 */
final class ControlMessage {
    static final int TYPE_INJECT_KEYCODE = 0;
    static final int TYPE_INJECT_TEXT = 1;
    static final int TYPE_INJECT_TOUCH = 2;
    static final int TYPE_INJECT_SCROLL = 3;
    static final int TYPE_BACK_OR_SCREEN_ON = 4;
    static final int TYPE_GET_CLIPBOARD = 8;
    static final int TYPE_SET_CLIPBOARD = 9;
    static final int TYPE_RESET_VIDEO = 17;
    static final int TYPE_OTHER = -1;

    static final int ACTION_DOWN = 0;
    static final int ACTION_UP = 1;
    static final int ACTION_MOVE = 2;

    private static final int MAX_STRING = 1 << 18;

    int type;
    int action;
    int keycode;
    int repeat;
    int metaState;
    String text;
    boolean paste;
    int x;
    int y;
    int frameWidth;
    int frameHeight;
    float hScroll;
    float vScroll;

    static ControlMessage read(DataInputStream in) throws IOException {
        ControlMessage m = new ControlMessage();
        m.type = in.readUnsignedByte();
        switch (m.type) {
            case TYPE_INJECT_KEYCODE:
                m.action = in.readUnsignedByte();
                m.keycode = in.readInt();
                m.repeat = in.readInt();
                m.metaState = in.readInt();
                break;
            case TYPE_INJECT_TEXT:
                m.text = readString(in);
                break;
            case TYPE_INJECT_TOUCH:
                m.action = in.readUnsignedByte();
                in.readLong(); // pointer id
                readPosition(in, m);
                in.readUnsignedShort(); // pressure
                in.readInt(); // action button
                in.readInt(); // buttons
                break;
            case TYPE_INJECT_SCROLL:
                readPosition(in, m);
                // Encoded as i16 fixed point over [-1, 1], meaning [-16, 16].
                m.hScroll = in.readShort() / 32768f * 16f;
                m.vScroll = in.readShort() / 32768f * 16f;
                in.readInt(); // buttons
                break;
            case TYPE_BACK_OR_SCREEN_ON:
                m.action = in.readUnsignedByte();
                break;
            case TYPE_GET_CLIPBOARD:
                in.readUnsignedByte(); // copy key
                break;
            case TYPE_SET_CLIPBOARD:
                in.readLong(); // sequence
                m.paste = in.readUnsignedByte() != 0;
                m.text = readString(in);
                break;
            case 5: // expand notification panel
            case 6: // expand settings panel
            case 7: // collapse panels
            case 11: // rotate device
            case TYPE_RESET_VIDEO:
                break;
            case 10: // set display power
                in.readUnsignedByte();
                m.type = TYPE_OTHER;
                break;
            case 22: // scan file
                readString(in);
                m.type = TYPE_OTHER;
                break;
            default:
                throw new IOException("unsupported control message type " + m.type);
        }
        return m;
    }

    private static void readPosition(DataInputStream in, ControlMessage m) throws IOException {
        m.x = in.readInt();
        m.y = in.readInt();
        m.frameWidth = in.readUnsignedShort();
        m.frameHeight = in.readUnsignedShort();
    }

    private static String readString(DataInputStream in) throws IOException {
        int len = in.readInt();
        if (len < 0 || len > MAX_STRING) {
            throw new IOException("string too long: " + len);
        }
        byte[] b = new byte[len];
        in.readFully(b);
        return new String(b, StandardCharsets.UTF_8);
    }
}
