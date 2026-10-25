#!/usr/bin/env bash
# Compile UN chapitre isolément et rapporte les erreurs.
#   ./check-chapter.sh chapters/03-ir.tex
# Les références croisées vers d'autres chapitres sont listées mais non fatales.
set -u
cd "$(dirname "$0")"
ch="${1:?usage: check-chapter.sh chapters/NN-xxx.tex}"
name="$(basename "$ch" .tex)"
mkdir -p build
std="build/standalone-$name.tex"
{
  echo '\documentclass[11pt,a4paper,openany]{book}'
  echo '\input{preamble}'
  echo '\begin{document}'
  echo '\setcounter{chapter}{0}'
  echo "\\input{$ch}"
  echo '\end{document}'
} > "$std"
log="build/standalone-$name.log"
pdflatex -interaction=nonstopmode -halt-on-error -file-line-error -output-directory=build "$std" > /dev/null 2>&1
status=$?
# deuxième passe pour les références internes au chapitre
if [ $status -eq 0 ]; then
  pdflatex -interaction=nonstopmode -halt-on-error -file-line-error -output-directory=build "$std" > /dev/null 2>&1
  status=$?
fi
echo "== $ch : exit=$status"
if [ $status -ne 0 ]; then
  echo "-- ERREURS (log : $log)"
  grep -nE '^(\./|build/|\.\./)?[^ ]*\.tex:[0-9]+: |^! ' "$log" | head -30
  grep -nA3 '^! ' "$log" | head -40
fi
echo "-- Références non résolues (normales si elles visent un autre chapitre) :"
grep -oE "Reference \`[^']*' on page [0-9]+ undefined" "$log" | sort -u | head -30
grep -oE "Citation \`[^']*' on page [0-9]+ undefined" "$log" | sort -u | head -10
echo "-- Overfull > 20pt :"
grep -cE 'Overfull \\hbox \([2-9][0-9]\.|Overfull \\hbox \([0-9]{3,}' "$log"
if [ $status -eq 0 ] && command -v pdfinfo >/dev/null; then
  pdfinfo "build/standalone-$name.pdf" | grep -E '^Pages'
fi
exit $status
