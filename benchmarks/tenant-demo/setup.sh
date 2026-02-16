#!/bin/bash
#
# Populate the tenant_docs table for the demo app.
# Each tenant gets content from two random vocabulary domains.
#
# Usage:
#   bash setup.sh                     # uses defaults
#   NUM_TENANTS=20 bash setup.sh      # override tenant count
#
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

NUM_TENANTS="${NUM_TENANTS:-10}"
DOCS_MULTIPLIER="${DOCS_MULTIPLIER:-1}"
DATABASE_URL="${DATABASE_URL:-postgres:///postgres}"

echo "=== Tenant Demo: Setup ==="
echo "  tenants:         ${NUM_TENANTS}"
echo "  docs_multiplier: ${DOCS_MULTIPLIER}"
echo "  database:        ${DATABASE_URL}"
echo ""

psql "${DATABASE_URL}" \
    -v num_tenants="${NUM_TENANTS}" \
    -v docs_multiplier="${DOCS_MULTIPLIER}" \
    -f "${SCRIPT_DIR}/setup.sql"

echo ""
echo "=== Setup complete ==="
echo ""
echo "Start the demo app:"
echo "  DATABASE_URL=\"${DATABASE_URL}\" go run main.go"
echo ""
echo "Then open http://localhost:8080"
