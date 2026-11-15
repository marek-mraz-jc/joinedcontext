#!/bin/bash

#
# This work is licensed under [EU PL 1.2]
# (https://gitlab.com/civitas-connect/civitas-core/civitas-core/-/blob/main/LICENSE)
# by Civitas Connect e. V., Hafenweg 7, 48155 Münster, Germany, and [other authors]
# (https://gitlab.com/civitas-connect/civitas-core/civitas-core/-/blob/main/AUTHORS-ATTRIBUTION.md).
#
# You may not use this work except in compliance with the Licence.
#

cd "$(dirname "$0")"

set -e
. _functions.sh
set -a

# check whether all needed requirements are met
requirementsMet helm docker kubectl
for arg in "$@"; do
  if [[ "$arg" == "k3s" || "$arg" == "-k" ]]; then
    requirementsMet k3d
  fi
  if [[ "$arg" == "minikube" || "$arg" == "-m" ]]; then
    requirementsMet minikube
  fi
done

# SECONDS is a special shell variable that tracks the number of seconds since the script started
SECONDS=0

# If you change the tld, you need to recreate the root certificate
: ${DOMAIN:=joinedcontext.test} # Set DOMAIN if not already set
export DOMAIN=${DOMAIN}

# Function to show help
show_help() {
  start_color "yellow"
  echo
  echo "joinedcontext platform local deployment base installation"
  echo "==============================================="
  echo
  start_color "green"
  echo "This script helps deploying the joinedcontext platform base installation for local development. The script deploys the following:"
  stop_color
  echo "  - Minikube (optional, with -m option)"
  echo "  - Ingress-nginx"
  echo "  - MetalLB"
  echo "  - Cert-Manager"
  echo "  - Portainer (optional, with -p option)"
  echo
  echo_green "You may set the following environment variables:"
  echo_blue "  DOMAIN${GREEN}: The domain name for the cluster. Default: ${BLUE}$DOMAIN"
  echo_blue "  KUBECONFIG${GREEN}: The path to the kubeconfig file. Default: ${BLUE}$HOME/.kube/config"
  echo_blue "  KUBECONTEXT${GREEN}: The kubeconfig context to use, e.g. minukube or docker-desktop. Default: ${BLUE}minikube"
  echo
  echo_blue "  DRIVER${GREEN}: The minikube driver. Default: ${BLUE}docker"
  echo_blue "  CPUS${GREEN}: The number of CPUs to use for minikube. Default: ${BLUE}4"
  echo_blue "  MEMORY${GREEN}: The amount of memory to use for minikube in MB. Default: ${BLUE}7464"
  echo
  echo_green "Usage: ${BLUE}setup-script [OPTIONS]"
  echo
  echo "    Options:"
  echo "      cert, -c                   Create new root certificate"
  echo
  echo "      minikube, -m               Start the Minikube setup (skipped by default)"
  echo "      k3s, -k                    Create k3s cluster (skipped by default)"
  echo "      clear, -cl                 Clear all namespaces"
  echo "      uninstall, -u              Remove the cluster incl. the platform deployment"
  echo
  echo "      portainer, -p              Deploy the portainer service (skipped by default)"
  echo
  echo "      -f, -fmb, -fcm, -fi        Force installation/upgrade even if namespaces already exist"
  echo "                                 (fmb=metallb, fcm=cert-manager, fi=ingress-nginx)"
  echo
  echo "      --help, -h                 Show this help message and exit"
  echo
  echo "Happy coding!"
  echo
  stop_color
  exit 0
}

