#!/usr/bin/env bash
# Download this month's free DB-IP country lite database (CC BY 4.0) into data/.
# Never commit the result. Point the tracker at it: BF_GEOIP_DB=data/dbip-country-lite.mmdb
set -euo pipefail

cd "$(dirname "$0")/.."
mkdir -p data
month="${1:-$(date -u +%Y-%m)}"
url="https://download.db-ip.com/free/dbip-country-lite-${month}.mmdb.gz"

curl -fsSL "$url" -o data/dbip-country-lite.mmdb.gz
gunzip -f data/dbip-country-lite.mmdb.gz
echo "data/dbip-country-lite.mmdb (${month})"
