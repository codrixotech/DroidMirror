package com.codrixo.droidmirror;

import android.Manifest;
import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.app.Service;
import android.content.Context;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.content.pm.ServiceInfo;
import android.hardware.display.DisplayManager;
import android.hardware.display.VirtualDisplay;
import android.media.projection.MediaProjection;
import android.media.projection.MediaProjectionManager;
import android.net.nsd.NsdManager;
import android.net.nsd.NsdServiceInfo;
import android.os.Build;
import android.os.Handler;
import android.os.IBinder;
import android.os.Looper;
import android.provider.Settings;
import android.telecom.TelecomManager;
import android.telephony.TelephonyCallback;
import android.telephony.TelephonyManager;
import android.util.DisplayMetrics;
import android.util.Log;
import android.view.Display;

import java.io.BufferedInputStream;
import java.io.BufferedOutputStream;
import java.io.DataInputStream;
import java.io.IOException;
import java.io.OutputStream;
import java.net.Socket;
import java.nio.charset.StandardCharsets;
import java.util.Arrays;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.TimeUnit;

import javax.net.ssl.SSLServerSocket;
import javax.net.ssl.SSLSocket;

/**
 * Foreground service that shares the screen with paired Macs while the user has started
 * sharing. Holds the MediaProjection, runs the TLS server, advertises it on the local
 * network, and serves the video / control / events / pairing connections.
 */
public final class MirrorService extends Service {
    private static final String TAG = "DroidMirror";

    static final String ACTION_START = "com.codrixo.droidmirror.START";
    static final String ACTION_STOP = "com.codrixo.droidmirror.STOP";
    static final String ACTION_ALLOW = "com.codrixo.droidmirror.ALLOW";
    static final String ACTION_DENY = "com.codrixo.droidmirror.DENY";
    static final String EXTRA_RESULT_CODE = "result_code";
    static final String EXTRA_RESULT_DATA = "result_data";

    static final String SERVICE_TYPE = "_droidmirror._tcp";
    private static final String CHANNEL_SHARING = "sharing";
    private static final String CHANNEL_PAIRING = "pairing";
    private static final int NOTIFICATION_SHARING = 1;
    private static final int NOTIFICATION_PAIRING = 2;

    /** UI callbacks (MainActivity), called on the main thread. */
    interface Listener {
        void onStateChanged();

        void onPairingRequest(String macName, String code);

        void onPairingFinished();
    }

    private static volatile MirrorService instance;
    private static volatile Listener listener;
    private static volatile String status = "";

    private final Handler main = new Handler(Looper.getMainLooper());
    private final ExecutorService connections = Executors.newCachedThreadPool();
    private final Object sessionLock = new Object();

    private Pairings pairings;
    private TlsServer tls;
    private MediaProjection projection;
    private VirtualDisplay display;
    private SSLServerSocket server;
    private NsdManager.RegistrationListener nsdListener;
    private DisplayManager.DisplayListener displayListener;
    private TelephonyCallback telephonyCallback;
    private int lastWidth;
    private int lastHeight;

    // Current session (one Mac at a time), guarded by sessionLock.
    private Socket videoSocket;
    private ScreenEncoder encoder;
    private Socket eventsSocket;
    private OutputStream eventsOut;
    private volatile int callState;

    // Pending pairing request, at most one.
    private PendingPair pendingPair;

    private static final class PendingPair {
        final CountDownLatch decided = new CountDownLatch(1);
        volatile boolean allowed;
    }

    static boolean isRunning() {
        return instance != null;
    }

    static String status() {
        return status;
    }

    static void setListener(Listener l) {
        listener = l;
    }

    /** From the in-app dialog. */
    static void answerPairing(boolean allow) {
        MirrorService s = instance;
        if (s != null) {
            s.decide(allow);
        }
    }

    @Override
    public IBinder onBind(Intent intent) {
        return null;
    }

    @Override
    public int onStartCommand(Intent intent, int flags, int startId) {
        String action = intent == null ? null : intent.getAction();
        if (ACTION_START.equals(action) && instance == null) {
            start(intent);
        } else if (ACTION_STOP.equals(action)) {
            stopSharing();
        } else if (ACTION_ALLOW.equals(action) || ACTION_DENY.equals(action)) {
            decide(ACTION_ALLOW.equals(action));
        }
        return START_NOT_STICKY;
    }

