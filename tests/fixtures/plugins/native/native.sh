#!/bin/sh
# POSIX-shell mock devices for the R10 core tests. The host runs this plugin
# with the plugin directory as its working directory.
#
#   schedule  comma-separated behavior per invoke; the last entry repeats:
#             ok | crash | garbage
#   caps      comma-separated capabilities advertised by hello
#
# The invoke counter and spawned PIDs live in files, so behavior survives a
# host restart and process reaping can be checked after shutdown.

set -u
dir=$(pwd)

schedule=ok
if [ -f "$dir/schedule" ]; then
    IFS= read -r schedule < "$dir/schedule"
fi
caps="camera.takePhoto,geolocation.getCurrentPosition,bluetooth.scan,notification.show,payment.request,share.send"
if [ -f "$dir/caps" ]; then
    IFS= read -r caps < "$dir/caps"
fi

field_count() {
    n=1
    rest=$1
    while case "$rest" in *,*) true ;; *) false ;; esac; do
        rest=${rest#*,}
        n=$((n + 1))
    done
    printf '%s' "$n"
}

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
caps_total=$(field_count "$caps")
caps_json="["
i=1
while [ "$i" -le "$caps_total" ]; do
    cap=$(field "$caps" "$i")
    if [ "$i" -eq 1 ]; then
        caps_json="$caps_json\"$cap\""
    else
        caps_json="$caps_json,\"$cap\""
    fi
    i=$((i + 1))
done
caps_json="$caps_json]"

counter() {
    n=0
    if [ -f "$dir/count" ]; then IFS= read -r n < "$dir/count"; fi
    n=$((n + 1))
    printf '%s\n' "$n" > "$dir/count"
    printf '%s' "$n"
}

printf '%s\n' "$$" > "$dir/pid"
printf '%s\n' "$$" >> "$dir/pids"
printf 'yes\n' > "$dir/started"

while IFS= read -r line; do
    id=${line#*\"id\":}
    id=${id%%,*}
    id=${id%%\}*}
    case "$id" in
    ''|*[!0-9]*) continue ;;
    esac
    case "$line" in
    *'"method":"hello"'*)
        printf '{"id":%s,"ok":true,"result":{"name":"native","version":"0.1.0","capabilities":%s}}\n' \
            "$id" "$caps_json"
        ;;
    *'"method":"invoke"'*)
        cap=${line#*\"capability\":\"}
        cap=${cap%%\"*}
        n=$(counter)
        if [ "$n" -gt "$schedule_total" ]; then n=$schedule_total; fi
        behavior=$(field "$schedule" "$n")
        case "$behavior" in
        crash) exit 7 ;;
        garbage)
            printf '{"id":%s,"ok":\n' "$id"
            continue
            ;;
        esac
        case "$cap" in
        camera.takePhoto)
            result='{"path":"/mock/camera/photo.png","width":640,"height":480,"mock":true}'
            ;;
        geolocation.getCurrentPosition)
            result='{"latitude":37.7749,"longitude":-122.4194,"accuracy":5,"mock":true}'
            ;;
        bluetooth.scan)
            result='{"devices":[{"id":"mock-beacon-1","rssi":-42}],"mock":true}'
            ;;
        notification.show)
            result='{"delivered":true,"id":"mock-notification-1","mock":true}'
            ;;
        payment.request)
            result='{"approved":true,"transaction":"mock-payment-1","mock":true}'
            ;;
        share.send)
            result='{"shared":true,"target":"mock-share-sheet","mock":true}'
            ;;
        *)
            printf '{"id":%s,"ok":false,"error":"unknown capability: %s"}\n' "$id" "$cap"
            continue
            ;;
        esac
        printf '{"id":%s,"ok":true,"result":%s}\n' "$id" "$result"
        ;;
    esac
done
