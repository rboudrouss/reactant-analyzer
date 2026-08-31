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
  "excalidraw/excalidraw        excalidraw"
  "usememos/memos               memos"
  "steven-tey/novel             novel"
  "satnaing/shadcn-admin        shadcn-admin"
  "pmndrs/zustand               zustand"

  # Monorepos. Big, multi-package, and the ones that actually exercise the
  # scaling paths — they are where the utility-inlining budget is exhausted and
  # where the `infinite-loop` O(C²) hang (#86) shows up. `dub` and `twenty` do
  # not currently finish a `--all-roots` run; keep them out of any timed sweep
  # until #86 is fixed.
  "mantinedev/mantine           mantine"
  "dubinc/dub                   dub"
  "twentyhq/twenty              twenty"

  # Next.js (ADR-026). Chosen to cover the four layouts that change how the
  # analyzer resolves a project, not just to add volume :
  #   commerce                       app/ à la racine, tsconfig `baseUrl` SANS `paths`
  #                                  (specifiers nus : `import "lib/shopify"`)
  #   ai-chatbot                     app/ à la racine, `@/*` -> `./*`
  #   next-shadcn-dashboard-starter  src/app/, `@/*` -> `./src/*` (le plus gros)
  #   precedent                      alias multiples (`@/components/*`, `@/lib/*`…)
  "vercel/commerce                       commerce"
  "vercel/ai-chatbot                     ai-chatbot"
  "Kiranism/next-shadcn-dashboard-starter next-shadcn-dashboard-starter"
  "steven-tey/precedent                  precedent"
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
