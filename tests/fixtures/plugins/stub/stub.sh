#!/bin/sh
# Stub OPUROM plugin used by the opurom-core test suite.
# Speaks the plugin protocol on stdin/stdout. The behaviour is controlled by
# files in the plugin directory (the host runs the plugin with the plugin
# directory as its working directory):
#
#   schedule  comma separated behaviour per invoke; the last entry repeats
#             ok | stall | crash | bad-id | garbage
#   caps      comma separated capabilities advertised in the hello result
#
# The invoke counter lives in `count` so a schedule survives a plugin restart.
# Only shell builtins are used so the handshake stays well below the host
# timeout even on a loaded machine.

set -u
dir=$(pwd)

schedule=ok
if [ -f "$dir/schedule" ]; then
    IFS= read -r schedule < "$dir/schedule"
fi
caps="stub.echo,stub.fail"
if [ -f "$dir/caps" ]; then
    IFS= read -r caps < "$dir/caps"
fi

# Number of comma separated fields in $1.
field_count() {
    n=1
    rest=$1
    while case "$rest" in *,*) true ;; *) false ;; esac; do
        rest=${rest#*,}
        n=$((n + 1))
    done
    printf '%s' "$n"
}

# Field $2 of the comma separated list $1.
field() {
    rest=$1
    i=1
    while [ "$i" -lt "$2" ]; do
        rest=${rest#*,}
        i=$((i + 1))
    done
    case "$rest" in
    *,*) printf '%s' "${rest%%,*}" ;;
    *) printf '%s' "$rest" ;;
    esac
}

schedule_total=$(field_count "$schedule")
last_schedule=${schedule##*,}

counter() {
    n=0
    if [ -f "$dir/count" ]; then IFS= read -r n < "$dir/count"; fi
    n=$((n + 1))
    printf '%s' "$n" > "$dir/count"
    printf '%s' "$n"
}

caps_json="["
caps_total=$(field_count "$caps")
i=1
while [ "$i" -le "$caps_total" ]; do
    cap=$(field "$caps" "$i")
    if [ -n "$cap" ]; then
        if [ "$i" -eq 1 ]; then
            caps_json="$caps_json\"$cap\""
        else
            caps_json="$caps_json,\"$cap\""
        fi
    fi
    i=$((i + 1))
done
caps_json="$caps_json]"

while IFS= read -r line; do
    id=${line#*\"id\":}
    id=${id%%,*}
    id=${id%%\}*}
    case "$id" in
    ''|*[!0-9]*) continue ;;
    esac
    case "$line" in
    *'"method":"hello"'*)
        printf '{"id":%s,"ok":true,"result":{"name":"stub","version":"0.1.0","capabilities":%s}}\n' \
            "$id" "$caps_json"
        ;;
    *'"method":"invoke"'*)
        cap=${line#*\"capability\":\"}
        cap=${cap%%\"*}
        n=$(counter)
        if [ "$n" -gt "$schedule_total" ]; then n=$schedule_total; fi
        behavior=$(field "$schedule" "$n")
        case "$behavior" in
        stall) sleep 2 ;;
        crash) exit 1 ;;
        bad-id)
            printf '{"id":999,"ok":true,"result":{}}\n'
            continue
            ;;
        garbage)
            printf 'this is not json\n'
            continue
            ;;
        esac
        case "$cap" in
        stub.fail)
            printf '{"id":%s,"ok":false,"error":"unknown capability: %s"}\n' "$id" "$cap"
            ;;
        *)
            printf '{"id":%s,"ok":true,"result":{"capability":"%s"}}\n' "$id" "$cap"
            ;;
        esac
        ;;
    esac
done
