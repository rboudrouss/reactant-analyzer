#!/usr/bin/env bash
# Génère le dossier test-repo/ avec du code React réel pour le corpus de test.
# Usage:
#   ./scripts/setup-test-repo.sh          # clone les repos manquants
#   ./scripts/setup-test-repo.sh --force  # supprime et re-clone tout
#   ./scripts/setup-test-repo.sh --verify # vérifie les SHA sans rien cloner
#
# Chaque entrée est ÉPINGLÉE à un commit. Sans cela le corpus suit la branche
# par défaut de quatorze dépôts tiers, et un chiffre corpus change parce que
# quelqu'un d'autre a poussé — la mesure ne veut plus rien dire, et une CI qui
# la surveille échoue pour des raisons qui ne sont pas des régressions (#15).
# `degit` était plus rapide mais jetait `.git`, donc le corpus n'était même pas
# vérifiable après coup ; un clone superficiel sur un SHA garde l'identité.
#
# Pour bouger le corpus : changer un SHA ici, re-cloner avec --force, puis
# régénérer la ligne de base avec scripts/corpus-baseline.py. Les trois vont
# ensemble, dans un commit qui dit pourquoi.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEST="$ROOT/test-repo"

# Format : "source-github  nom-local  sha"
REPOS=(
  "alan2207/bulletproof-react   bulletproof-react  9506629ed003a561c6627735480cce4994244bb4"
  "chakra-ui/chakra-ui          chakra-ui          495e06934bf574bcbbeb009fbaefb414cb414d1e"
  "excalidraw/excalidraw        excalidraw         214cd6e6e8ac3ad6b68486aa7aa7241abdf9445f"
  "usememos/memos               memos              dfa0fda76602d49dfbb68a6683ef20b068c8d45b"
  "steven-tey/novel             novel              fa95098e66476c466faebb8211baa5869c101a9c"
  "satnaing/shadcn-admin        shadcn-admin       e16c87f213a5ba5e45964e9b67c792105ec74d26"
  "pmndrs/zustand               zustand            b57db4f86ef179285da216eeb291266da82c361c"

  # Monorepos. Big, multi-package, and the ones that actually exercise the
  # scaling paths — they are where the utility-inlining budget is exhausted and
  # where the `infinite-loop` O(C²) hang (#86) shows up.
  "mantinedev/mantine           mantine            3862b09668c4523161a5b635666e0d68a3fe347b"
  "dubinc/dub                   dub                73415cf5e6be13ce9adb7ba5e97474307db34a17"
  "twentyhq/twenty              twenty             bad3fff518311065022d5fb46a6f890fe718fe8d"

  # Next.js (ADR-026). Chosen to cover the four layouts that change how the
  # analyzer resolves a project, not just to add volume :
  #   commerce                       app/ à la racine, tsconfig `baseUrl` SANS `paths`
  #                                  (specifiers nus : `import "lib/shopify"`)
  #   ai-chatbot                     app/ à la racine, `@/*` -> `./*`
  #   next-shadcn-dashboard-starter  src/app/, `@/*` -> `./src/*` (le plus gros)
  #   precedent                      alias multiples (`@/components/*`, `@/lib/*`…)
  "vercel/commerce                        commerce                       3761e52e60df9c6a316e067dbfd7032e494d3634"
  "vercel/ai-chatbot                      ai-chatbot                     c2f8235e1f3ea903ad8b7f61447c4f74164b5c58"
  "Kiranism/next-shadcn-dashboard-starter next-shadcn-dashboard-starter  7705dfc0d13889e45c26a55ad5908da6a7a9a605"
  "steven-tey/precedent                   precedent                      3be40205d7cdf56082cd284f07f12251b9208f79"
)

MODE=clone
case "${1:-}" in
  --force)  MODE=force ;;
  --verify) MODE=verify ;;
  "")       ;;
  *) echo "usage: $0 [--force|--verify]" >&2; exit 2 ;;
esac

# `--verify` ne clone rien : il dit si test-repo/ est bien le corpus épinglé.
# Un clone `degit` d'avant l'épinglage n'a pas de `.git` et sort « unpinned » —
# ce n'est pas une erreur, c'est le seul aveu honnête possible : cet arbre-là
# n'est identifiable par rien.
if [[ $MODE == verify ]]; then
  status=0
  for entry in "${REPOS[@]}"; do
    read -r src name sha <<<"$entry"
    target="$DEST/$name"
    if [[ ! -d "$target" ]]; then
      printf '%-32s missing\n' "$name"; status=1; continue
    fi
    if [[ ! -d "$target/.git" ]]; then
      printf '%-32s unpinned (no .git — cloned before pinning)\n' "$name"; status=1; continue
    fi
    have="$(git -C "$target" rev-parse HEAD)"
    if [[ "$have" == "$sha" ]]; then
      printf '%-32s ok\n' "$name"
    else
      printf '%-32s DRIFT have=%s want=%s\n' "$name" "${have:0:12}" "${sha:0:12}"; status=1
    fi
  done
  exit $status
fi

mkdir -p "$DEST"

for entry in "${REPOS[@]}"; do
  read -r src name sha <<<"$entry"
  target="$DEST/$name"

  if [[ -d "$target" ]]; then
    if [[ $MODE == force ]]; then
      echo ">> suppression de $name"
      rm -rf "$target"
    else
      echo ">> $name déjà présent, skip (--force pour re-cloner)"
      continue
    fi
  fi

  echo ">> $src@${sha:0:12} -> test-repo/$name"
  mkdir -p "$target"
  git -C "$target" init -q
  git -C "$target" remote add origin "https://github.com/$src.git"
  git -C "$target" fetch -q --depth 1 origin "$sha"
  git -C "$target" checkout -q FETCH_HEAD
done

echo
echo "Terminé. Contenu de test-repo/ :"
ls -1 "$DEST"
