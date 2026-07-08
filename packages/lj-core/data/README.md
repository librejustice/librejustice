# Données embarquées `lj-core`

Embarquées via `include_str!` (cf. règle « données statiques »). Pures (aucune I/O
au runtime).

## `accents_fr.txt` — lexique de restauration d'accents

Une forme accentuée par ligne. Consommé par `src/truecase.rs` : la clé de
recherche est la forme repliée (`fold`), la valeur la forme accentuée. Sert à
recasser/réaccentuer les vieilles décisions en MAJUSCULES *sans accents* (vieux
fonds Cassation) — affichage uniquement.

- **Source** : [Lexique383](http://www.lexique.org) (Lexique 3.83), colonnes
  `ortho`, `freqfilms2`, `freqlivres`.
- **Licence** : CC BY-SA 4.0 — attribution New, B., Pallier, C., Ferrand, L.,
  Matos, R. (2001) *Une base de données lexicales du français contemporain sur
  internet : LEXIQUE*. L'Année Psychologique.
- **Construction** : pour chaque clé repliée, on retient la forme accentuée la
  plus fréquente **seulement si** l'orthographe sans accent est rare comme mot
  réel (`freq(sans-accent)/total ≤ 0.15`). Ce filtre de dominance protège les
  ambigus (« a/à », « ou/où », « sur/sûr », « du/dû », « cote/côté ») en les
  laissant intacts.

## `accents_supplement_fr.txt` — complément lexique (formes sans ambiguïté)

Même rôle et même format que `accents_fr.txt`, chargé dans la même table par
`truecase.rs`. Fichier séparé uniquement pour rester sous le plafond du hook
`check-added-large-files --maxkb=200` (chaque fichier < 200 Ko).

- **Source** : Lexique383 + supplément juridique curé (termes genrés/jargon
  absents de Lexique : « préfète », « énonciations », « irrépétibles »,
  « intimée », « susvisé »…) + suffixes ordinaux accolés aux chiffres
  (« ème », « ère » : « 2ème », « 1ère » — repliés « eme »/« ere », sans homographe).
- **Construction** : formes dont l'orthographe sans accent est *quasi* inexistante
  comme mot réel (`freq(sans-accent)/total ≤ 0.05`), restreintes aux participes
  passés accordés (`-ée/-és/-ées`), aux noms/adjectifs et aux adverbes — on exclut
  les temps littéraires (passé simple, subjonctif imparfait) absents des décisions.
  Récupère le vocabulaire juridique (« greffière », « requérante », « désistement »,
  « récursoire »), les adverbes (« postérieurement », « antérieurement ») et les
  participes féminins/pluriels (« attaquée », « déférée ») sans risque de précision.

## `participles_fr.txt` — participes passés homographes (`-é`)

Participes en `-é` dont l'orthographe sans accent est un mot réel courant
(présent ou nom : « condamne »/« condamné », « attaque »/« attaqué ») — donc
**exclus** du lexique par le filtre de dominance. Restaurés par `truecase.rs`
seulement quand un auxiliaire précède (« a condamné », « est fondé ») : signal
mesuré ≥ 97 % de précision sur la GT recasse.

- **Format** : `folded<TAB>accentué<TAB>tier`, `tier ∈ {s, w}` (`s` = lecture
  participe dominante dans Lexique → « a » nu admis ; `w` = nom/présent dominant
  → auxiliaire strict requis).
- **Source / construction** : Lexique383 (`cgram=VER`, `infover ~ par:pas`),
  filtrés des participes féminins irréguliers (« mise », « prise ») et des
  collisions où le verbe en `-er` est dwarfé par le nom homographe.

## `participle_context_fr.txt` — modèle contextuel participe `-é` (ADR 0072)

Désambiguïse le participe masc. `-é`/`-és` (présent/nom « condamne » vs participe
« condamné » ; double homographe « arrête »/« arrêté » ; pluriels « exposés »,
« liquidés », « tirés ») d'après les voisins. Consulté par `truecase.rs` **avant**
le lexique (il corrige les doubles homographes que le lexique fixait à tort), avec
repli sur la règle auxiliaire.

- **Format** : `folded<TAB>forme_é<TAB>forme_alt<TAB>défaut(e|é)<TAB>flips` —
  `forme_é` = participe accentué, `forme_alt` = lecture alternative (vide ⇒ non
  accentuée), `flips` = voisins repliés qui inversent la décision par défaut
  (`p<token>` = mot précédent, `n<token>` = mot suivant ; `^`/`$` = début/fin).
- **Source / construction** : appris hors-ligne sur ~29 000 décisions accentuées
  en casse mixte (`apps/lj-bench/gt/ranking/_bodies_*`), **disjointes de la GT
  recasse** (exclusion par `source_uid`). On retient la lecture majoritaire par
  forme et les contextes voisins ≥ 95 % décisifs (précision ≈ 99 % sur la cible).
  Les pluriels `-és` (clés repliées en `es`, longueur > 4 pour écarter « des »/
  « les ») ne sont émis qu'à pureté participe ≥ 95 % dans le corpus — sinon laissés
  hors table. Déterministe ; régénération = re-parcours du corpus.

## `a_aux_next_fr.txt` / `a_aux_prev_fr.txt` — désambiguïsation « a »/« à »

Listes de mots repliés (un par ligne) qui tranchent l'homographe « a » (auxiliaire/
verbe avoir) vs « à » (préposition) sur les voisins immédiats (cf. `disambiguate_a`).
Défaut **préposition** « à » ; on garde « a » nu si le mot **suivant** est dans
`a_aux_next_fr.txt` (« a été », « a pas », « a lieu », « a condamné », « a donc ») ou
le mot **précédent** dans `a_aux_prev_fr.txt` (sujet élidé/pronom/titre : « il a »,
« n'a », « qui a », « les a », « Mme a »).

- **Source / construction** : `a_aux_next_fr.txt` appris hors-ligne sur les décisions
  accentuées disjointes de la GT (mots suivant « a/à » à pureté « a » ≥ 97 %, freq
  ≥ 80), unis aux participes passés irréguliers connus (« fait », « dit », « dû »…).
  `a_aux_prev_fr.txt` est une liste curée de sujets élidés / pronoms / titres
  (frequence-filtrée puis nettoyée des artefacts OCR). Précision « à » ≈ 97 % mesurée.

## `proper_nouns_fr.txt` — gazetteer de noms propres (lieux/juridictions)

Une forme propre par ligne (déjà capitalisée + accentuée : « Versailles »,
« Nîmes »). Recapitalise les noms de communes/départements/régions dans le
truecasing.

- **Source** : [API Découpage administratif](https://geo.api.gouv.fr)
  (départements, régions, communes ≥ 8 000 hab.).
- **Licence** : Licence Ouverte / Open Licence (Etalab).
- **Construction** : noms mono-token (≥ 4 lettres), **exclus** s'ils collisionnent
  avec un mot français courant (« sens », « vienne », « tour », « die »…) sauf
  sièges de cours d'appel.

## Régénération

Scripts ad hoc (réseau requis) ; sortie déterministe. Les fichiers source
intermédiaires vivent sous `data/accents/` (gitignoré, scratch). Voir l'historique
git du commit d'introduction pour le pipeline Python exact (Lexique383 +
geo.api.gouv.fr → filtre de dominance / anti-collision).

## `legifrance_codes.json`

Snapshot des titres de codes Légifrance (cf. `src/data.rs`).
