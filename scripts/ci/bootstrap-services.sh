#!/usr/bin/env bash
# Ephemeral Linux CI fixtures only. Never point these commands at shared services.
set -euo pipefail
if [[ ${1:-} == --help ]]; then
  echo 'Usage: bootstrap-services.sh (Linux Docker; POSTGRES_CONTAINER, MYSQL_CONTAINER, REDIS_CONTAINER, MONGO_CONTAINER, GITHUB_ENV required)'
  exit 0
fi
: "${POSTGRES_CONTAINER:?}" "${MYSQL_CONTAINER:?}" "${REDIS_CONTAINER:?}" "${MONGO_CONTAINER:?}" "${GITHUB_ENV:?}"
script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
fixture_dir="$script_dir/../../.tools/services"
mkdir -p "$fixture_dir"

# Explicit SCRAM user, not trust authentication.
docker exec "$POSTGRES_CONTAINER" psql -U turnloop -d turnloop -v ON_ERROR_STOP=1 -c "SET password_encryption='scram-sha-256'; ALTER ROLE turnloop PASSWORD 'turnloop';"

# MySQL 9 caching_sha2_password over authenticated TLS. Test CA has localhost SANs.
openssl req -x509 -newkey rsa:2048 -nodes -days 2 -subj '/CN=turnloop CI CA' \
  -keyout "$fixture_dir/ca-key.pem" -out "$fixture_dir/ca.pem"
openssl req -newkey rsa:2048 -nodes -subj '/CN=localhost' \
  -keyout "$fixture_dir/server-key.pem" -out "$fixture_dir/server.csr"
printf '%s\n' 'subjectAltName=DNS:localhost,IP:127.0.0.1' 'extendedKeyUsage=serverAuth' > "$fixture_dir/extensions.cnf"
openssl x509 -req -in "$fixture_dir/server.csr" -CA "$fixture_dir/ca.pem" \
  -CAkey "$fixture_dir/ca-key.pem" -CAcreateserial -days 2 \
  -extfile "$fixture_dir/extensions.cnf" -out "$fixture_dir/server-cert.pem"
for file in ca.pem server-key.pem server-cert.pem; do
  docker cp "$fixture_dir/$file" "$MYSQL_CONTAINER:/var/lib/mysql/$file"
done
docker exec --user root "$MYSQL_CONTAINER" sh -c 'chown mysql:mysql /var/lib/mysql/ca.pem /var/lib/mysql/server-key.pem /var/lib/mysql/server-cert.pem; chmod 600 /var/lib/mysql/server-key.pem'
docker restart "$MYSQL_CONTAINER"
ready=false
for _ in {1..90}; do
  if docker exec -e MYSQL_PWD=turnloop-root "$MYSQL_CONTAINER" mysqladmin ping -uroot --silent; then
    ready=true
    break
  fi
  sleep 1
done
[[ $ready == true ]] || { echo 'MySQL did not restart' >&2; exit 1; }
docker exec -e MYSQL_PWD=turnloop-root "$MYSQL_CONTAINER" mysql -uroot -e \
  "ALTER USER 'turnloop'@'%' IDENTIFIED WITH caching_sha2_password BY 'turnloop' REQUIRE SSL; SET GLOBAL require_secure_transport=ON;"
docker exec -e MYSQL_PWD=turnloop "$MYSQL_CONTAINER" mysql -h127.0.0.1 -uturnloop --ssl-mode=VERIFY_IDENTITY --ssl-ca=/var/lib/mysql/ca.pem -e 'SELECT 1; SHOW SESSION STATUS LIKE "Ssl_cipher";'

bash "$script_dir/bootstrap-redis-cluster.sh"
bash "$script_dir/bootstrap-mongo-replica.sh"
cat >> "$GITHUB_ENV" <<ENV
TURNLOOP_TEST_REQUIRED=1
TURNLOOP_TEST_POSTGRES_URL=postgres://turnloop:turnloop@127.0.0.1:5432/turnloop
TURNLOOP_TEST_MYSQL_URL=mysql://turnloop:turnloop@localhost:3306/turnloop
TURNLOOP_TEST_MYSQL_TLS_CA=$fixture_dir/ca.pem
TURNLOOP_TEST_MYSQL_TLS_SERVER_NAME=localhost
TURNLOOP_TEST_MYSQL_AUTH_PLUGIN=caching_sha2_password
TURNLOOP_TEST_REDIS_URL=redis://127.0.0.1:6379
TURNLOOP_TEST_MONGODB_URL=mongodb://turnloop:turnloop@127.0.0.1:27017/turnloop?authSource=admin
ENV
# Real server operations prove bootstrap subjects ran.
docker exec "$REDIS_CONTAINER" redis-cli SET turnloop:ci:probe ready
docker exec "$REDIS_CONTAINER" redis-cli GET turnloop:ci:probe | rg '^ready$'
docker exec "$MONGO_CONTAINER" mongosh --quiet -u turnloop -p turnloop --authenticationDatabase admin --eval 'const c=db.getSiblingDB("turnloop").ci_probe; c.insertOne({probe:1}); if(c.countDocuments({probe:1})<1) throw Error("no Mongo write");'
