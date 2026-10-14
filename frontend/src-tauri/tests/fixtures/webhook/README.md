# Webhook test certificate

`localhost-cert.der` and `localhost-key.der` are a self-signed certificate and private key used only by the loopback HTTPS receiver unit test. The certificate is valid for `127.0.0.1`; its key is intentionally public test material and must never be used outside tests.