clear_namespaces() {
  start_color "red"
  echo
  echo "Deleting namespaces..."
  echo
  helm uninstall --namespace cert-manager cert-manager || true
  helm uninstall --namespace metallb-system metallb || true
  helm uninstall --namespace ingress-nginx ingress-nginx || true
  kubectl --kubeconfig ${KUBECONFIG} --context ${KUBECONTEXT} delete namespace cert-manager || true
  kubectl --kubeconfig ${KUBECONFIG} --context ${KUBECONTEXT} delete namespace metallb-system || true
  kubectl --kubeconfig ${KUBECONFIG} --context ${KUBECONTEXT} delete namespace ingress-nginx || true
  kubectl --kubeconfig ${KUBECONFIG} --context ${KUBECONTEXT} delete namespace portainer || true
  echo_blue_nl "Waiting for namespaces to be deleted..."
  kubectl --kubeconfig ${KUBECONFIG} --context ${KUBECONTEXT} wait --namespace cert-manager --for=delete pod --selector=app.kubernetes.io/name=cert-manager --timeout=600s || true
  kubectl --kubeconfig ${KUBECONFIG} --context ${KUBECONTEXT} wait --namespace metallb-system --for=delete pod --selector=app=metallb --timeout=600s || true
  kubectl --kubeconfig ${KUBECONFIG} --context ${KUBECONTEXT} wait --namespace ingress-nginx --for=delete pod --selector=app.kubernetes.io/name=ingress-nginx,app.kubernetes.io/component=controller --timeout=600s || true
  kubectl --kubeconfig ${KUBECONFIG} --context ${KUBECONTEXT} wait --namespace portainer --for=delete pod --selector=app=portainer --timeout=600s || true
  echo_green_nl "All namespaces deleted."
  echo
  stop_color
  exit 0
}

uninstall() {
  start_color "red"
  echo
  echo "Removing Minikube cluster..."
  echo
  minikube delete --all
  echo_green_nl "Minikube cluster removed."
  echo
  echo
  echo "Removing k3d cluster..."
  echo
  k3d cluster delete jc-local
  echo_green_nl "K3D cluster removed."
  echo
  stop_color
  exit 0
}

FORCE_UPGRADE=false
FORCE_INGRESS_UPGRADE=false
FORCE_MB_UPGRADE=false
FORCE_CERT_UPGRADE=false

# Check for help switch
for arg in "$@"; do
  if [[ "$arg" == "--help" || "$arg" == "-h" ]]; then
    show_help
  fi
  if [[ "$arg" == "clear" || "$arg" == "-cl" ]]; then
    clear_namespaces
  fi
  if [[ "$arg" == "uninstall" || "$arg" == "-u" ]]; then
    uninstall
  fi
  if [[ "$arg" == "-f" ]]; then
    FORCE_UPGRADE=true
  fi
  if [[ "$arg" == "-fmb" ]]; then
    FORCE_MB_UPGRADE=true
  fi
  if [[ "$arg" == "-fcm" ]]; then
    FORCE_CERT_UPGRADE=true
  fi
  if [[ "$arg" == "-fi" ]]; then
    FORCE_INGRESS_UPGRADE=true
  fi
done


# Check for cert switch
for arg in "$@"; do
  if [[ "$arg" == "cert" || "$arg" == "-c" ]]; then
    echo_green_nl "Creating new root certificate..."
    cd .ssl
    rm -rf jc.crt
    rm -rf jc.key
    ./create_ca.sh
    cd -
    # `tr`, not dos2unix: the only job here is stripping CRLFs from a certificate this
    # script just generated, and dos2unix is not installed on a CI runner — the whole
    # deployment-variants job died on `dos2unix: command not found` before k3d ever ran.
    for pem in .ssl/jc.crt .ssl/jc.key; do
      tr -d '\r' < "$pem" > "$pem.unix" && mv "$pem.unix" "$pem"
    done
  fi
done

# Set DRIVER if not already set
: ${CPUS:=4}
: ${MEMORY:=7464}
: ${DRIVER:=docker}
: ${KUBECONFIG:=$HOME/.kube/config}

DEFAULT_CONTEXT=$(kubectl config current-context 2>/dev/null || echo "")
: ${KUBECONTEXT:=${DEFAULT_CONTEXT:-minikube}}

: ${HTTP_PROXY:""}
: ${HTTPS_PROXY:""}
: ${NO_PROXY:""}

export CLUSTER_IP=${CLUSTER_IP:-127.0.0.1} # Default to 127.0.0.1

