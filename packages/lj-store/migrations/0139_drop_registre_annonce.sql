-- 0139 — retrait du stock local d'annonces de registres (ADR 0199,
-- supersede l'acquisition de l'ADR 0197). La chronologie BODACC/JOAFE de la
-- fiche entité est servie à l'affichage par les APIs publiques (BODACC et
-- JOAFE Opendatasoft DILA par SIREN/RNA, API RNE INPI pour les documents),
-- pas par un stock local : 59 Go pour une donnée disponible en live.
DROP TABLE IF EXISTS registre_annonce;
DROP TABLE IF EXISTS registre_annonce_file;
