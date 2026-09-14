# RFC 5929 test certificates

Generated locally with `python3 generate.py` and OpenSSL 3.6.3. All fourteen DER
files are real self-signed X.509 certificates; no private keys are retained.
The generator records independent Python hashlib binding digests in `digests.txt`.
Tests include the certificates and expectations and need no OpenSSL at runtime.
RSA covers MD5/SHA-1/SHA-256/384/512, ECDSA covers SHA-256/384/512, and RSA-PSS
covers default SHA-1 plus SHA-256/384/512 and a differing MGF1 digest. Ed25519
exercises undefined channel binding. Weak signatures are parser fixtures only;
they are never trusted for production or handshake acceptance tests.