    @Override
    public void onDestroy() {
        stopSharing();
        connections.shutdownNow();
        super.onDestroy();
    }

    // --- lifecycle ---------------------------------------------------------------

    private void start(Intent intent) {
        createChannels();
        // Android 14+: the foreground service must run before getMediaProjection().
        startForeground(NOTIFICATION_SHARING, sharingNotification(),
                ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PROJECTION);
        int code = intent.getIntExtra(EXTRA_RESULT_CODE, 0);
        Intent data = Build.VERSION.SDK_INT >= 33
                ? intent.getParcelableExtra(EXTRA_RESULT_DATA, Intent.class)
                : legacyIntentExtra(intent);
        MediaProjectionManager mpm = getSystemService(MediaProjectionManager.class);
        projection = data == null ? null : mpm.getMediaProjection(code, data);
        if (projection == null) {
            Log.e(TAG, "screen capture permission missing");
            stopSelf();
            return;
        }
        // Must be registered before createVirtualDisplay (Android 14+).
        projection.registerCallback(new MediaProjection.Callback() {
            @Override
            public void onStop() {
                main.post(MirrorService.this::stopSharing);
            }
        }, main);

        DisplayMetrics m = ScreenEncoder.realMetrics(this);
        int[] size = Protocol.encodedSize(m.widthPixels, m.heightPixels, ScreenEncoder.MAX_SIZE);
        lastWidth = m.widthPixels;
        lastHeight = m.heightPixels;
        // One virtual display per projection (Android 14+); the encoder resizes it.
        display = projection.createVirtualDisplay("DroidMirror", size[0], size[1], m.densityDpi,
                DisplayManager.VIRTUAL_DISPLAY_FLAG_AUTO_MIRROR, null, null, null);

        pairings = new Pairings(this);
        instance = this;
        connections.execute(this::serve);
        watchRotation();
        watchCalls();
        setStatus("Starting…");
    }

    private void stopSharing() {
        if (instance != this) {
            return;
        }
        instance = null;
        closeSession();
        decide(false);
        unregisterNsd();
        closeQuietly(server);
        server = null;
        if (displayListener != null) {
            getSystemService(DisplayManager.class).unregisterDisplayListener(displayListener);
            displayListener = null;
        }
        if (telephonyCallback != null) {
            getSystemService(TelephonyManager.class).unregisterTelephonyCallback(telephonyCallback);
            telephonyCallback = null;
        }
        if (display != null) {
            display.release();
            display = null;
        }
        if (projection != null) {
            projection.stop();
            projection = null;
        }
        stopForeground(STOP_FOREGROUND_REMOVE);
        setStatus("");
        stopSelf();
    }

    private void setStatus(String s) {
        status = s;
        main.post(() -> {
            Listener l = listener;
            if (l != null) {
                l.onStateChanged();
            }
        });
    }

    // --- network -------------------------------------------------------------------

    private void serve() {
        try {
            tls = TlsServer.load();
            server = tls.listen();
        } catch (Exception e) {
            Log.e(TAG, "could not start the secure server", e);
            setStatus("Could not start: " + e.getMessage());
            return;
        }
        registerNsd(server.getLocalPort());
        while (instance == this && server != null && !server.isClosed()) {
            try {
                SSLSocket s = (SSLSocket) server.accept();
                connections.execute(() -> handle(s));
            } catch (IOException e) {
                break; // closed
            }
        }
    }

    private void registerNsd(int port) {
        NsdServiceInfo info = new NsdServiceInfo();
        info.setServiceName(deviceDisplayName());
        info.setServiceType(SERVICE_TYPE);
        info.setPort(port);
        info.setAttribute("id", pairings.deviceId());
        nsdListener = new NsdManager.RegistrationListener() {
            @Override
            public void onServiceRegistered(NsdServiceInfo i) {
                setStatus("Visible to Macs on this Wi-Fi as \"" + i.getServiceName() + "\"");
            }

            @Override
            public void onRegistrationFailed(NsdServiceInfo i, int error) {
                setStatus("Could not advertise on the network (error " + error + ")");
            }

            @Override
            public void onServiceUnregistered(NsdServiceInfo i) {}

            @Override
            public void onUnregistrationFailed(NsdServiceInfo i, int error) {}
        };
        getSystemService(NsdManager.class).registerService(info, NsdManager.PROTOCOL_DNS_SD, nsdListener);
    }

