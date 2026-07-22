# reactant-analyzer

Analyseur statique React (niveau 3, interprétation abstraite) en Rust.

## Principes de conception — NON NÉGOCIABLES

1. **Pas de workarounds.** Corriger la cause racine, jamais le symptôme. Si un fix
   contourne un problème au lieu de le résoudre (patch local, cas spécial, hack
   rule-par-rule pour un défaut du moteur), c'est la mauvaise solution : corriger
   le moteur / l'IR / la couche centrale à la place.

2. **Règle du paragraphe unique.** Si une décision demande plus d'un paragraphe
   pour être justifiée, c'est une mauvaise idée. Chercher la solution plus simple
   qui se justifie en une ou deux phrases.

3. **Modulaire et général d'abord.** Toujours préférer une solution générale et
   réutilisable (mécanisme partagé, abstraction dans la couche commune) à une
   solution ad hoc dupliquée dans chaque règle. Un problème qui touche plusieurs
   règles se corrige une fois, au niveau central.

## Invariants du projet

- **Soundness** : l'interprétation abstraite calcule un sur-ensemble des
  comportements. Faux positifs tolérés, faux négatifs INTERDITS.
- **Niveaux de diagnostic** : Error (certain), Warning (incertain),
  Info (limitations, derrière `--info`).

## Références

- Décisions d'architecture : `docs/adr/`
- TODO courant : `docs/TODO.md`
- Dette technique (workarounds + architecture, chiffrés et séquencés) : `docs/tech-debt.md`
