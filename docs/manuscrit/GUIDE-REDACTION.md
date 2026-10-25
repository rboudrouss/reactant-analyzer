# Guide de rédaction du manuscrit

Ce document s'adresse à qui écrit ou relit un chapitre de `docs/manuscrit/`.
Le manuscrit est un **mémoire pédagogique** (type thèse / polycopié) qui explique
exhaustivement `reactant-analyzer` : un lecteur qui l'a lu doit très bien
connaître la codebase. Il photographie le dépôt au commit `e67b10a`
(27 septembre 2026).

## 1. Ce qu'est un chapitre

- Fichier `chapters/NN-slug.tex`, commençant par `\chapter{Titre}` et
  `\label{chap:slug}`. **Aucun** `\documentclass`, `\usepackage`,
  `\begin{document}`, `\newcommand`, `\definecolor`, `\lstdefinelanguage` :
  tout vient de `preamble.tex`, `macros.tex`, `listings-setup.tex`.
- Le chapitre s'ouvre par un bloc `\begin{objectifs} \item … \end{objectifs}`
  puis `\prerequis{…}` (chapitres à lire avant, avec `\cref{chap:…}`).
- Il se ferme par `\begin{resume} … \end{resume}` (une page au plus : les
  idées à retenir, les types et fichiers rencontrés, ce que le chapitre
  suivant ajoutera), puis facultativement une section `Exercices`.
- Progression : du plus simple au plus difficile, **dans** le chapitre comme
  entre chapitres. On part d'un problème concret (un programme React, une
  question que l'analyseur doit trancher), on montre la difficulté, puis la
  solution du code, puis les cas limites.
- Longueur visée : indiquée dans le plan (`PLAN.md`). Un chapitre central fait
  typiquement 20 à 40 pages PDF.

## 2. Langue et style

- Français, ton d'un bon cours : phrases courtes, un sujet par paragraphe,
  définitions avant usage. Les identifiants, noms de fichiers, de types et de
  règles restent en anglais tels qu'ils sont dans le code.
- Babel français est chargé : les espaces avant `:` `;` `?` `!` sont gérées
  automatiquement ; guillemets avec `\og texte\fg{}` ; pas de `~` avant `:`.
- Pas de `_` ni `#` nus dans le texte : utiliser `\code{snake_case}` (qui
  accepte `_` sans échappement) ; en mode mathématique utiliser `\_`.
- `%` s'écrit `\%`, `&` s'écrit `\&`, `~` s'écrit `\textasciitilde`.
- Les caractères Unicode courants (`⊤ ⊥ ⊓ ⊔ ⊑ → ⇒ ≤ ∀ ∞ é …`) sont acceptés
  dans la prose ET dans les listings (table `literate`). En cas d'erreur
  `Unicode character … not set up`, remplacer par la macro TeX équivalente ou
  ajouter le caractère dans `listings-setup.tex` (signaler-le).

## 3. Macros disponibles (`macros.tex`)

