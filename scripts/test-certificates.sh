#!/bin/sh
# Local-only test CA and server leaf. These committed keys are public test data.
set -eu
cd "$(dirname "$0")/.."
cert_dir=protocols/turnloop-smtp/tests/fixtures
mkdir -p "$cert_dir" .tools/certificates
openssl req -x509 -newkey rsa:2048 -nodes -keyout .tools/certificates/ca-key.pem -out "$cert_dir/ca.pem" -days 3650 -subj '/CN=turnloop test CA' -addext 'basicConstraints=critical,CA:TRUE' -addext 'keyUsage=critical,keyCertSign,cRLSign'
openssl req -newkey rsa:2048 -nodes -keyout "$cert_dir/server-key.pem" -out .tools/certificates/server.csr -subj '/CN=localhost'
cat > .tools/certificates/extensions <<'EXT'
basicConstraints=critical,CA:FALSE
keyUsage=critical,digitalSignature,keyEncipherment
extendedKeyUsage=serverAuth
subjectAltName=DNS:localhost,IP:127.0.0.1
EXT
openssl x509 -req -in .tools/certificates/server.csr -CA "$cert_dir/ca.pem" -CAkey .tools/certificates/ca-key.pem -CAcreateserial -CAserial .tools/certificates/ca.srl -out "$cert_dir/server.pem" -days 3650 -extfile .tools/certificates/extensions
openssl x509 -in "$cert_dir/ca.pem" -outform DER -out "$cert_dir/ca.der"
openssl x509 -in "$cert_dir/server.pem" -outform DER -out "$cert_dir/server.der"
openssl pkcs8 -topk8 -nocrypt -in "$cert_dir/server-key.pem" -outform DER -out "$cert_dir/server-key.der"
