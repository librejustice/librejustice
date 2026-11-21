# [LibreJustice](https://librejustice.fr)

**The plugin installs the LibreJustice connector and two skills at once. Your agent reaches case law and legal texts, and follows a method to cite them without mistakes.**

The connector answers on its own. The two skills decide what the agent does with it: what it reads before citing, at which date it reads a text and what it checks before presenting a decision as an authority.

## What the plugin installs

| Component | Role |
| -- | -- |
| The `librejustice` connector | It gives access to ~5 million decisions from French, European, Swiss, Belgian, Luxembourg, Dutch and African courts, linked article by article to ~2.1 million articles in force and ~1.8 million past versions. It has six tools, two of them for the official journals. |
| The [`recherche-jurisprudence`](skills/recherche-jurisprudence/SKILL.md) skill | No decision enters the answer unless it was read in full. Quotations are word for word with a link, and the decision's fate on appeal or in cassation is checked before it serves as an authority. |
| The [`recherche-normes`](skills/recherche-normes/SKILL.md) skill | It first sets the date that matters, then quotes the version of the text in force on that date. In most disputes, today's version is the wrong one. |

## The collection

Case law covers the administrative order (tribunaux administratifs, cours administratives d'appel, Conseil d'État) and the judicial order (Cour de cassation, cours d'appel, tribunaux judiciaires and de commerce, conseils de prud'hommes). It also includes the Conseil constitutionnel, the Tribunal des conflits, the Défenseur des droits, the CNIL, the Autorité de la concurrence, the AMF, the ECHR since 1960, the CJEU and the CNDA (public decisions only). The same engine searches case law from Switzerland, Belgium, Luxembourg, the Netherlands, Monaco and Canada, and from 25 African countries and the community courts, OHADA's CCJA among them.

The texts include codes and statutes, the Journal officiel linked to what each issue amends or repeals, collective agreements (KALI), circulaires, BOFiP and the Service-Public practical sheets. EU law is there too, with treaties and bilateral agreements consolidated amendment by amendment. Swiss federal and cantonal law and Belgian regional and community law are there too. Codes from 59 countries cover the cases where a French court applies foreign law.

The connector searches 24 official journals, among them the Journal officiel, the French ministries' bulletins, the Swiss Recueil officiel and the regional and community editions of the Moniteur belge. It also searches open public tenders, company notices (BODACC) and association notices (JOAFE).

Texts read as they stood on the chosen date, with the history of their versions, and are searched at that date. Citations can be followed both ways, from a decision to the articles it cites and from an article to the decisions that cite it. Each decision and each article links to its official publisher. The corpus is resynchronised daily.

## Installation

**ChatGPT and Codex**: install [LibreJustice](https://chatgpt.com/plugins/plugin_asdk_app_6a053194fe5c81918e1a3b98ab1f4d63) from the app directory. The app carries the connector and both skills, and the free plan is enough. Codex picks it up on its own, since both clients share the same account.

**Claude**: install [LibreJustice](https://claude.ai/directory/connectors/librejustice) from the connector directory. The two skills are added separately.

**Claude Code and Codex outside the directory**: add the repository as a source, then install the plugin.

```
/plugin marketplace add librejustice/librejustice
/plugin install librejustice@librejustice
```

```
codex plugin marketplace add librejustice/librejustice
codex plugin add librejustice@librejustice
```

**Elsewhere (Le Chat, Perplexity...)**: add a custom MCP connector at `https://librejustice.fr/mcp/`, over streamable HTTP with the OAuth method.

On first use, the agent opens the browser to connect a librejustice.fr account or create one. The account is free.

The [installation guide](https://librejustice.fr/mcp-guide) walks through each client step by step, with the plan it requires.

## Access

LibreJustice is free and open source, under the [Apache-2.0](https://github.com/librejustice/librejustice) licence. A free account is enough for the connector, and browsing [librejustice.fr](https://librejustice.fr) needs no sign-up.
