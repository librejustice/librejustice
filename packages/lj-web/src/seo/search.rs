//! SEO de la page /recherche.
//!
//! La route `/recherche` n'a PAS de meta propre côté React : elle hérite du
//! title/description racine (`site_default()`), comme les pages statiques. Le
//! React actuel n'expose aucun `meta` dynamique « Recherche : {q} » — on ne
//! l'invente donc pas (règle #16, #11). Ce module reste vide tant que la spec
//! produit n'introduit pas un title dynamique ; il existe pour matérialiser la
//! frontière SEO de la tranche (le `mod.rs` figé le déclare).