# Check for install minikube switch
for arg in "$@"; do
  if [[ "$arg" == "minikube" || "$arg" == "-m" ]]; then
    echo_green_nl "Setting up Minikube with driver $DRIVER..."
    select_docker_context
    minikube start --driver=$DRIVER --no-vtx-check --cpus $CPUS --memory $MEMORY

    # convert windows paths to linux pathes for use with WSL
    if [[ "$OSTYPE" == "msys" ]] || [[ "$OSTYPE" == "cygwin" ]] || [[ "$OSTYPE" == "wsl" ]] || [[ "$OSTYPE" == "win32" ]]; then
      sed -E "s|([A-Za-z]):\\\\|/mnt/\L\1/|g; s|\\\\|/|g" $HOME/.kube/config > $HOME/.kube/config.wsl
    fi

    echo_green_nl "Waiting for Minikube to start..."
    export KUBECONTEXT=minikube
    kubectl --kubeconfig ${KUBECONFIG} --context ${KUBECONTEXT} wait --for=condition=Ready nodes --all --timeout=600s
    # export CLUSTER_IP=$(minikube ip)
  fi
done

# Check for install k3s switch
for arg in "$@"; do
  if [[ "$arg" == "k3s" || "$arg" == "-k" ]]; then
    if k3d cluster list | grep -q '^jc-local\b'; then
        echo_green_nl "K3D cluster jc-local alredy exists."
        export KUBECONTEXT=k3d-jc-local
    else
      echo_green_nl "Setting up k3s cluster..."
      select_docker_context
      k3d cluster create --config k3d-jc-local.yaml
      k3d kubeconfig merge -a -d

      echo_green_nl "Waiting for k3s cluster to start..."
      export KUBECONTEXT=k3d-jc-local
      kubectl --kubeconfig ${KUBECONFIG} --context ${KUBECONTEXT} wait --for=condition=Ready nodes --all --timeout=600s
    fi
  fi
done

if [[ "$KUBECONTEXT" != *"minikube"* && "$KUBECONTEXT" != *"k3d-jc-local"* && "$KUBECONTEXT" != *"docker-desktop"* ]]; then
    echo_red "Warning: The current KUBECONTEXT does not contain 'minikube', 'k3d-jc-local', or 'docker-desktop'. Currently set to: ${KUBECONTEXT}"
    echo_red "Set KUBECONTEXT to 'minikube*', 'k3d-jc-local*' or 'docker-desktop*' to avoid this warning."
    read -p "Press Enter to continue..."
fi

echo_green_nl "Getting cluster info..."
kubectl --kubeconfig ${KUBECONFIG} --context ${KUBECONTEXT} cluster-info # --context docker-desktop

export IP_RANGE_STOP=$(increment_ip $CLUSTER_IP 20)
echo_green_nl "Cluster IP: ${YELLOW}$CLUSTER_IP"

echo_green_nl "Adding helm repositories..."
helm repo add ingress-nginx https://kubernetes.github.io/ingress-nginx --force-update
helm repo add metallb https://metallb.github.io/metallb --force-update
helm repo add jetstack https://charts.jetstack.io --force-update
helm repo update


INGRESS_RELEASE_EXISTS=$(helm list --namespace ingress-nginx -q | grep -w "ingress-nginx" || true)
if [ -z "$INGRESS_RELEASE_EXISTS" ] || [ "$FORCE_UPGRADE" = true ] || [ "$FORCE_INGRESS_UPGRADE" = true ]; then
  echo_green_nl "Enabling Nginx Ingress..."
  helm upgrade --namespace ingress-nginx --install --create-namespace --set controller.config.annotations-risk-level=Critical --set controller.config.enable-annotation-validation=false --set controller.publishService.enabled=true  --set controller.config.proxy-buffer-size="64k" --set controller.config.proxy-buffers="4 64k" --set controller.config.proxy-busy-buffers-size="128k" ingress-nginx ingress-nginx/ingress-nginx
  echo_green_nl "Waiting for Nginx Ingress to be ready..."
  wait_for_ready_pod ingress-nginx "app.kubernetes.io/name=ingress-nginx,app.kubernetes.io/component=controller" 600s
else
  echo_green_nl "Nginx Ingress already enabled. Skipping upgrade."
fi


