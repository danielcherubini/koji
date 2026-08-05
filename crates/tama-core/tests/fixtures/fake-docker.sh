#!/usr/bin/env bash
# Fake Docker CLI for testing.
#
# Usage: copy this script to a tempdir as `docker`, chmod +x, prepend tempdir to PATH,
# and set FAKE_DOCKER_STATE_DIR to a unique temp directory per test.
#
# Supported commands:
#   docker info              → exit 0 (simulates daemon reachable)
#   docker image inspect <image> → exit 0 if image exists in state dir, exit 1 otherwise
#   docker pull <image>      → writes image to state dir, outputs JSON progress lines

set -euo pipefail

STATE_DIR="${FAKE_DOCKER_STATE_DIR:-}"
IMAGES_DIR="${STATE_DIR}/images"
CONTAINERS_DIR="${STATE_DIR}/containers"

mkdir -p "$IMAGES_DIR" "$CONTAINERS_DIR"

command="${1:-}"

case "$command" in
    info)
        # Simulate a reachable Docker daemon.
        cat <<'EOF'
{
  "ID": "fake-docker-id",
  "Containers": 0,
  "OperatingSystem": "Fake Docker Engine",
  "OSType": "linux",
  "Architecture": "x86_64"
}
EOF
        exit 0
        ;;

    image)
        subcommand="${2:-}"
        case "$subcommand" in
            inspect)
                image="${3:-}"
                if [ -n "$image" ] && [ -f "${IMAGES_DIR}/${image//\//_}" ]; then
                    cat <<EOF
