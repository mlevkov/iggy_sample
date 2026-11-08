#!/bin/sh
# Docker healthcheck: healthy once the runtime reports both connectors
# running. The runtime API needs the api-key header, which the entrypoint
# writes to a file readable by this user only, so the key never appears on
# a command line.
set -eu

header="${CONNECTORS_CONFIG_DIR:-/var/lib/iggy-connectors/config}/api-key.header"
for connector in sources/webhooks sinks/gateway_inbound; do
    curl -fsS --max-time 2 -H "@$header" "http://127.0.0.1:8081/$connector" |
        grep -q '"status":"running"'
done
