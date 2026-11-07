# [LibreJustice](https://librejustice.fr)

**The connector and the two skills that frame it, installed at once: all of French law in your agent, and the method to cite it without mistakes.**

The connector alone already answers. The two skills decide what the agent does with it: what it reads before citing, at which date it reads a text, and what it checks before calling a decision an authority.

## What the plugin installs

| Component | Role |
| -- | -- |
| The `librejustice` connector | ~3.9 million French-language and European decisions, ~2.1 million articles in force and ~1.8 million historical versions, linked article by article, four tools |
| The [`recherche-jurisprudence`](skills/recherche-jurisprudence/SKILL.md) skill | no decision enters the answer unless its full text was read, quotations are verbatim and linked, the outcome on appeal is shown before the decision serves as an authority |
| The [`recherche-normes`](skills/recherche-normes/SKILL.md) skill | the date first, then the text: today's version is the wrong one for most disputes |

## The collection

Administrative case law (tribunaux administratifs, cours administratives d'appel, Conseil d'État) and judicial case law (Cour de cassation, cours d'appel, tribunaux judiciaires and de commerce), Conseil constitutionnel, CNIL, Tribunal des conflits, ECHR since 1960, CJEU, CNDA (public decisions only).

Codes and statutes, the Journal officiel linked to what each issue amends or repeals, collective agreements (KALI), circulaires, BOFiP, EU law, treaties and bilateral agreements consolidated amendment by amendment, foreign codes from 59 countries for the cases where a French court applies foreign law.

Every text reads as in force on any date, with the history of its versions, and is searched at that date. The citation graph runs both ways: from a decision to the articles it cites, from an article to the decisions that cite it. The corpus is resynchronised daily.

## Installation

**ChatGPT and Codex**: install [LibreJustice](https://chatgpt.com/plugins/plugin_asdk_app_6a053194fe5c81918e1a3b98ab1f4d63) from the app directory. It carries the connector and both skills, the free plan is enough, and Codex picks it up on its own: both clients share the same account.

**Claude**: install [LibreJustice](https://claude.ai/directory/connectors/librejustice) from the connector directory; the two skills are added separately.

**Claude Code**, and Codex without the directory: add the repository as a source, then install the plugin.

```
/plugin marketplace add librejustice/librejustice
/plugin install librejustice@librejustice
```

```
codex plugin marketplace add librejustice/librejustice
codex plugin add librejustice@librejustice
```

**Elsewhere (Le Chat, Perplexity...)**: add a custom MCP connector at `https://librejustice.fr/mcp/`, streamable HTTP, OAuth method.

On first use, the agent opens the browser to connect a librejustice.fr account, or create one. The account is free.

Step by step for each client, plan requirements included: [the installation guide](https://librejustice.fr/mcp-guide).

## Access

Free and open source, under the [Apache-2.0](https://github.com/librejustice/librejustice) licence. A free account is enough for the connector; browsing [librejustice.fr](https://librejustice.fr) needs no sign-up.
