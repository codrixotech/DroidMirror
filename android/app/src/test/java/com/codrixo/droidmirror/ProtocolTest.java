package com.codrixo.droidmirror;

import static org.junit.Assert.assertArrayEquals;
import static org.junit.Assert.assertEquals;

import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.io.DataInputStream;
import java.util.Arrays;

import org.junit.Test;

public class ProtocolTest {
    private static byte[] filled(int len, int value) {
        byte[] b = new byte[len];
        Arrays.fill(b, (byte) value);
        return b;
    }

    @Test
    public void pairingCodeMatchesTheMacApp() {
        // Same vector as cross_language_vector() in the Mac app's src/companion/tests.rs.
        assertEquals(453614, Protocol.pairingCode(filled(32, 7), filled(16, 1)));
        assertEquals("453 614", Protocol.formatCode(453614));
        assertEquals("000 042", Protocol.formatCode(42));
    }

    @Test
    public void sessionHeader() {
        assertArrayEquals(new byte[] {(byte) 0x80, 0, 0, 0, 0, 0, 1, (byte) 0xc8, 0, 0, 4, 0},
                Protocol.sessionHeader(456, 1024));
    }

    @Test
    public void mediaHeaders() {
        byte[] h = new byte[12];
        Protocol.mediaHeader(h, false, false, 0x0102, 3);
        assertArrayEquals(new byte[] {0, 0, 0, 0, 0, 0, 1, 2, 0, 0, 0, 3}, h);
        Protocol.mediaHeader(h, false, true, 9, 1);
        assertArrayEquals(new byte[] {0x20, 0, 0, 0, 0, 0, 0, 9, 0, 0, 0, 1}, h);
        Protocol.mediaHeader(h, true, false, 12345, 30);
        assertArrayEquals(new byte[] {0x40, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 30}, h);
    }

    @Test
    public void codecId() throws Exception {
        ByteArrayOutputStream out = new ByteArrayOutputStream();
        Protocol.writeCodecId(out);
        assertArrayEquals("h264".getBytes(), out.toByteArray());
    }

    @Test
    public void callStateMessageMatchesTheMacParser() {
        // Same bytes as parses_companion_call_state() in the Mac app.
        assertArrayEquals(new byte[] {100, 1, 0, 0, 0, 3, '1', '2', '3'}, Protocol.callStateMessage(1, "123"));
        assertArrayEquals(new byte[] {100, 0, 0, 0, 0, 0}, Protocol.callStateMessage(0, null));
    }

    @Test
    public void controlUnavailableMatchesTheMacParser() {
        // Same byte as parses_companion_control_unavailable() in the Mac app.
        assertArrayEquals(new byte[] {101}, Protocol.controlUnavailableMessage());
    }

    @Test
    public void deviceName() {
        byte[] f = Protocol.deviceNameField("RMX3771");
        assertEquals(64, f.length);
        assertEquals('R', f[0]);
        assertEquals(0, f[7]);
        // Never splits a multi-byte character, always NUL-terminated.
        byte[] long1 = Protocol.deviceNameField("é".repeat(40));
        assertEquals(0, long1[63]);
        assertEquals(0, long1[62]);
    }

    @Test
    public void encodedSizes() {
        assertArrayEquals(new int[] {456, 1024}, Protocol.encodedSize(1080, 2412, 1024));
        assertArrayEquals(new int[] {1024, 456}, Protocol.encodedSize(2412, 1080, 1024));
        assertArrayEquals(new int[] {720, 1600}, Protocol.encodedSize(720, 1600, 2000));
    }

    private static ControlMessage parse(int... bytes) throws Exception {
        byte[] b = new byte[bytes.length];
        for (int i = 0; i < bytes.length; i++) {
            b[i] = (byte) bytes[i];
        }
        return ControlMessage.read(new DataInputStream(new ByteArrayInputStream(b)));
    }

    // Vectors from scrcpy's app/tests/test_control_msg_serialize.c (also used by the Mac app).

    @Test
    public void parsesKeycode() throws Exception {
        ControlMessage m = parse(0, 0x01, 0, 0, 0, 0x42, 0, 0, 0, 5, 0, 0, 0, 0x41);
        assertEquals(ControlMessage.TYPE_INJECT_KEYCODE, m.type);
        assertEquals(ControlMessage.ACTION_UP, m.action);
        assertEquals(66, m.keycode);
        assertEquals(5, m.repeat);
        assertEquals(0x41, m.metaState);
    }

    @Test
    public void parsesTouch() throws Exception {
        ControlMessage m = parse(2, 0x00, 0x12, 0x34, 0x56, 0x78, 0x87, 0x65, 0x43, 0x21, 0, 0, 0, 0x64,
                0, 0, 0, 0xc8, 0x04, 0x38, 0x07, 0x80, 0xff, 0xff, 0, 0, 0, 1, 0, 0, 0, 1);
        assertEquals(ControlMessage.TYPE_INJECT_TOUCH, m.type);
        assertEquals(ControlMessage.ACTION_DOWN, m.action);
        assertEquals(100, m.x);
        assertEquals(200, m.y);
        assertEquals(1080, m.frameWidth);
        assertEquals(1920, m.frameHeight);
    }

    @Test
    public void parsesScroll() throws Exception {
        ControlMessage m = parse(3, 0, 0, 0x01, 0x04, 0, 0, 0x04, 0x02, 0x04, 0x38, 0x07, 0x80,
                0x7f, 0xff, 0x80, 0x00, 0, 0, 0, 1);
        assertEquals(ControlMessage.TYPE_INJECT_SCROLL, m.type);
        assertEquals(260, m.x);
        assertEquals(1026, m.y);
        assertEquals(16f, m.hScroll, 0.01f);
        assertEquals(-16f, m.vScroll, 0.01f);
    }

    @Test
    public void parsesTextAndClipboard() throws Exception {
        ControlMessage t = parse(1, 0, 0, 0, 2, 'h', 'i');
        assertEquals("hi", t.text);
        ControlMessage c = parse(9, 1, 2, 3, 4, 5, 6, 7, 8, 1, 0, 0, 0, 2, 'o', 'k');
        assertEquals(ControlMessage.TYPE_SET_CLIPBOARD, c.type);
        assertEquals(true, c.paste);
        assertEquals("ok", c.text);
    }

    @Test
    public void parsesEmptyMessages() throws Exception {
        assertEquals(ControlMessage.TYPE_RESET_VIDEO, parse(17).type);
        assertEquals(ControlMessage.TYPE_BACK_OR_SCREEN_ON, parse(4, 0).type);
        assertEquals(ControlMessage.TYPE_GET_CLIPBOARD, parse(8, 1).type);
    }

    @Test(expected = java.io.IOException.class)
    public void rejectsUnknownType() throws Exception {
        parse(99);
    }
}
