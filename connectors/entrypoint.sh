#!/bin/sh
# Render the connector runtime's configuration, then run the runtime.
#
# The templates in /etc/iggy-connectors/templates hold placeholders (a name
# in capitals between at signs) for secrets, because the runtime cannot read
# plugin secrets from the environment: its env overrides reach top-level
# plugin fields only, and the webhook endpoint's HMAC secret and the sink's
# Authorization header are nested. connectors-init generates the secrets into two volumes, mounted
# read-only here:
#
#   /run/secrets/connectors/webhook_endpoint_id   32 hex digits
#   /run/secrets/connectors/webhook_hmac_secret   64 hex digits
#   /run/secrets/connectors/sink_inbound_token    64 hex digits
#   /run/secrets/iggy/connectors_api_key          64 hex digits
#   /run/secrets/iggy/connectors_pat              read by the runtime itself
#
# Each value is checked to be lowercase hex of the expected length before it
# is substituted, so nothing in a secret file can change the TOML around it.
# The rendered files are readable by this user only.
set -eu

templates=/etc/iggy-connectors/templates
config_dir="${CONNECTORS_CONFIG_DIR:-/var/lib/iggy-connectors/config}"

fail() {
    echo "connectors-entrypoint: $*" >&2
    exit 1
}

# read_secret FILE LENGTH: print the secret in FILE after checking it is
# LENGTH lowercase hex digits. Never prints the value on error.
read_secret() {
    [ -r "$1" ] || fail "cannot read $1 (connectors-init writes it)"
    value=$(tr -d '\n' <"$1")
    case "$value" in
        "" | *[!0-9a-f]*) fail "$1 is not lowercase hex" ;;
    esac
    [ "${#value}" -eq "$2" ] || fail "$1 must hold $2 hex digits, not ${#value}"
    printf '%s' "$value"
}

webhook_endpoint_id=$(read_secret /run/secrets/connectors/webhook_endpoint_id 32)
webhook_hmac_secret=$(read_secret /run/secrets/connectors/webhook_hmac_secret 64)
sink_inbound_token=$(read_secret /run/secrets/connectors/sink_inbound_token 64)
runtime_api_key=$(read_secret /run/secrets/iggy/connectors_api_key 64)
[ -r /run/secrets/iggy/connectors_pat ] ||
    fail "cannot read /run/secrets/iggy/connectors_pat (connectors-init writes it)"

case "$config_dir" in
    /*) ;;
    *) fail "CONNECTORS_CONFIG_DIR must be an absolute path" ;;
esac
case "$config_dir" in
    *[!A-Za-z0-9/._-]*) fail "CONNECTORS_CONFIG_DIR may hold only letters, digits and / . _ -" ;;
esac

# Files are overwritten, never removed: a container starts from the image,
# so nothing stale can be there, and a mistyped CONNECTORS_CONFIG_DIR must
# not delete anything.
umask 077
mkdir -p "$config_dir/connectors"
chmod 0700 "$config_dir" "$config_dir/connectors"

render() {
    sed -e "s|@WEBHOOK_ENDPOINT_ID@|$webhook_endpoint_id|g" \
        -e "s|@WEBHOOK_HMAC_SECRET@|$webhook_hmac_secret|g" \
        -e "s|@SINK_INBOUND_TOKEN@|$sink_inbound_token|g" \
        -e "s|@RUNTIME_API_KEY@|$runtime_api_key|g" \
        -e "s|@CONFIG_DIR@|$config_dir|g" \
        "$1" >"$2"
}

render "$templates/config.toml" "$config_dir/config.toml"
for template in "$templates"/connectors/*.toml; do
    render "$template" "$config_dir/connectors/$(basename "$template")"
done

# The healthcheck's api-key header (connectors/healthcheck.sh)
printf 'api-key: %s\n' "$runtime_api_key" >"$config_dir/api-key.header"

if grep -l '@[A-Z_]*@' "$config_dir/config.toml" "$config_dir"/connectors/*.toml >/dev/null; then
    fail "a template placeholder was left unrendered"
fi

echo "connectors-entrypoint: configuration rendered to $config_dir"
export IGGY_CONNECTORS_CONFIG_PATH="$config_dir/config.toml"
exec iggy-connectors "$@"