    private void unregisterNsd() {
        if (nsdListener != null) {
            try {
                getSystemService(NsdManager.class).unregisterService(nsdListener);
            } catch (IllegalArgumentException ignored) {
                // not registered
            }
            nsdListener = null;
        }
    }

    private String deviceDisplayName() {
        String name = Settings.Global.getString(getContentResolver(), Settings.Global.DEVICE_NAME);
        if (name == null || name.isEmpty()) {
            name = Build.MANUFACTURER + " " + Build.MODEL;
        }
        return name;
    }

    private void handle(SSLSocket s) {
        try {
            s.setTcpNoDelay(true);
            s.setSoTimeout(10_000);
            s.startHandshake();
            DataInputStream in = new DataInputStream(new BufferedInputStream(s.getInputStream()));
            OutputStream out = new BufferedOutputStream(s.getOutputStream());
            byte[] magic = new byte[4];
            in.readFully(magic);
            if (!Arrays.equals(magic, Protocol.MAGIC)) {
                throw new IOException("bad hello");
            }
            int channel = in.readUnsignedByte();
            if (channel == Protocol.CHANNEL_PAIR) {
                handlePair(in, out);
                return;
            }
            byte[] token = new byte[Protocol.TOKEN_LENGTH];
            in.readFully(token);
            if (!pairings.isKnown(token)) {
                out.write(Protocol.STATUS_UNKNOWN_TOKEN);
                out.flush();
                return;
            }
            s.setSoTimeout(0);
            switch (channel) {
                case Protocol.CHANNEL_VIDEO:
                    handleVideo(s, out);
                    break;
                case Protocol.CHANNEL_CONTROL:
                    out.write(Protocol.STATUS_OK);
                    out.flush();
                    handleControl(in);
                    break;
                case Protocol.CHANNEL_EVENTS:
                    handleEvents(s, in, out);
                    break;
                default:
                    throw new IOException("unknown channel " + channel);
            }
        } catch (IOException e) {
            Log.d(TAG, "connection ended: " + e.getMessage());
        } finally {
            closeQuietly(s);
        }
    }

    private void handlePair(DataInputStream in, OutputStream out) throws IOException {
        byte[] nonce = new byte[Protocol.NONCE_LENGTH];
        in.readFully(nonce);
        byte[] name = new byte[in.readUnsignedByte()];
        in.readFully(name);
        String macName = new String(name, StandardCharsets.UTF_8);
        String code = Protocol.formatCode(Protocol.pairingCode(tls.fingerprint, nonce));

        PendingPair p;
        synchronized (this) {
            if (pendingPair != null) {
                out.write(Protocol.STATUS_DENIED);
                out.flush();
                return;
            }
            p = pendingPair = new PendingPair();
        }
        showPairingRequest(macName, code);
        boolean allowed;
        try {
            allowed = p.decided.await(70, TimeUnit.SECONDS) && p.allowed;
        } catch (InterruptedException e) {
            allowed = false;
        }
        synchronized (this) {
            pendingPair = null;
        }
        hidePairingRequest();
        if (!allowed) {
            out.write(Protocol.STATUS_DENIED);
            out.flush();
            return;
        }
        byte[] token = pairings.add(macName);
        byte[] id = pairings.deviceId().getBytes(StandardCharsets.UTF_8);
        out.write(Protocol.STATUS_OK);
        out.write(token);
        out.write(id.length);
        out.write(id);
        out.flush();
        setStatus("Paired with " + macName);
    }

    private void decide(boolean allow) {
        PendingPair p;
        synchronized (this) {
            p = pendingPair;
        }
        if (p != null) {
            p.allowed = allow;
            p.decided.countDown();
        }
    }

    private void handleVideo(Socket s, OutputStream out) throws IOException {
        VirtualDisplay d = display;
        if (d == null) {
            out.write(Protocol.STATUS_NOT_SHARING);
            out.flush();
            return;
        }
        ScreenEncoder e = new ScreenEncoder(this, d, out);
        synchronized (sessionLock) {
            closeSession(); // one Mac at a time: the newest wins
            videoSocket = s;
            encoder = e;
        }
        out.write(Protocol.STATUS_OK);
        out.write(Protocol.deviceNameField(Build.MODEL));
        out.flush();
        setStatus("Mirroring to your Mac");
        try {
            e.run();
        } finally {
            synchronized (sessionLock) {
                if (encoder == e) {
                    encoder = null;
                    videoSocket = null;
                }
            }
            if (instance == this) {
                setStatus("Visible to Macs on this Wi-Fi");
            }
        }
    }

