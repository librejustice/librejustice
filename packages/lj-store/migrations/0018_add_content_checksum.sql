-- Transition sha256 → xxh3 : colonne intermédiaire content_checksum (xxhash 64-bit).
-- content_hash (sha256) reste jusqu'à ce que toutes les lignes aient leur checksum,
-- puis sera supprimé dans une migration ultérieure.

ALTER TABLE decisions ADD COLUMN content_checksum text;
