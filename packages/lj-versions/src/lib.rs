//! `lj-versions`: the versions stamped on the derived columns of `decisions`,
//! compared by the queries that pick what to recompute.

/// Version of the extraction pipeline, stamped as `extract_version` on
/// `decisions` and compared to gate the re-extraction. Written by `lj-extract`,
/// read by `lj-store` in its SQL. Bump it when the extraction logic changes.
pub const EXTRACT_VERSION: i16 = 89;

/// Version of the bodily-injury quantum extraction, an axis apart from
/// `EXTRACT_VERSION` until their cadences converge, the quantum iterating with
/// every validation rule added. Bump it when what is extracted or validated
/// changes.
pub const PC_EXTRACT_VERSION: i16 = 1;

/// Manual revision of the quantum, above any automatic version: `lj-store`
/// refuses to overwrite it, and comparing against it reduces to a filter.
pub const PC_GOLD_VERSION: i16 = 1000;

/// Two automatic readings of a decision, their rows under the publication
/// guards: above any deterministic pass, under the manual revision.
pub const PC_READING_VERSION: i16 = 500;

/// Version of the capture certifier, an axis orthogonal to `EXTRACT_VERSION`. A
/// decision is certified when the high-recall oracle finds nothing more than the
/// recognizer and its citations resolve. The default re-extraction skips the
/// decisions certified at the current version, even on an `EXTRACT_VERSION`
/// bump; the targeted one ignores it. Bump it when the oracle improves.
pub const CERTIFIER_VERSION: i32 = 1;
