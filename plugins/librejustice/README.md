# Plugin LibreJustice

Connecte votre agent (Claude Code ou Codex) au moteur de recherche de
jurisprudence et de législation françaises
[librejustice.fr](https://librejustice.fr) : cinq outils MCP et deux skills
qui encadrent l'usage — de la recherche du texte applicable à la citation
vérifiée.

## Installation

Claude Code :

```
/plugin marketplace add librejustice/librejustice
/plugin install librejustice@librejustice
```

Codex :

```
codex plugin marketplace add librejustice/librejustice
codex plugin add librejustice@librejustice
```

À la première utilisation, l'agent ouvre le navigateur pour connecter (ou
créer) un compte librejustice.fr. OAuth 2.1 avec enregistrement dynamique de
client : aucune clé à configurer.

## Contenu

| Composant | Rôle |
|---|---|
| Serveur MCP `librejustice` | `https://librejustice.fr/mcp` : `search_decisions`, `get_decision`, `get_legal_text`, `search_legal_texts`, `list_my_activity` |
| Skill `recherche-jurisprudence` | trouver, vérifier et citer les décisions (deux ordres), avec lien systématique |
| Skill `recherche-normes` | le texte applicable à date : codes FR versionnés, traités, droit UE, codes étrangers, conventions collectives, BOFiP |

Le serveur MCP est aussi utilisable seul, dans tout client MCP (Le Chat,
Perplexity, claude.ai…) en connecteur custom : URL
`https://librejustice.fr/mcp`.