| Macro | Usage |
|---|---|
| `\code{ident}` | identifiant, expression courte, chemin court (accepte `_`) |
| `\file{src/ir/expr.rs}` | chemin de fichier |
| `\src{src/ir/expr.rs}{12}{40}` / `\srcline{…}{12}` | renvoi précis dans le code |
| `\regle{infinite-loop}` | nom d'une règle du catalogue |
| `\relation{writers}` | nom d'une relation du moteur |
| `\adr{27}` | renvoi vers la fiche ADR-27 (annexe, label `adr:27`) |
| `\issue{63}` | lien vers l'issue GitHub #63 |
| `\terme{point fixe}` | terme défini ici (italique + entrée d'index) |
| `\idx{mot}` | entrée d'index seule |
| `\Error`, `\Warning`, `\Info` | niveaux de diagnostic |
| `\reactant` | le nom du projet |
| Math : `\must \may \Top \Bot \join \meet \widen \lleq \lfp \gam \abs \sem{e}` | notation unifiée des treillis |
| `\Render \Commit \Effect \Tick` | phases de React en math |

Notation mathématique **unifiée** dans tout le manuscrit : ordre `\lleq`,
join `\join`, meet `\meet`, widening `\widen`, concrétisation `\gam`,
abstraction `\abs`, plus petit point fixe `\lfp`. Ne pas en inventer d'autre.

## 4. Citer le code — la règle d'or

Tout extrait est **verbatim** et **localisé**. Deux moyens :

1. **Extrait pris directement dans le dépôt** (préféré pour les définitions de
   types et les passages décisifs, car impossible à falsifier) :
   ```latex
   \rustsnippet{src/ir/expr.rs}{10}{25}{les primitives du domaine des valeurs}
   \rustsnippet[label={lst:expr-prim}]{src/ir/expr.rs}{10}{25}{…}   % avec label
   \tsxsnippet{tests/fixtures/counter.tsx}{1}{12}{un compteur}
   \rustfile{src/engine/program_relations.rs}{le point d'entrée des relations}  % fichier entier (court)
   ```
   Les numéros de ligne affichés sont les vrais. Vérifier la plage avec
   `sed -n '10,25p' src/ir/expr.rs` avant de l'écrire. Plage raisonnable :
   5 à 60 lignes. Au-delà, découper et commenter entre les morceaux.
2. **Extrait inline** (programmes React d'exemple, code élagué, pseudo-code) :
   ```latex
   \begin{lstlisting}[language=TypeScript,caption={Compteur qui boucle},label={lst:boucle}]
   …
   \end{lstlisting}
   \begin{lstlisting}[language=Rust,caption={…}]  … \end{lstlisting}
   \begin{lstlisting}[language=JSON] … \end{lstlisting}
   ```
   Si un extrait inline vient du dépôt mais est élagué, le dire dans la
   légende (« élagué ») et donner `\src{…}{a}{b}`.
- Sortie de l'analyseur : `\begin{sortie} … \end{sortie}` ; commande shell :
  `\begin{shell} … \end{shell}`. Ces sorties doivent avoir été réellement
  observées (`cargo run -q -- check /tmp/ex.tsx`), pas imaginées.
- Dans la prose, un renvoi au code se fait par `\src{}` ou `\srcline{}`, jamais
  par un numéro de ligne nu. Au plus un ou deux renvois par phrase.
- **Rien d'inventé.** Un comportement non vérifié se signale par une note
  « à vérifier » dans un commentaire LaTeX `% TODO-VERIF: …` (le relecteur les
  traitera). Un manuscrit exact mais incomplet vaut mieux qu'un manuscrit
  complet et faux.

## 5. Environnements pédagogiques

```latex
\begin{definition}{Titre}{label}   … \end{definition}   % -> \cref{def:label}
\begin{theoreme}{Titre}{label}     … \end{theoreme}     % thm:label
\begin{proposition}{Titre}{label}  … \end{proposition}  % prop:label
\begin{lemme}{Titre}{label}        … \end{lemme}        % lem:label
\begin{invariant}{Titre}{label}    … \end{invariant}    % inv:label — invariant du code
\begin{exemple}{Titre}{label}      … \end{exemple}      % ex:label
\begin{exercice}{Titre}{label}     … \end{exercice}     % exo:label
\begin{solution} … \end{solution}
\begin{remarque} … \end{remarque}
\begin{piege} … \end{piege}                              % erreur classique, subtilité
\begin{react}[sujet] … \end{react}                       % rappel des internals React
\begin{decision}[ADR-27] … \end{decision}                % décision de conception et alternatives
```
Les deux arguments obligatoires (titre, label) sont **requis** même vides :
`\begin{exemple}{}{}` est accepté par tcolorbox.

Usage recommandé : un `react` chaque fois qu'un comportement de React est
nécessaire à la compréhension (render/commit, batching, `Object.is`, ordre des
hooks, Strict Mode, Server Components…) ; une `decision` pour chaque ADR
mobilisée, avec les alternatives refusées ; un `piege` pour chaque cas où le
lecteur (ou un contributeur) se tromperait naturellement ; des `exemple` gradués.

## 6. Schémas (TikZ)

TikZ est chargé avec les bibliothèques `arrows.meta, positioning,
shapes.geometric, shapes.misc, calc, fit, matrix, chains,
decorations.pathreplacing, backgrounds, automata`. Styles prêts :
`bloc`, `noeud`, `cfgbloc`, `fleche`, `flechemust`, `flechemay`, `treillis`,
`etiquette`.

Un schéma quand il **remplace** une page d'explication : pipeline d'analyse,
diagramme de Hasse d'un treillis, CFG d'un programme d'exemple, graphe de
churn, arbre de rendu, chronologie render → commit → effets → setState,
chaîne de témoins. Toujours dans un `figure` avec `\caption` et
`\label{fig:…}`, et toujours référencé dans le texte (`\cref{fig:…}`).
Pas de schéma décoratif.

```latex
\begin{figure}[htbp]\centering
\begin{tikzpicture}[node distance=10mm]
  \node[bloc] (a) {parse (oxc)};
  \node[bloc,right=of a] (b) {lowering};
  \draw[fleche] (a) -- (b);
\end{tikzpicture}
\caption{…}\label{fig:pipeline}
\end{figure}
```
Pour les algorithmes : `algorithm2e` est chargé
(`\begin{algorithm} \KwIn{…} \While{…}{…} \caption{…} \end{algorithm}`).

## 7. Références croisées

- Labels : `chap:slug`, `sec:slug-sujet`, `fig:…`, `lst:…`, `tab:…`,
  `def:…`, `thm:…`, `ex:…`, `exo:…`, `inv:…`, `adr:NN` (fiches ADR de
  l'annexe, déjà prévues : ne pas les définir dans un chapitre).
- Renvois avec `\cref{…}` (cleveref, en français). Vers un autre chapitre,
  utiliser le label indiqué dans `PLAN.md` ; il sera résolu à l'assemblage
  (le compilateur isolé signale ces renvois comme non résolus : c'est normal).
- Bibliographie : `\cite{CousotCousot77}`, `\cite{RivalYi2020}`,
  `\cite{ReactDocs}`, `\cite{ReactRulesOfHooks}`, `\cite{ReactRenderCommit}`,
  `\cite{oxc}` existent dans `refs.bib`. Pour ajouter une référence, écrire la
  clé à ajouter dans un commentaire `% BIB: @misc{…}` en tête de chapitre (le
  relecteur l'intégrera) — ne pas modifier `refs.bib` soi-même.

## 8. Compiler et vérifier

```bash
cd docs/manuscrit
./check-chapter.sh chapters/NN-slug.tex     # compile ce chapitre seul
```
Le script affiche `exit=0` et le nombre de pages, ou les erreurs avec
`fichier:ligne`. Un chapitre livré compile **sans erreur** ; les seuls
avertissements tolérés sont les références vers d'autres chapitres et les
`Overfull` de moins de 20 pt. Pour voir le résultat :
`pdftoppm -r 60 -png build/standalone-NN-slug.pdf build/NN` puis ouvrir les PNG.

## 8 bis. Points réglés dans le préambule (ne pas contourner)

- Le raccourci `:` de babel-french est désactivé pour tout le document
  (`\shorthandoff{:}` global) : `\cref{sec:...}` fonctionne, et il ne faut
  **pas** mettre `\shorthandoff{:}` / `\shorthandon{:}` dans un chapitre.
  Écrire « mot : suite » avec une espace ordinaire, ou `mot~: suite` si l'on
  veut interdire la coupure.
- Le caractère `…` est accepté dans les listings (rendu `...`).
- `\code{}` et `\file{}` acceptent `_ # & $ ^ ~` nus **et** les caractères
  UTF-8 (`⊤`, `✓`, `é`…) ; `\{ \} \# \% \ldots \textbackslash` y restent des
  commandes et s'impriment comme prévu. Seul `%` doit être écrit `\%` (sinon
  il commente la fin de ligne). `\code` fonctionne aussi en mode mathématique.
- `\cref` vers `definition`, `theoreme`, `proposition`, `lemme`, `invariant`,
  `exemple`, `exercice`, extraits et algorithmes donne le bon nom en français.
- Références bibliographiques disponibles en plus : `LeeAhnYi2025`
  (React-tRace), `EslintReactHooks`.

## 9. Ce qu'on ne fait pas

- Modifier quoi que ce soit hors de son propre fichier de chapitre (et des
  fichiers d'exemple temporaires sous `/tmp`). Jamais `git stash`, `checkout`,
  `commit`, `reset`, `clean`.
- Paraphraser un ADR sans le citer : les ADR sont des sources, on renvoie avec
  `\adr{NN}` et on résume la décision et les alternatives refusées.
- Écrire « le code fait X » sans l'avoir lu : chaque affirmation sur le
  comportement est adossée à un `\src{}` ou à une sortie observée.
- Laisser un lecteur sans exemple : chaque notion nouvelle est illustrée par un
  programme React court avant d'être formalisée.
