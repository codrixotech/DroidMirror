package com.codrixo.droidmirror;

import android.accessibilityservice.AccessibilityService;
import android.accessibilityservice.GestureDescription;
import android.content.ClipData;
import android.content.ClipboardManager;
import android.graphics.Path;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.os.SystemClock;
import android.util.DisplayMetrics;
import android.view.accessibility.AccessibilityEvent;
import android.view.accessibility.AccessibilityNodeInfo;

/**
 * Performs input from the paired Mac: taps and swipes as gestures, scrolling as short swipes,
 * text by editing the focused field, and Back / Home / Recents as global actions.
 *
 * <p>Android only lets an Accessibility Service do this; the user turns it on in Settings. It
 * reads no screen content except the focused text field when typing into it.
 */
public final class ControlService extends AccessibilityService {
    private static volatile ControlService instance;

    private final Handler main = new Handler(Looper.getMainLooper());

    // Current finger stroke (between DOWN and UP), main thread only.
    private Path stroke;
    private long strokeStart;

    // Scroll accumulated over a short window, then sent as one swipe.
    private float pendingH;
    private float pendingV;
    private float scrollX;
    private float scrollY;
    private boolean scrollScheduled;

    static ControlService get() {
        return instance;
    }

    @Override
    protected void onServiceConnected() {
        instance = this;
    }

    @Override
    public boolean onUnbind(android.content.Intent intent) {
        instance = null;
        return super.onUnbind(intent);
    }

    @Override
    public void onDestroy() {
        instance = null;
        super.onDestroy();
    }

    @Override
    public void onAccessibilityEvent(AccessibilityEvent event) {
        // Not used: we only act on commands from the Mac.
    }

    @Override
    public void onInterrupt() {}

    /** Called from the control connection thread. */
    void handle(ControlMessage m) {
        main.post(() -> handleOnMain(m));
    }

    private void handleOnMain(ControlMessage m) {
        switch (m.type) {
            case ControlMessage.TYPE_INJECT_TOUCH:
                touch(m);
                break;
            case ControlMessage.TYPE_INJECT_SCROLL:
                scroll(m);
                break;
            case ControlMessage.TYPE_INJECT_KEYCODE:
                if (m.action == ControlMessage.ACTION_DOWN) {
                    key(m.keycode, m.metaState);
                }
                break;
            case ControlMessage.TYPE_BACK_OR_SCREEN_ON:
                if (m.action == ControlMessage.ACTION_DOWN) {
                    performGlobalAction(GLOBAL_ACTION_BACK);
                }
                break;
            case ControlMessage.TYPE_INJECT_TEXT:
                insertText(m.text);
                break;
            case ControlMessage.TYPE_SET_CLIPBOARD:
                setClipboard(m.text, m.paste);
                break;
            default:
                break;
        }
    }

    // --- touch ----------------------------------------------------------------

    private float[] toScreen(ControlMessage m) {
        DisplayMetrics d = ScreenEncoder.realMetrics(this);
        float sx = m.frameWidth > 0 ? (float) d.widthPixels / m.frameWidth : 1f;
        float sy = m.frameHeight > 0 ? (float) d.heightPixels / m.frameHeight : 1f;
        float x = Math.max(0, Math.min(d.widthPixels - 1, m.x * sx));
        float y = Math.max(0, Math.min(d.heightPixels - 1, m.y * sy));
        return new float[] {x, y};
    }

    /**
     * Gestures can only be dispatched whole, so the stroke is recorded from DOWN to UP and
     * replayed with its real duration: taps, long presses and swipes all work, a drag shows
     * on the phone when the mouse button is released.
     */
    private void touch(ControlMessage m) {
        float[] p = toScreen(m);
        switch (m.action) {
            case ControlMessage.ACTION_DOWN:
                stroke = new Path();
                stroke.moveTo(p[0], p[1]);
                strokeStart = SystemClock.uptimeMillis();
                break;
            case ControlMessage.ACTION_MOVE:
                if (stroke != null) {
                    stroke.lineTo(p[0], p[1]);
                }
                break;
            case ControlMessage.ACTION_UP:
                if (stroke == null) {
                    return;
                }
                stroke.lineTo(p[0], p[1]);
                long max = GestureDescription.getMaxGestureDuration();
                long duration = Math.max(1, Math.min(max, SystemClock.uptimeMillis() - strokeStart));
                dispatch(stroke, duration);
                stroke = null;
                break;
            default:
                break;
        }
    }

    private void dispatch(Path path, long durationMs) {
        GestureDescription g = new GestureDescription.Builder()
                .addStroke(new GestureDescription.StrokeDescription(path, 0, durationMs))
                .build();
        dispatchGesture(g, null, null);
    }

    private void scroll(ControlMessage m) {
        float[] p = toScreen(m);
        scrollX = p[0];
        scrollY = p[1];
        pendingH += m.hScroll;
        pendingV += m.vScroll;
        if (!scrollScheduled) {
            scrollScheduled = true;
            main.postDelayed(this::flushScroll, 80);
        }
    }

    private void flushScroll() {
        scrollScheduled = false;
        DisplayMetrics d = ScreenEncoder.realMetrics(this);
        float step = d.heightPixels / 12f; // one wheel tick
        // Wheel up (positive) scrolls content down: the finger moves down.
        float dx = -pendingH * step;
        float dy = pendingV * step;
        pendingH = 0;
        pendingV = 0;
        if (Math.abs(dx) < 1 && Math.abs(dy) < 1) {
            return;
        }
        float endX = Math.max(0, Math.min(d.widthPixels - 1, scrollX + dx));
        float endY = Math.max(0, Math.min(d.heightPixels - 1, scrollY + dy));
        Path path = new Path();
        path.moveTo(scrollX, scrollY);
        path.lineTo(endX, endY);
        dispatch(path, 150);
    }

