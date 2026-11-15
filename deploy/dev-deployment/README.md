# Local cluster for development

`startup.sh` creates a k3d cluster (`k3d-jc-local`) with ingress-nginx, MetalLB and cert-manager (self-signed CA distributed by the `prepare` component). Prerequisites and the full flow: `../../docs/Deployment/01-prerequisites.md` and `02-installation.md`.

## Dev CA

The dev CA private key (`.ssl/jc.key`) is not committed. Run `cd dev-deployment/.ssl && ./create_ca.sh` once to regenerate the CA (`jc.crt` + `jc.key`) before the first local startup.
