# Reaching a judge, a track or a kind of decision

A judge sitting by function and a composition of the bench are `formation` paths, under the court type: take them from `facets.formation`. A track has no axis of its own: a kind of référé, a QPC, an avis or a referral is reached by the article that founds it, with `legal_article`. Cross the two when both matter, for instance `formation: ["FR.TA.JUGE_REFERES"]` with `legal_article: ["code-de-justice-administrative|L521-2"]`.

## Judges and compositions, by `formation`

- Juge des référés: `"FR.TA.JUGE_REFERES"`, `"FR.CAA.JUGE_REFERES"`, `"FR.CE.JUGE_REFERES"`, `"FR.TJ.JUGE_REFERES"`, `"FR.TCOM.JUGE_REFERES"`, `"FR.CPH.JUGE_REFERES"`.
- JEX, JAF, JLD, JCP, juge des enfants: `"FR.TJ.JEX"`, `"FR.TJ.JAF"`, `"FR.TJ.JLD"`, `"FR.TJ.JCP"`, `"FR.TJ.JUGE_ENFANTS"`.
- Juge or conseiller de la mise en état: `"FR.TJ.MISE_EN_ETAT"`, `"FR.CA.MISE_EN_ETAT"`.
- Premier président, delegates included: `"FR.CA.PREMIER_PRESIDENT"`, `"FR.CC.PREMIER_PRESIDENT"`. The JLD's rétention and psychiatric-care orders are appealed to the premier président of the cour d'appel.
- Cour de cassation, by the code its decisions print: `"FR.CC.RESTREINTE"` (F), `"FR.CC.SECTION"` (FS), `"FR.CC.PLENIERE"` (FP).
- Single judge, three or five judges: `"FR.TA.JUGE_UNIQUE"`, `"FR.TA.A_TROIS"`, `"FR.CAA.A_CINQ"`, and so on under each court type.

A court type alone (`"FR.TJ"`) matches every formation under it.

## Tracks, by `legal_article`

- Référé-suspension, référé-liberté, référé mesures utiles: `"code-de-justice-administrative|L521-1"`, `"code-de-justice-administrative|L521-2"`, `"code-de-justice-administrative|L521-3"`.
- Référé précontractuel and contractuel: `"code-de-justice-administrative|L551-1"`, `"code-de-justice-administrative|L551-13"`.
- Référé-provision, référé-expertise, référé-constat: `"code-de-justice-administrative|R541-1"`, `"code-de-justice-administrative|R532-1"`, `"code-de-justice-administrative|R531-1"`.
- Civil référé, for urgency or a manifestly unlawful disturbance: `"code-de-procedure-civile|834"`, `"code-de-procedure-civile|835"`; at the tribunal de commerce, `"code-de-procedure-civile|872"`, `"code-de-procedure-civile|873"`.
- Mesure d'instruction in futurum: `"code-de-procedure-civile|145"`.
- QPC: `"constitution-du-4-octobre-1958|61-1"`. By stage, ordonnance 58-1067 art. 23-1 (raised before a trial court), 23-2 (its transmission) and 23-5 (raised before the Conseil d'État or the Cour de cassation), as `"ordonnance-n-58-1067-du-7-novembre-1958-portant-loi-organique-sur-le-conseil|23-5"`. A Cour de cassation decision on a QPC cites the provision under review rather than these: there, put the words in the query, `question prioritaire de constitutionnalité` with `renvoyer` or `transmettre`, under `seat: ["FR.CC"]`.
- Avis: Cour de cassation `"code-de-l-organisation-judiciaire|L441-1"`, Conseil d'État `"code-de-justice-administrative|L113-1"`.
- Non-admission of a pourvoi at the Conseil d'État: `"code-de-justice-administrative|L822-1"`.
- Preliminary reference to the CJUE: `"traite-sur-le-fonctionnement-de-l-union-europeenne|267"`.
