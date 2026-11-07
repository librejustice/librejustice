//! `lj-textsearch`: the text analysis shared by the search index and the
//! ingest: the BM25 body tokenizer, the collocations gazetteer, the usage terms
//! and the suggest n-grams.

pub mod body_tok;
pub mod collocations;
pub mod highlight;
pub mod suggest;
pub mod usage;
