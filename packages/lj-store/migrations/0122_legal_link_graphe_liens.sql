-- ADR 0174 : `legal_link` — graphe de liens DILA (bloc <LIENS>), miroir fidèle
-- par propriétaire. Une ligne par <LIEN> tel qu'il apparaît dans le fichier de
-- son propriétaire (version d'article, ou texte lui-même : owner_num_key = ''
-- et owner_date_debut = '0001-01-01', sentinelles alignées sur legal_article).
-- Cible en clé pendante (IDs DILA : LEGIARTI/LEGISCTA/LEGITEXT/JORFTEXT/KALI*),
-- résolue au read-time contre legal_text/legal_article — pas de FK : la cible
-- (et même l'owner) peut être ingérée après l'arête, comme texte/article.

CREATE TABLE legal_link (
    owner_text_uid   TEXT NOT NULL,
    owner_num_key    TEXT NOT NULL,
    owner_date_debut DATE NOT NULL,
    seq              INTEGER NOT NULL,
    -- typelien brut DILA (22 valeurs observées) + famille normalisée `verb`
    -- (cite|modifie|cree|abroge|codifie|concorde|…) + `direction` vue de l'owner
    -- (outgoing = il agit/cite, incoming = il subit/est cité, de `sens`).
    typelien         TEXT NOT NULL,
    verb             TEXT NOT NULL,
    direction        TEXT NOT NULL,
    -- Cible : grain (article|section|texte), ID DILA, texte porteur (cidtexte),
    -- n° d'article brut + normalisé, nature, libellé verbatim, date de
    -- signature (sentinelles absorbées), NOR.
    target_kind      TEXT NOT NULL,
    target_uid       TEXT,
    target_text_uid  TEXT,
    target_num       TEXT,
    target_num_key   TEXT,
    target_nature    TEXT,
    target_label     TEXT NOT NULL,
    target_date      DATE,
    target_nor       TEXT,
    PRIMARY KEY (owner_text_uid, owner_num_key, owner_date_debut, seq)
);

-- Requêtes inverses (« qui modifie/cite X ? ») : par ID DILA cible, et par
-- (texte, n° normalisé) pour les liens sans ID.
CREATE INDEX idx_legal_link_target_uid ON legal_link (target_uid)
    WHERE target_uid IS NOT NULL;
CREATE INDEX idx_legal_link_target_art ON legal_link (target_text_uid, target_num_key)
    WHERE target_text_uid IS NOT NULL;

-- Résolution read-time `target_uid` (LEGIARTI/KALIARTI…) → version d'article.
CREATE INDEX idx_legal_article_source_uid ON legal_article (source_uid);