    private void handleControl(DataInputStream in) throws IOException {
        long lastWarning = 0;
        int received = 0;
        while (true) {
            ControlMessage m = ControlMessage.read(in);
            if (received++ == 0) {
                Log.i(TAG, "control channel: receiving input from the Mac");
            }
            if (m.type == ControlMessage.TYPE_RESET_VIDEO) {
                ScreenEncoder e = encoder;
                if (e != null) {
                    e.requestKeyFrame();
                }
            } else if (m.type == ControlMessage.TYPE_INJECT_KEYCODE && (m.keycode == 5 || m.keycode == 6)) {
                if (m.action == ControlMessage.ACTION_DOWN) {
                    callKey(m.keycode == 5);
                }
            } else {
                ControlService c = ControlService.get();
                if (c != null) {
                    c.handle(m);
                } else {
                    long now = android.os.SystemClock.uptimeMillis();
                    if (now - lastWarning > 5000) {
                        lastWarning = now;
                        Log.w(TAG, "input ignored: the DroidMirror Accessibility service isn't running");
                        sendEvent(Protocol.controlUnavailableMessage());
                    }
                }
            }
        }
    }

    private void handleEvents(Socket s, DataInputStream in, OutputStream out) throws IOException {
        synchronized (sessionLock) {
            closeQuietly(eventsSocket);
            eventsSocket = s;
            eventsOut = out;
            out.write(Protocol.STATUS_OK);
            out.write(Protocol.callStateMessage(callState, ""));
            out.flush();
        }
        // Nothing is expected from the Mac; this detects when it goes away.
        while (in.read() >= 0) {
            // ignore
        }
        synchronized (sessionLock) {
            if (eventsSocket == s) {
                eventsSocket = null;
                eventsOut = null;
            }
        }
    }

    private void closeSession() {
        synchronized (sessionLock) {
            if (encoder != null) {
                encoder.stop();
                encoder = null;
            }
            closeQuietly(videoSocket);
            videoSocket = null;
            closeQuietly(eventsSocket);
            eventsSocket = null;
            eventsOut = null;
        }
    }

    // --- rotation and calls ------------------------------------------------------------

    private void watchRotation() {
        displayListener = new DisplayManager.DisplayListener() {
            @Override
            public void onDisplayChanged(int displayId) {
                if (displayId != Display.DEFAULT_DISPLAY) {
                    return;
                }
                DisplayMetrics m = ScreenEncoder.realMetrics(MirrorService.this);
                if (m.widthPixels != lastWidth || m.heightPixels != lastHeight) {
                    lastWidth = m.widthPixels;
                    lastHeight = m.heightPixels;
                    ScreenEncoder e = encoder;
                    if (e != null) {
                        e.requestRestart();
                    }
                }
            }

            @Override
            public void onDisplayAdded(int displayId) {}

            @Override
            public void onDisplayRemoved(int displayId) {}
        };
        getSystemService(DisplayManager.class).registerDisplayListener(displayListener, main);
    }

    private void watchCalls() {
        if (checkSelfPermission(Manifest.permission.READ_PHONE_STATE) != PackageManager.PERMISSION_GRANTED) {
            Log.i(TAG, "phone permission not granted: no call alerts");
            return;
        }
        final class CallListener extends TelephonyCallback implements TelephonyCallback.CallStateListener {
            @Override
            public void onCallStateChanged(int state) {
                int s = state == TelephonyManager.CALL_STATE_RINGING ? 1
                        : state == TelephonyManager.CALL_STATE_OFFHOOK ? 2 : 0;
                callState = s;
                sendEvent(Protocol.callStateMessage(s, ""));
            }
        }
        telephonyCallback = new CallListener();
        getSystemService(TelephonyManager.class).registerTelephonyCallback(getMainExecutor(), telephonyCallback);
    }

