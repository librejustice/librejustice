//! Cible lib de `lj-ingest` (précédent : le chunker, ADR 0081). N'expose que
//! le pont d'extraction (`extract`) : c'est LA fonction d'extraction du
//! système — le banc (`lj-bench`) l'importe pour scorer exactement l'artefact
//! que l'ingest persiste, sans plomberie parallèle. Le reste (pipelines,
//! CLI, cron) demeure privé au binaire.

pub mod extract;
