package com.codrixo.droidmirror;

import android.security.keystore.KeyGenParameterSpec;
import android.security.keystore.KeyProperties;

import java.net.Socket;
import java.security.KeyPairGenerator;
import java.security.KeyStore;
import java.security.Principal;
import java.security.PrivateKey;
import java.security.cert.X509Certificate;
import java.security.spec.ECGenParameterSpec;

import javax.net.ssl.KeyManager;
import javax.net.ssl.SSLContext;
import javax.net.ssl.SSLEngine;
import javax.net.ssl.SSLServerSocket;
import javax.net.ssl.X509ExtendedKeyManager;
import javax.security.auth.x500.X500Principal;

/**
 * TLS server identity: an EC P-256 key generated inside the Android Keystore (it never leaves
 * the secure hardware) with a self-signed certificate. The Mac pins the certificate's SHA-256.
 */
final class TlsServer {
    private static final String ALIAS = "droidmirror-tls";

    final SSLContext context;
    final byte[] fingerprint;

    private TlsServer(SSLContext context, byte[] fingerprint) {
        this.context = context;
        this.fingerprint = fingerprint;
    }

    static TlsServer load() throws Exception {
        KeyStore ks = KeyStore.getInstance("AndroidKeyStore");
        ks.load(null);
        if (!ks.containsAlias(ALIAS)) {
            KeyPairGenerator g = KeyPairGenerator.getInstance(KeyProperties.KEY_ALGORITHM_EC, "AndroidKeyStore");
            g.initialize(new KeyGenParameterSpec.Builder(ALIAS, KeyProperties.PURPOSE_SIGN)
                    .setAlgorithmParameterSpec(new ECGenParameterSpec("secp256r1"))
                    .setDigests(KeyProperties.DIGEST_NONE, KeyProperties.DIGEST_SHA256,
                            KeyProperties.DIGEST_SHA384, KeyProperties.DIGEST_SHA512)
                    .setCertificateSubject(new X500Principal("CN=DroidMirror"))
                    .build());
            g.generateKeyPair();
        }
        PrivateKey key = (PrivateKey) ks.getKey(ALIAS, null);
        X509Certificate cert = (X509Certificate) ks.getCertificate(ALIAS);

        SSLContext ctx = SSLContext.getInstance("TLS");
        ctx.init(new KeyManager[] {new SingleKeyManager(key, cert)}, null, null);
        return new TlsServer(ctx, Protocol.sha256(cert.getEncoded()));
    }

    SSLServerSocket listen() throws Exception {
        SSLServerSocket s = (SSLServerSocket) context.getServerSocketFactory().createServerSocket(0);
        s.setEnabledProtocols(new String[] {"TLSv1.2", "TLSv1.3"});
        return s;
    }

    /** Serves the one Keystore key for EC cipher suites. */
    private static final class SingleKeyManager extends X509ExtendedKeyManager {
        private final PrivateKey key;
        private final X509Certificate cert;

        SingleKeyManager(PrivateKey key, X509Certificate cert) {
            this.key = key;
            this.cert = cert;
        }

        private static String aliasFor(String keyType) {
            return keyType != null && keyType.startsWith("EC") ? ALIAS : null;
        }

        @Override
        public String chooseServerAlias(String keyType, Principal[] issuers, Socket socket) {
            return aliasFor(keyType);
        }

        @Override
        public String chooseEngineServerAlias(String keyType, Principal[] issuers, SSLEngine engine) {
            return aliasFor(keyType);
        }

        @Override
        public String[] getServerAliases(String keyType, Principal[] issuers) {
            String a = aliasFor(keyType);
            return a == null ? null : new String[] {a};
        }

        @Override
        public X509Certificate[] getCertificateChain(String alias) {
            return ALIAS.equals(alias) ? new X509Certificate[] {cert} : null;
        }

        @Override
        public PrivateKey getPrivateKey(String alias) {
            return ALIAS.equals(alias) ? key : null;
        }

        @Override
        public String[] getClientAliases(String keyType, Principal[] issuers) {
            return null;
        }

        @Override
        public String chooseClientAlias(String[] keyType, Principal[] issuers, Socket socket) {
            return null;
        }
    }
}
