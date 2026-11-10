package com.codrixo.droidmirror;

import android.Manifest;
import android.app.Activity;
import android.app.AlertDialog;
import android.content.ComponentName;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.graphics.Insets;
import android.graphics.Typeface;
import android.media.projection.MediaProjectionConfig;
import android.media.projection.MediaProjectionManager;
import android.os.Build;
import android.os.Bundle;
import android.provider.Settings;
import android.text.TextUtils;
import android.view.View;
import android.view.WindowInsets;
import android.widget.Button;
import android.widget.LinearLayout;
import android.widget.ScrollView;
import android.widget.TextView;

import java.util.ArrayList;
import java.util.List;

/** Setup and status: allow control, grant permissions, start/stop sharing, pairing prompts. */
public final class MainActivity extends Activity implements MirrorService.Listener {
    private static final int REQUEST_CAPTURE = 1;
    private static final int REQUEST_PERMISSIONS = 2;

    private TextView controlStatus;
    private Button controlButton;
    private TextView permissionStatus;
    private Button permissionButton;
    private TextView shareStatus;
    private Button shareButton;
    private TextView pairedStatus;
    private Button forgetButton;
    private AlertDialog pairingDialog;

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        LinearLayout root = new LinearLayout(this);
        root.setOrientation(LinearLayout.VERTICAL);
        int pad = dp(20);
        root.setPadding(pad, pad, pad, pad);

        root.addView(text("DroidMirror", 26, true));
        root.addView(text("See and control this phone from your Mac, over Wi-Fi. "
                + "Install DroidMirror on the Mac, then follow these steps.", 15, false));

        root.addView(heading("1. Allow control"));
        controlStatus = text("", 14, false);
        root.addView(controlStatus);
        controlButton = button("Turn on in Settings", v -> showControlDisclosure());
        root.addView(controlButton);

        root.addView(heading("2. Notifications and calls"));
        permissionStatus = text("", 14, false);
        root.addView(permissionStatus);
        permissionButton = button("Allow", v -> requestPermissions());
        root.addView(permissionButton);

        root.addView(heading("3. Share the screen"));
        shareStatus = text("", 14, false);
        root.addView(shareStatus);
        shareButton = button("Start sharing", v -> toggleSharing());
        root.addView(shareButton);

        root.addView(heading("Paired Macs"));
        pairedStatus = text("", 14, false);
        root.addView(pairedStatus);
        forgetButton = button("Forget all paired Macs", v -> {
            MirrorService.forgetAll(this);
            refresh();
        });
        root.addView(forgetButton);