METALLB_RELEASE_EXISTS=$(helm list --namespace metallb-system -q | grep -w "metallb" || true)
if [ -z "$METALLB_RELEASE_EXISTS" ] || [ "$FORCE_UPGRADE" = true ] || [ "$FORCE_MB_UPGRADE" = true ]; then
  echo_green_nl "Enabling MetalLB for IP range ${YELLOW}$CLUSTER_IP ${GREEN}- ${YELLOW}$IP_RANGE_STOP${GREEN}..."
  helm upgrade --namespace metallb-system --install --create-namespace metallb metallb/metallb
  echo_green_nl "Waiting for MetalLB to be ready..."
  wait_for_ready_pod metallb-system "app.kubernetes.io/component=controller" 600s
  echo_green_nl "Configuring MetalLB..."
  # MetalLB validates IPAddressPool and L2Advertisement through an admission webhook that its
  # own controller serves. The pod reports Ready a moment before that webhook answers, so a
  # straight apply loses a race with a 502 from the API server. With `set -e` that aborts the
  # whole bootstrap, and everything below here - cert-manager included - is silently skipped;
  # the run then fails minutes later on a missing CRD instead of here (T-0350).
  metallb_configured=false
  for attempt in $(seq 1 30); do
    if envsubst < metallb-template.yaml | kubectl --kubeconfig ${KUBECONFIG} --context ${KUBECONTEXT} apply -f -; then
      metallb_configured=true
      break
    fi
    echo "  MetalLB webhook not answering yet (attempt ${attempt}/30), retrying in 5s..."
    sleep 5
  done
  if [ "$metallb_configured" != true ]; then
    echo "MetalLB webhook never became ready; refusing to continue with a half-built cluster." >&2
    exit 1
  fi

  EXISTING=$(kubectl --kubeconfig ${KUBECONFIG} --context ${KUBECONTEXT} --namespace kube-system get configmap coredns-custom -o yaml 2>/dev/null | grep -q "joinedcontext.server" && echo "FOUND" || echo "NOT FOUND")
  if [ "$EXISTING" = "NOT FOUND" ]; then
    echo_green_nl "Configuring CoreDNS via coredns-custom..."
    cat <<EOF | kubectl --kubeconfig ${KUBECONFIG} --context ${KUBECONTEXT} --namespace kube-system apply -f -
apiVersion: v1
kind: ConfigMap
metadata:
  name: coredns-custom
  namespace: kube-system
data:
  joinedcontext.server: |
    joinedcontext.test {
      template IN ANY {
        match "^(.*\\.)?joinedcontext\\.test\\.$"
        answer "{{ .Name }} 60 IN CNAME ingress-nginx-controller.ingress-nginx.svc.cluster.local."
        fallthrough
      }
    }
EOF
    # CoreDNS neu laden
    kubectl --kubeconfig ${KUBECONFIG} --context ${KUBECONTEXT} -n kube-system rollout restart deploy/coredns
  else
    echo_green_nl "coredns-custom already configured. Skipping upgrade."
  fi

else
  echo_green_nl "MetalLB already enabled. Skipping upgrade."
fi


CERTMANAGER_RELEASE_EXISTS=$(helm list --namespace cert-manager -q | grep -w "cert-manager" || true)
if [ -z "$CERTMANAGER_RELEASE_EXISTS" ] || [ "$FORCE_UPGRADE" = true ] || [ "$FORCE_CERT_UPGRADE" = true ]; then
  echo_green_nl "Installing Cert-Manager..."
  helm upgrade cert-manager jetstack/cert-manager --install --namespace cert-manager --create-namespace --version v1.14.4 --set installCRDs=true
  echo_green_nl "Waiting for Cert-Manager to be ready..."
  kubectl --kubeconfig ${KUBECONFIG} --context ${KUBECONTEXT} wait --namespace cert-manager --for=condition=Available deployment --selector=app.kubernetes.io/name=cert-manager --timeout=600s
  kubectl --kubeconfig ${KUBECONFIG} --context ${KUBECONTEXT} wait --namespace cert-manager --for=condition=ready pod --selector=app=cert-manager,app.kubernetes.io/component=controller --timeout=600s


  echo_green_nl "Applying CA configuration..."
  kubectl --kubeconfig ${KUBECONFIG} --context ${KUBECONTEXT} create configmap --namespace cert-manager jc.crl.pem --from-file=.ssl/jc.crl.pem -o yaml --dry-run=client | kubectl --kubeconfig ${KUBECONFIG} --context ${KUBECONTEXT} apply -f -
  kubectl --kubeconfig ${KUBECONFIG} --context ${KUBECONTEXT} create configmap --namespace cert-manager jc.crl --from-file=.ssl/jc.crl -o yaml --dry-run=client | kubectl --kubeconfig ${KUBECONFIG} --context ${KUBECONTEXT} apply -f -
  kubectl --kubeconfig ${KUBECONFIG} --context ${KUBECONTEXT} create configmap --namespace cert-manager jc.crt --from-file=.ssl/jc.crt -o yaml --dry-run=client | kubectl --kubeconfig ${KUBECONFIG} --context ${KUBECONTEXT} apply -f -
  export CA_CERT=$(cat .ssl/jc.crt | base64 | tr -d '\n')
  export CA_KEY=$(cat .ssl/jc.key | base64 | tr -d '\n')
  envsubst < ca-template.yaml | kubectl --kubeconfig ${KUBECONFIG} --context ${KUBECONTEXT} apply -f -
