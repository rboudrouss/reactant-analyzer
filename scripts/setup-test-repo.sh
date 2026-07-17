#!/usr/bin/env bash
# Génère le dossier test-repo/ avec du code React réel pour le corpus de test.
# Usage:
#   ./scripts/setup-test-repo.sh          # clone les repos manquants
#   ./scripts/setup-test-repo.sh --force  # supprime et re-clone tout
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEST="$ROOT/test-repo"

# Format : "source-github  nom-local"
REPOS=(
  "alan2207/bulletproof-react   bulletproof-react"
  "chakra-ui/chakra-ui          chakra-ui"
  "vercel/commerce              commerce"
  "excalidraw/excalidraw        excalidraw"
  "usememos/memos               memos"
  "steven-tey/novel             novel"
  "satnaing/shadcn-admin        shadcn-admin"
  "pmndrs/zustand               zustand"
)

FORCE=0
[[ "${1:-}" == "--force" ]] && FORCE=1

mkdir -p "$DEST"

for entry in "${REPOS[@]}"; do
  read -r src name <<<"$entry"
  target="$DEST/$name"

  if [[ -d "$target" ]]; then
    if [[ $FORCE -eq 1 ]]; then
      echo ">> suppression de $name"
      rm -rf "$target"
    else
      echo ">> $name déjà présent, skip (--force pour re-cloner)"
      continue
    fi
  fi

  echo ">> degit $src -> test-repo/$name"
  npx --yes degit "$src" "$target"
done

echo
echo "Terminé. Contenu de test-repo/ :"
ls -1 "$DEST"