        ScrollView scroll = new ScrollView(this);
        scroll.addView(root);
        // Edge-to-edge (targetSdk 35+): keep content clear of the system bars.
        scroll.setOnApplyWindowInsetsListener((v, insets) -> {
            Insets bars = insets.getInsets(WindowInsets.Type.systemBars());
            v.setPadding(bars.left, bars.top, bars.right, bars.bottom);
            return WindowInsets.CONSUMED;
        });
        setContentView(scroll);
    }

    @Override
    protected void onResume() {
        super.onResume();
        MirrorService.setListener(this);
        refresh();
    }

    @Override
    protected void onPause() {
        MirrorService.setListener(null);
        super.onPause();
    }

    private void refresh() {
        boolean control = isControlEnabled();
        controlStatus.setText(control
                ? "✓ On. Your Mac can tap, swipe and type on this phone."
                : "Needed so your Mac can tap, swipe and type. Turn on \"DroidMirror\" under "
                        + "Settings → Accessibility → Installed apps (or Downloaded apps).");
        controlButton.setVisibility(control ? View.GONE : View.VISIBLE);

        List<String> missing = missingPermissions();
        permissionStatus.setText(missing.isEmpty()
                ? "✓ Allowed. Pairing requests and incoming calls show on your Mac."
                : "Lets pairing requests appear as notifications, and lets your Mac show, "
                        + "answer and end calls.");
        permissionButton.setVisibility(missing.isEmpty() ? View.GONE : View.VISIBLE);

        boolean running = MirrorService.isRunning();
        String status = MirrorService.status();
        shareStatus.setText(running
                ? (TextUtils.isEmpty(status) ? "Sharing." : status)
                : "Android asks for permission each time. Choose \"Entire screen\".");
        shareButton.setText(running ? "Stop sharing" : "Start sharing");

        int paired = MirrorService.pairedCount(this);
        pairedStatus.setText(paired == 0
                ? "None yet. Start sharing, open DroidMirror on your Mac and confirm the code."
                : paired + (paired == 1 ? " Mac" : " Macs") + " can connect to this phone.");
        forgetButton.setVisibility(paired == 0 ? View.GONE : View.VISIBLE);
    }

    private boolean isControlEnabled() {
        if (ControlService.get() != null) {
            return true;
        }
        String enabled = Settings.Secure.getString(getContentResolver(),
                Settings.Secure.ENABLED_ACCESSIBILITY_SERVICES);
        String me = new ComponentName(this, ControlService.class).flattenToString();
        return enabled != null && enabled.contains(me);
    }

    /** Google Play requires a prominent disclosure before the Accessibility settings. */
    private void showControlDisclosure() {
        new AlertDialog.Builder(this)
                .setTitle("Allow DroidMirror to control this phone?")
                .setMessage("DroidMirror uses Android's Accessibility service to perform the taps, "
                        + "swipes, scrolling and typing that you send from your paired Mac, and to "
                        + "press Back, Home and Recent apps.\n\n"
                        + "It reads only the text field you're typing into, when you type from the "
                        + "Mac. It doesn't collect, store or send any data anywhere else: the screen "
                        + "and your input go only to your paired Mac, encrypted, over your local "
                        + "network.")
                .setPositiveButton("Continue", (d, w) ->
                        startActivity(new Intent(Settings.ACTION_ACCESSIBILITY_SETTINGS)))
                .setNegativeButton("Not now", null)
                .show();
    }

    private List<String> missingPermissions() {
        List<String> wanted = new ArrayList<>();
        if (Build.VERSION.SDK_INT >= 33) {
            wanted.add(Manifest.permission.POST_NOTIFICATIONS);
        }
        wanted.add(Manifest.permission.READ_PHONE_STATE);
        wanted.add(Manifest.permission.ANSWER_PHONE_CALLS);
        List<String> missing = new ArrayList<>();
        for (String p : wanted) {
            if (checkSelfPermission(p) != PackageManager.PERMISSION_GRANTED) {
                missing.add(p);
            }
        }
        return missing;
    }

    private void requestPermissions() {
        List<String> missing = missingPermissions();
        if (!missing.isEmpty()) {
            requestPermissions(missing.toArray(new String[0]), REQUEST_PERMISSIONS);
        }
    }

    @Override
    public void onRequestPermissionsResult(int requestCode, String[] permissions, int[] results) {
        super.onRequestPermissionsResult(requestCode, permissions, results);
        refresh();
    }

    private void toggleSharing() {
        if (MirrorService.isRunning()) {
            startService(new Intent(this, MirrorService.class).setAction(MirrorService.ACTION_STOP));
            return;
        }
        MediaProjectionManager mpm = getSystemService(MediaProjectionManager.class);
        Intent consent = Build.VERSION.SDK_INT >= 34
                // Offer the whole screen, not a single app.
                ? mpm.createScreenCaptureIntent(MediaProjectionConfig.createConfigForDefaultDisplay())
                : mpm.createScreenCaptureIntent();
        startActivityForResult(consent, REQUEST_CAPTURE);
    }

    @Override
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        super.onActivityResult(requestCode, resultCode, data);
        if (requestCode == REQUEST_CAPTURE && resultCode == RESULT_OK && data != null) {
            Intent start = new Intent(this, MirrorService.class)
                    .setAction(MirrorService.ACTION_START)
                    .putExtra(MirrorService.EXTRA_RESULT_CODE, resultCode)
                    .putExtra(MirrorService.EXTRA_RESULT_DATA, data);
            startForegroundService(start);
        }
        refresh();
    }

    // --- MirrorService.Listener -----------------------------------------------------

    @Override
    public void onStateChanged() {
        refresh();
    }

    @Override
    public void onPairingRequest(String macName, String code) {
        if (pairingDialog != null) {
            pairingDialog.dismiss();
        }
        pairingDialog = new AlertDialog.Builder(this)
                .setTitle("Pair with " + macName + "?")
                .setMessage("Allow only if your Mac shows this code:\n\n" + code
                        + "\n\nThe Mac will be able to see and control this phone.")
                .setPositiveButton("Allow", (d, w) -> MirrorService.answerPairing(true))
                .setNegativeButton("Deny", (d, w) -> MirrorService.answerPairing(false))
                .setCancelable(false)
                .show();
    }

    @Override
    public void onPairingFinished() {
        if (pairingDialog != null) {
            pairingDialog.dismiss();
            pairingDialog = null;
        }
        refresh();
    }

    // --- small view helpers -------------------------------------------------------

    private int dp(int v) {
        return Math.round(v * getResources().getDisplayMetrics().density);
    }

    private TextView text(String s, int sp, boolean bold) {
        TextView t = new TextView(this);
        t.setText(s);
        t.setTextSize(sp);
        if (bold) {
            t.setTypeface(Typeface.DEFAULT_BOLD);
        }
        t.setPadding(0, dp(4), 0, dp(4));
        return t;
    }

    private TextView heading(String s) {
        TextView t = text(s, 18, true);
        t.setPadding(0, dp(20), 0, dp(4));
        return t;
    }

    private Button button(String label, View.OnClickListener l) {
        Button b = new Button(this);
        b.setText(label);
        b.setOnClickListener(l);
        return b;
    }
}