[{"Id": "sha256:$(cat /dev/urandom | tr -dc 'a-f0-9' | head -c 64)", "RepoTags": ["${image}"]}]
EOF
                    exit 0
                else
                    echo "Error: No such image: ${image}" >&2
                    exit 1
                fi
                ;;
            *)
                echo "fake-docker: unknown image subcommand '${subcommand}'" >&2
                exit 1
                ;;
        esac
        ;;

    pull)
        image="${2:-}"
        if [ -z "$image" ]; then
            echo "fake-docker: missing image argument" >&2
            exit 1
        fi

        # Simulate pull progress with JSON lines.
        # Each line is a JSON object with status, id, progress, etc.
        image_file="${IMAGES_DIR}/${image//\//_}"

        echo '{"status":"Pulling from '"$image"'","id":"latest"}'
        sleep 0.1
        echo '{"status":"Downloading","progress":"10%"}'
        sleep 0.1
        echo '{"status":"Downloading","progress":"30%"}'
        sleep 0.1
        echo '{"status":"Downloading","progress":"50%"}'
        sleep 0.1
        echo '{"status":"Download complete","id":"latest"}'
        sleep 0.1
        echo '{"status":"Extracting","progress":"10%"}'
        sleep 0.1
        echo '{"status":"Extracting","progress":"50%"}'
        sleep 0.1
        echo '{"status":"Extract complete","id":"latest"}'
        sleep 0.1
        echo '{"status":"Pull complete","id":"latest"}'
        sleep 0.1
        echo '{"status":"Already exists","id":"latest"}'

        # Mark image as pulled (create state file).
        touch "$image_file"
        exit 0
        ;;

    run)
        # Simulate docker run -d. Extract container name and labels.
        container_name="tama-container"
        managed_label="false"
        for arg in "$@"; do
            if [ "$arg" = "--name" ]; then
                shift
                container_name="$1"
                continue
            fi
            if [ "$arg" = "--label" ]; then
                shift
                label_val="$1"
                case "$label_val" in
                    tama.managed=true) managed_label="true" ;;
                esac
                continue
            fi
            shift
        done

        # Generate a fake container ID (use od instead of pipe to avoid SIGPIPE with set -eo pipefail)
        container_id="$(od -An -tx1 -N6 /dev/urandom | tr -d ' \n')"
        pid=$$

        # Build labels JSON if managed label is set
        if [ "$managed_label" = "true" ]; then
            labels_json='"Labels":{"tama.managed":"true"}'
        else
            labels_json=""
        fi

        # Create container state file
        if [ -n "$labels_json" ]; then
            cat > "${CONTAINERS_DIR}/${container_id}" <<EOF
{"Id": "${container_id}", "Name": "${container_name:-tama-container}", "State": {"Running": true, "Pid": ${pid}}, "HostConfig": {"PortBindings": {"8000/tcp": [{"HostIp": "", "HostPort": "12345"}]}, "Labels": {"tama.managed": "true"}}}
EOF
        else
            cat > "${CONTAINERS_DIR}/${container_id}" <<EOF
{"Id": "${container_id}", "Name": "${container_name:-tama-container}", "State": {"Running": true, "Pid": ${pid}}, "HostConfig": {"PortBindings": {"8000/tcp": [{"HostIp": "", "HostPort": "12345"}]}}}
EOF
        fi

        # Print the container ID (what docker run outputs)
        echo "$container_id"
        exit 0
        ;;

    stop)
        # Simulate docker stop -t <timeout> <name>
        timeout=10
        name=""
        while [ $# -gt 0 ]; do
            case "$1" in
                -t) timeout="$2"; shift 2 ;;
                *) name="$1"; shift ;;
            esac
        done

        # Try to find container by name or ID
        found=0
        for state_file in "${CONTAINERS_DIR}"/*; do
            [ -f "$state_file" ] || continue
            file_name=$(grep -o '"Name": *"[^"]*"' "$state_file" 2>/dev/null | cut -d'"' -f4)
            file_id=$(grep -o '"Id": *"[^"]*"' "$state_file" 2>/dev/null | cut -d'"' -f4)
            if [ "$file_name" = "$name" ] || [ "$file_id" = "$name" ]; then
                # Update state to stopped
                sed -i 's/"Running": true/"Running": false/' "$state_file"
                echo "$name"
                found=1
                break
            fi
        done

        if [ "$found" -eq 0 ]; then
            echo "Error: No such container: ${name}" >&2
            exit 1
        fi
        exit 0
        ;;

    rm)
        # Simulate docker rm -f <name>
        name=""
        for arg in "$@"; do
            case "$arg" in
                -*) ;;
                *) name="$arg" ;;
            esac
        done

        # Try to find container by name or ID
        found=0
        for state_file in "${CONTAINERS_DIR}"/*; do
            [ -f "$state_file" ] || continue
            file_name=$(grep -o '"Name": *"[^"]*"' "$state_file" 2>/dev/null | cut -d'"' -f4)
            file_id=$(grep -o '"Id": *"[^"]*"' "$state_file" 2>/dev/null | cut -d'"' -f4)
            if [ "$file_name" = "$name" ] || [ "$file_id" = "$name" ]; then
                rm -f "$state_file"
                echo "$name"
                found=1
                break
            fi
        done

        if [ "$found" -eq 0 ]; then
            echo "Error: No such container: ${name}" >&2
            exit 1
        fi
        exit 0
        ;;

    logs)
        # Simulate docker logs -f --since <epoch> <container>
        since=""
        container=""
        while [ $# -gt 0 ]; do
            case "$1" in
                -f|--follow) shift ;;
                --since) since="$2"; shift 2 ;;
                *) container="$1"; shift ;;
            esac
        done

        # Find container state file
        for state_file in "${CONTAINERS_DIR}"/*; do
            [ -f "$state_file" ] || continue
            file_name=$(grep -o '"Name": *"[^"]*"' "$state_file" 2>/dev/null | cut -d'"' -f4)
            file_id=$(grep -o '"Id": *"[^"]*"' "$state_file" 2>/dev/null | cut -d'"' -f4)
            if [ "$file_name" = "$container" ] || [ "$file_id" = "$container" ]; then
                echo "[fake-docker] Container ${container} logs"
                echo "[fake-docker] Starting backend on port 8000"
                echo "[fake-docker] Model loaded successfully"
                exit 0
            fi
        done

        echo "Error: No such container: ${container}" >&2
        exit 1
        ;;

    ps)
        # Simulate docker ps -a --filter label=tama.managed=true --format '{{.ID}} {{.Names}}'
        show_all=0
        filter_label=""
        filter_value=""
        format=""

        while [ $# -gt 0 ]; do
            case "$1" in
                -a|-q|--all)
                    show_all=1
                    shift ;;
                --filter)
                    filter_label="$2"; shift 2 ;;
                --format)
                    format="$2"; shift 2 ;;
                *) shift ;;
            esac
        done

        # Parse label filter: "label=tama.managed=true"
        if [ -n "$filter_label" ]; then
            case "$filter_label" in
                label=*)
                    filter_value="${filter_label#label=}"
                    filter_key="${filter_value%%=*}"
                    filter_value="${filter_value#*=}" ;;
                *) filter_key=""; filter_value="" ;;
            esac
        else
            filter_key=""
            filter_value=""
        fi

        # Collect matching containers
        for state_file in "${CONTAINERS_DIR}"/*; do
            [ -f "$state_file" ] || continue
            file_name=$(grep -o '"Name": *"[^"]*"' "$state_file" 2>/dev/null | cut -d'"' -f4)
            file_id=$(grep -o '"Id": *"[^"]*"' "$state_file" 2>/dev/null | cut -d'"' -f4)

            # Check label filter if present
            if [ -n "$filter_key" ]; then
                # Check if the state file contains the label key=value pair (with optional spaces around colon)
                if ! grep -q "\"${filter_key}\": *\"${filter_value}\"" "$state_file" 2>/dev/null; then
                    continue
                fi
            fi

            # Apply format or default to "ID NAME"
            if [ "$format" = "{{.ID}} {{.Names}}" ]; then
                echo "${file_id} ${file_name}"
            elif [ "$format" = "{{.ID}}" ]; then
                echo "$file_id"
            elif [ "$format" = "{{.Names}}" ]; then
                echo "$file_name"
            else
                echo "${file_id} ${file_name}"
            fi
        done
        exit 0
        ;;

    inspect)
        # Simulate docker inspect <name>
        name="$2"

        for state_file in "${CONTAINERS_DIR}"/*; do
            [ -f "$state_file" ] || continue
            file_name=$(grep -o '"Name": *"[^"]*"' "$state_file" 2>/dev/null | cut -d'"' -f4)
            file_id=$(grep -o '"Id": *"[^"]*"' "$state_file" 2>/dev/null | cut -d'"' -f4)
            if [ "$file_name" = "$name" ] || [ "$file_id" = "$name" ]; then
                # docker inspect returns a JSON array
                printf '[%s]' "$(cat "$state_file")"
                exit 0
            fi
        done

        echo "Error: No such container: ${name}" >&2
        exit 1
        ;;

    *)
        echo "fake-docker: unknown command '${command}'" >&2
        exit 1
        ;;
esac