    // --- keys and text ----------------------------------------------------------

    private static final int META_CTRL_ON = 0x1000;

    private void key(int keycode, int meta) {
        boolean ctrl = (meta & META_CTRL_ON) != 0;
        switch (keycode) {
            case 3: // HOME
                performGlobalAction(GLOBAL_ACTION_HOME);
                return;
            case 4: // BACK
            case 111: // ESCAPE
                performGlobalAction(GLOBAL_ACTION_BACK);
                return;
            case 187: // APP_SWITCH
                performGlobalAction(GLOBAL_ACTION_RECENTS);
                return;
            case 66: // ENTER
            case 160: // NUMPAD_ENTER
                withFocus(n -> n.performAction(
                        AccessibilityNodeInfo.AccessibilityAction.ACTION_IME_ENTER.getId()));
                return;
            case 67: // DEL (backspace)
                withFocus(n -> deleteChars(n, true));
                return;
            case 112: // FORWARD_DEL
                withFocus(n -> deleteChars(n, false));
                return;
            case 21: // DPAD_LEFT
                withFocus(n -> moveCursor(n, -1));
                return;
            case 22: // DPAD_RIGHT
                withFocus(n -> moveCursor(n, 1));
                return;
            case 122: // MOVE_HOME
                withFocus(n -> setSelection(n, 0, 0));
                return;
            case 123: // MOVE_END
                withFocus(n -> {
                    int len = textOf(n).length();
                    setSelection(n, len, len);
                });
                return;
            default:
                break;
        }
        if (ctrl) {
            switch (keycode) {
                case 29: // A
                    withFocus(n -> setSelection(n, 0, textOf(n).length()));
                    break;
                case 31: // C
                    withFocus(n -> n.performAction(AccessibilityNodeInfo.ACTION_COPY));
                    break;
                case 52: // X
                    withFocus(n -> n.performAction(AccessibilityNodeInfo.ACTION_CUT));
                    break;
                case 50: // V
                    withFocus(n -> n.performAction(AccessibilityNodeInfo.ACTION_PASTE));
                    break;
                default:
                    break;
            }
        }
    }

    private interface NodeAction {
        void run(AccessibilityNodeInfo node);
    }

    private void withFocus(NodeAction action) {
        AccessibilityNodeInfo root = getRootInActiveWindow();
        if (root == null) {
            return;
        }
        AccessibilityNodeInfo node = root.findFocus(AccessibilityNodeInfo.FOCUS_INPUT);
        if (node != null && node.isEditable()) {
            action.run(node);
        }
    }

    private static String textOf(AccessibilityNodeInfo n) {
        if (n.isShowingHintText()) {
            return "";
        }
        CharSequence t = n.getText();
        return t == null ? "" : t.toString();
    }

    private static int[] selection(AccessibilityNodeInfo n, String text) {
        int s = n.getTextSelectionStart();
        int e = n.getTextSelectionEnd();
        if (s < 0 || e < 0 || s > text.length() || e > text.length()) {
            return new int[] {text.length(), text.length()};
        }
        return new int[] {Math.min(s, e), Math.max(s, e)};
    }

    private static void setText(AccessibilityNodeInfo n, String text, int cursor) {
        Bundle args = new Bundle();
        args.putCharSequence(AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE, text);
        n.performAction(AccessibilityNodeInfo.ACTION_SET_TEXT, args);
        setSelection(n, cursor, cursor);
    }

    private static void setSelection(AccessibilityNodeInfo n, int start, int end) {
        Bundle args = new Bundle();
        args.putInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_START_INT, start);
        args.putInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_END_INT, end);
        n.performAction(AccessibilityNodeInfo.ACTION_SET_SELECTION, args);
    }

    private void insertText(String insert) {
        if (insert == null || insert.isEmpty()) {
            return;
        }
        withFocus(n -> {
            String text = textOf(n);
            int[] sel = selection(n, text);
            setText(n, text.substring(0, sel[0]) + insert + text.substring(sel[1]), sel[0] + insert.length());
        });
    }

    private static void deleteChars(AccessibilityNodeInfo n, boolean before) {
        String text = textOf(n);
        int[] sel = selection(n, text);
        if (sel[0] != sel[1]) {
            setText(n, text.substring(0, sel[0]) + text.substring(sel[1]), sel[0]);
        } else if (before && sel[0] > 0) {
            int start = text.offsetByCodePoints(sel[0], -1);
            setText(n, text.substring(0, start) + text.substring(sel[0]), start);
        } else if (!before && sel[0] < text.length()) {
            int end = text.offsetByCodePoints(sel[0], 1);
            setText(n, text.substring(0, sel[0]) + text.substring(end), sel[0]);
        }
    }

    private static void moveCursor(AccessibilityNodeInfo n, int delta) {
        String text = textOf(n);
        int[] sel = selection(n, text);
        int pos = delta < 0 ? sel[0] : sel[1];
        if (sel[0] == sel[1]) {
            if (delta < 0 && pos > 0) {
                pos = text.offsetByCodePoints(pos, -1);
            } else if (delta > 0 && pos < text.length()) {
                pos = text.offsetByCodePoints(pos, 1);
            }
        }
        setSelection(n, pos, pos);
    }

    private void setClipboard(String text, boolean paste) {
        ClipboardManager cm = getSystemService(ClipboardManager.class);
        if (cm != null && text != null) {
            cm.setPrimaryClip(ClipData.newPlainText("DroidMirror", text));
        }
        if (paste) {
            withFocus(n -> n.performAction(AccessibilityNodeInfo.ACTION_PASTE));
        }
    }
}
