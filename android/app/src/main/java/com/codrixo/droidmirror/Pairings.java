package com.codrixo.droidmirror;

import android.content.Context;
import android.content.SharedPreferences;

import java.security.MessageDigest;
import java.security.SecureRandom;
import java.util.HashSet;
import java.util.Set;
import java.util.UUID;

/**
 * Macs this phone has paired with. Each gets a random 32-byte token; only its SHA-256 is
 * stored, so the stored data can't be used to impersonate a Mac.
 */
final class Pairings {
    private static final String PREFS = "pairings";
    private static final String KEY_TOKENS = "token_hashes";
    private static final String KEY_DEVICE_ID = "device_id";

    private final SharedPreferences prefs;
    private final SecureRandom random = new SecureRandom();

    Pairings(Context context) {
        prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE);
    }

    /** Stable random id advertised to Macs (not a hardware identifier). */
    synchronized String deviceId() {
        String id = prefs.getString(KEY_DEVICE_ID, null);
        if (id == null) {
            id = UUID.randomUUID().toString();
            prefs.edit().putString(KEY_DEVICE_ID, id).apply();
        }
        return id;
    }

    /** Creates a token for a newly approved Mac. */
    synchronized byte[] add(String macName) {
        byte[] token = new byte[Protocol.TOKEN_LENGTH];
        random.nextBytes(token);
        Set<String> set = new HashSet<>(prefs.getStringSet(KEY_TOKENS, new HashSet<>()));
        set.add(hex(Protocol.sha256(token)) + "|" + macName.replace('|', ' '));
        prefs.edit().putStringSet(KEY_TOKENS, set).apply();
        return token;
    }

    synchronized boolean isKnown(byte[] token) {
        String h = hex(Protocol.sha256(token));
        for (String entry : prefs.getStringSet(KEY_TOKENS, new HashSet<>())) {
            int bar = entry.indexOf('|');
            String stored = bar < 0 ? entry : entry.substring(0, bar);
            if (MessageDigest.isEqual(stored.getBytes(), h.getBytes())) {
                return true;
            }
        }
        return false;
    }

    synchronized int count() {
        return prefs.getStringSet(KEY_TOKENS, new HashSet<>()).size();
    }

    synchronized void clear() {
        prefs.edit().remove(KEY_TOKENS).apply();
    }

    private static String hex(byte[] b) {
        StringBuilder s = new StringBuilder();
        for (byte x : b) {
            s.append(String.format("%02x", x));
        }
        return s.toString();
    }
}
