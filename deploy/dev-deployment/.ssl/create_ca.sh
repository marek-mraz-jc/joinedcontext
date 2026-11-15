#!/bin/bash

# Set DOMAIN if not already set
: ${DOMAIN:=jc.test}
export DOMAIN=${DOMAIN}

cd "$(dirname "$0")"

touch index.txt
echo 01 > crlnumber

rm -rf jc.*

# Generate private key
openssl genrsa -out jc.key 2048

# Generate self-signed certificate
openssl req -x509 -new -nodes \
  -key jc.key \
  -days 3650 \
  -sha256 \
  -out jc.crt \
  -subj "/C=DE/ST=State/L=SmartCity/O=joinedcontext/OU=platform/CN=$DOMAIN" \
  -addext "keyUsage=critical, cRLSign, digitalSignature, keyCertSign" \
  -addext "extendedKeyUsage=serverAuth" \
  -addext "subjectAltName=DNS:$DOMAIN,DNS:*.$DOMAIN,IP:127.0.0.1" \
  -addext "crlDistributionPoints=URI:http://$DOMAIN/jc.crl"

# Generate CRL if needed
# Ensure ca.conf is correctly configured for OpenSSL CA usage
if [ -f ca.conf ]; then
  cat ca.conf | envsubst | openssl ca \
    -gencrl \
    -keyfile jc.key \
    -cert jc.crt \
    -out jc.crl.pem \
    -config /dev/stdin
  openssl crl -inform PEM -in jc.crl.pem -outform DER -out jc.crl
else
  echo "Warning: ca.conf not found. Skipping CRL generation."
fi