else
  echo_green_nl "Cert-Manager already enabled. Skipping upgrade."
fi

# Check for portainer argument
for arg in "$@"; do
  if [[ "$arg" == "portainer" || "$arg" == "-p" ]]; then
    echo_green_nl "Deploying portainer service..."
    envsubst < portainer-template.yaml | kubectl --kubeconfig ${KUBECONFIG} --context ${KUBECONTEXT} apply -f -

    echo_green_nl "Waiting for portainer service to be ready..."
    wait_for_ready_pod portainer "app.kubernetes.io/name=portainer" 600s
  fi
done

echo
echo_yellow "Setup complete! Setup executed in $(format_time $SECONDS)."
echo
echo_green "Cluster IP: ${BLUE}$CLUSTER_IP${NC}"
echo

# if system is windows
echo_green "If you are using a local DNS server:"
if [[ "$OSTYPE" == "msys" ]] || [[ "$OSTYPE" == "cygwin" ]] || [[ "$OSTYPE" == "wsl" ]] || [[ "$OSTYPE" == "win32" ]]; then
  echo_green "  Add the IP Address of your kubernetes cluster to your DNS Server's hosts file, e.g. Acrylic, with the following line:"
  echo_blue "     $CLUSTER_IP *.$DOMAIN $DOMAIN"
  echo_green "  Please remember to flush the dns cache with the command '${BLUE}ipconfig /flushdns${NC}'"
else
  echo_green "  Add the IP Address of your kubernetes cluster to your DNS Server's hosts file, e.g. from dnsmasq, with the following line:"
  echo_blue "     address=/$DOMAIN/127.0.0.1"
  echo_green "  Please remember to flush the dns cache with the command 'sudo systemd-resolve --flush-caches'"
fi

echo
echo_green "Alternatively, you may add the DNS names to your ${BLUE}hosts${GREEN} file (easy method). Please read the documentation for more details."
# Check for portainer argument
for arg in "$@"; do
  if [[ "$arg" == "portainer" || "$arg" == "-p" ]]; then
    echo_green "If you installed portainer and use a ${BLUE}hosts${GREEN} file, make sure that you add the following line to your ${BLUE}hosts${GREEN} file:"
    echo_blue "     $CLUSTER_IP portainer.$DOMAIN"
  fi
done

if [[ "$OSTYPE" == "msys" ]] || [[ "$OSTYPE" == "cygwin" ]] || [[ "$OSTYPE" == "wsl" ]] || [[ "$OSTYPE" == "win32" ]]; then
  if [[ "$KUBECONTEXT" == "minikube" ]]; then
    echo
    echo_red "WSL users${GREEN}: (e.g. for running ansible locally)"
    echo_green "  If you want to access the services from within WSL2, make sure you set ${RED}networkingMode=mirrored${GREEN} in your  ~/.wslconfig (restart Windows)."
  elif [[ "$KUBECONTEXT" == "docker-desktop" ]]; then
    echo
    echo_green "Docker Desktop users:"
    echo_green "  The cluster runs best with standard network settings (e.g. NAT)."
  fi
fi

if [[ "$KUBECONTEXT" == "minikube" ]]; then
  echo
  echo_green "Next, you have to start ${RED}minikube tunnel${GREEN} to access the cluster from your local machine."
fi

echo
echo_green "Next step is to deploy the joinedcontext platform as described in ${YELLOW}../docs/Deployment/02-installation.md"
echo
echo_green "Happy coding!"
echo