    private void sendEvent(byte[] message) {
        connections.execute(() -> {
            synchronized (sessionLock) {
                if (eventsOut == null) {
                    return;
                }
                try {
                    eventsOut.write(message);
                    eventsOut.flush();
                } catch (IOException e) {
                    closeQuietly(eventsSocket);
                    eventsSocket = null;
                    eventsOut = null;
                }
            }
        });
    }

    /** Answer (true) or decline/end (false) through the official telecom API. */
    @SuppressWarnings("deprecation") // still the API for apps that aren't the dialer
    private void callKey(boolean answer) {
        if (checkSelfPermission(Manifest.permission.ANSWER_PHONE_CALLS) != PackageManager.PERMISSION_GRANTED) {
            Log.w(TAG, "phone call permission not granted");
            return;
        }
        TelecomManager tm = getSystemService(TelecomManager.class);
        try {
            if (answer) {
                tm.acceptRingingCall();
            } else {
                tm.endCall();
            }
        } catch (SecurityException e) {
            Log.w(TAG, "call action refused", e);
        }
    }

    // --- notifications --------------------------------------------------------------

    private void createChannels() {
        NotificationManager nm = getSystemService(NotificationManager.class);
        nm.createNotificationChannel(new NotificationChannel(CHANNEL_SHARING, "Screen sharing",
                NotificationManager.IMPORTANCE_LOW));
        NotificationChannel pairing = new NotificationChannel(CHANNEL_PAIRING, "Pairing requests",
                NotificationManager.IMPORTANCE_HIGH);
        nm.createNotificationChannel(pairing);
    }

    private PendingIntent serviceIntent(String action, int request) {
        Intent i = new Intent(this, MirrorService.class).setAction(action);
        return PendingIntent.getService(this, request, i, PendingIntent.FLAG_IMMUTABLE);
    }

    private Notification sharingNotification() {
        PendingIntent open = PendingIntent.getActivity(this, 0,
                new Intent(this, MainActivity.class), PendingIntent.FLAG_IMMUTABLE);
        return new Notification.Builder(this, CHANNEL_SHARING)
                .setSmallIcon(R.drawable.ic_notification)
                .setContentTitle("Sharing screen with your Mac")
                .setContentText("Paired Macs on this Wi-Fi can see and control this phone.")
                .setContentIntent(open)
                .setOngoing(true)
                .addAction(new Notification.Action.Builder(null, "Stop", serviceIntent(ACTION_STOP, 1)).build())
                .build();
    }

    private void showPairingRequest(String macName, String code) {
        Notification n = new Notification.Builder(this, CHANNEL_PAIRING)
                .setSmallIcon(R.drawable.ic_notification)
                .setContentTitle("Pair with " + macName + "?")
                .setContentText("Allow only if your Mac shows " + code)
                .setStyle(new Notification.BigTextStyle().bigText(
                        "Allow only if your Mac shows " + code + ". It will be able to see and control this phone."))
                .setCategory(Notification.CATEGORY_CALL)
                .setAutoCancel(true)
                .addAction(new Notification.Action.Builder(null, "Deny", serviceIntent(ACTION_DENY, 2)).build())
                .addAction(new Notification.Action.Builder(null, "Allow", serviceIntent(ACTION_ALLOW, 3)).build())
                .build();
        getSystemService(NotificationManager.class).notify(NOTIFICATION_PAIRING, n);
        main.post(() -> {
            Listener l = listener;
            if (l != null) {
                l.onPairingRequest(macName, code);
            }
        });
    }

    private void hidePairingRequest() {
        getSystemService(NotificationManager.class).cancel(NOTIFICATION_PAIRING);
        main.post(() -> {
            Listener l = listener;
            if (l != null) {
                l.onPairingFinished();
            }
        });
    }

    private static void closeQuietly(java.io.Closeable c) {
        if (c != null) {
            try {
                c.close();
            } catch (IOException ignored) {
                // closing anyway
            }
        }
    }

    @SuppressWarnings("deprecation") // the typed variant needs Android 13
    private static Intent legacyIntentExtra(Intent intent) {
        return intent.getParcelableExtra(EXTRA_RESULT_DATA);
    }

    /** Count of paired Macs, for the UI. */
    static int pairedCount(Context context) {
        return new Pairings(context).count();
    }

    static void forgetAll(Context context) {
        new Pairings(context).clear();
    }
}
