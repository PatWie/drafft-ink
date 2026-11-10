# Test fixtures

`localhost.cert.der` and `localhost.key.der` are a self-signed P-256 certificate (CN and SAN `localhost`, `127.0.0.1`, `::1`,
not a CA, valid for 100 years) and its PKCS#8 key. They exist so `tests/server.rs` can run a TLS-terminating
proxy in front of the real server and check that the client speaks `wss://` and refuses a certificate it was not told
to trust. The key protects nothing: it is public, and only the tests trust the certificate.

Regenerate with:

    openssl ecparam -name prime256v1 -genkey -noout -out k.pem
    openssl req -new -x509 -key k.pem -sha256 -days 36500 -subj "/CN=localhost" \
      -addext "subjectAltName=DNS:localhost,IP:127.0.0.1,IP:::1" -addext "basicConstraints=critical,CA:FALSE" \
      -addext "keyUsage=critical,digitalSignature" -addext "extendedKeyUsage=serverAuth" -outform DER -out localhost.cert.der
    openssl pkcs8 -topk8 -nocrypt -in k.pem -outform DER -out localhost.key.der
