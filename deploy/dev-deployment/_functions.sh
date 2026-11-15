# Utility functions

# Define color codes
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Function to increment IP address
increment_ip() {
  local ip=$1
  local inc=$2

  IFS='.' read -r -a octets <<< "$ip"
  for ((i=${#octets[@]}-1; i>=0; i--)); do
    octets[$i]=$((octets[$i]+inc))
    if [ "${octets[$i]}" -lt 256 ]; then
      break
    fi
    octets[$i]=0
  done
  echo "${octets[0]}.${octets[1]}.${octets[2]}.${octets[3]}"
}


# Function to echo in green
echo_green() {
  echo -e "${GREEN}$1${NC}"
}

# Function to echo in blue
echo_blue() {
  echo -e "${BLUE}$1${NC}"
}

# Function to echo in yellow
echo_yellow() {
  echo -e "${YELLOW}$1${NC}"
}

# Function to echo in red
echo_red() {
  echo -e "${RED}$1${NC}"
}

# Function to echo in green with a newline
echo_green_nl() {
  echo -e "\n${GREEN}$1${NC}"
}

# Function to echo in blue with a newline
echo_blue_nl() {
  echo -e "\n${BLUE}$1${NC}"
}

# Function to echo in yellow with a newline
echo_yellow_nl() {
  echo -e "\n${YELLOW}$1${NC}"
}

# Function to echo in red with a newline
echo_red_nl() {
  echo -e "\n${RED}$1${NC}"
}


# Function to start a color
start_color() {
  case $1 in
    "red")
      echo -ne "${RED}"
      ;;
    "green")
      echo -ne "${GREEN}"
      ;;
    "yellow")
      echo -ne "${YELLOW}"
      ;;
    "blue")
      echo -ne "${BLUE}"
      ;;
    *)
      echo -ne "${NC}"
      ;;
  esac
}

# Function to stop all colors
stop_color() {
  echo -ne "${NC}"
}

format_time() {
    local total_seconds=$1
    local hours=$((total_seconds / 3600))
    local minutes=$(( (total_seconds % 3600) / 60 ))
    local seconds=$((total_seconds % 60))

    if (( hours > 0 )); then
        printf "%d hours, %d minutes and %d seconds\n" "$hours" "$minutes" "$seconds"
    elif (( minutes > 0 )); then
        printf "%d minutes and %d seconds\n" "$minutes" "$seconds"
    else
        printf "%d seconds\n" "$seconds"
    fi
}

# Selects a Docker context that can reach the Docker daemon for the current OS.
#
# macOS (Docker Desktop) exposes the daemon via the "desktop-linux" context,
# while Linux/WSL use the "default" context.
select_docker_context() {
    local preferred=()
    case "$OSTYPE" in
        darwin*) preferred=("desktop-linux" "default") ;;
        *) preferred=("default" "desktop-linux") ;;
    esac

    local available
    available=$(docker context ls --format '{{.Name}}' 2>/dev/null || echo "")

    local ctx
    for ctx in "${preferred[@]}" $available; do
        if echo "$available" | grep -qx "$ctx" && docker --context "$ctx" info >/dev/null 2>&1; then
            docker context use "$ctx" >/dev/null
            echo_green_nl "Using Docker context '${YELLOW}${ctx}${GREEN}'."
            return 0
        fi
    done

    echo_red "Warning: Could not find a Docker context that can reach the Docker daemon."
    echo_red "Is Docker (Docker Desktop / dockerd) running? Available contexts: ${available:-none}"
    return 1
}

# Waits for a pod matching a label selector to exist, then to become Ready.
#
# "kubectl wait" fails with "no matching resources found" when the pods do not
# exist yet, so we first poll until at least one matching pod appears.
#
# Arguments:
#   $1 - namespace
#   $2 - label selector
#   $3 - timeout (optional, default: 600s)
wait_for_ready_pod() {
    local namespace="$1"
    local selector="$2"
    local timeout="${3:-600s}"

    local kube_args=()
    [ -n "${KUBECONFIG:-}" ] && kube_args+=(--kubeconfig "${KUBECONFIG}")
    [ -n "${KUBECONTEXT:-}" ] && kube_args+=(--context "${KUBECONTEXT}")

    local attempts=0
    local max_attempts=120
    until kubectl "${kube_args[@]}" --namespace "${namespace}" get pod --selector="${selector}" \
            -o name 2>/dev/null | grep -q .; do
        attempts=$((attempts + 1))
        if [ "${attempts}" -ge "${max_attempts}" ]; then
            echo_red "Timed out waiting for a pod matching '${selector}' to appear in namespace '${namespace}'."
            return 1
        fi
        sleep 1
    done

    kubectl "${kube_args[@]}" --namespace "${namespace}" wait --for=condition=Ready pod \
        --selector="${selector}" --timeout="${timeout}"
}

# Checks if all given commands are available on the system.
#
# Arguments:
#   $@ - One or more command names to check for availability.
#
# Example:
#   requirementsMet curl wget
requirementsMet() {
    local unmet=() # save all unmet requirements, so they are all checked
    local cmd
    for cmd in "$@"; do
        if ! command -v $cmd 2>&1 >/dev/null; then
            unmet+=($cmd)
        fi
    done
    if [ ${#unmet[@]} -gt 0 ]; then
        start_color red
        for cmd in "${unmet[@]}"; do
            echo "Required command '${cmd}' is not available"
        done
        stop_color
        exit 1
    fi
}
