# TODO — déplacé

Ce fichier ne tient plus le backlog. Son contenu a été éclaté en issues, une par
limite, le 2026-08-27.

- **Résumé des limites connues, pour les utilisateurs** :
  [limitations.md](limitations.md)
- **Travail ouvert, une entrée par limite** :
  [le tracker](https://github.com/rboudrouss/reactant-analyzer/issues)

Labels : `soundness-bug` (l'analyse est fausse, pas seulement imprécise),
`precision-fn` / `precision-fp` (compromis assumés), `infra`, `rule-proposal`,
`ux` ; taille `size/S|M|L` ; `blocked` quand une issue en attend une autre.

Les limites tranchées « on ne corrige pas » sont des issues **fermées**
`wontfix`, pour que le raisonnement reste citable et que personne ne repropose
le fix :
[`--state closed --label wontfix`](https://github.com/rboudrouss/reactant-analyzer/issues?q=is%3Aissue+is%3Aclosed+label%3Awontfix).

> Ce fichier reste comme redirection : une dizaine d'ADR le référencent par ce
> chemin, et un ADR est un enregistrement historique qu'on ne réécrit pas.
