#!/usr/bin/env bash
# Compile le manuscrit complet : pdflatex ×2, bibtex, makeindex, pdflatex ×2.
set -u
cd "$(dirname "$0")"
mkdir -p build
run() { pdflatex -interaction=nonstopmode -file-line-error -output-directory=build main.tex > build/pass-$1.out 2>&1; }
run 1
if [ -s refs.bib ]; then (cd build && BIBINPUTS=..: bibtex main > bibtex.out 2>&1); fi
if [ -f build/main.idx ]; then (cd build && makeindex -q main.idx > makeindex.out 2>&1); fi
run 2
run 3
status=$?
echo "exit=$status"
grep -nE '^! |\.tex:[0-9]+: ' build/main.log | head -40
echo "-- Références non résolues :"
grep -oE "Reference \`[^']*' on page [0-9]+ undefined" build/main.log | sed -E 's/ on page [0-9]+//' | sort -u | head -60
echo "-- Citations non résolues :"
grep -oE "Citation \`[^']*' on page [0-9]+ undefined" build/main.log | sed -E 's/ on page [0-9]+//' | sort -u
grep -c 'Overfull' build/main.log | sed 's/^/-- Overfull boxes : /'
command -v pdfinfo >/dev/null && pdfinfo build/main.pdf | grep -E '^Pages'
cp -f build/main.pdf reactant-manuscrit.pdf 2>/dev/null && echo "-> reactant-manuscrit.pdf"
exit $status
