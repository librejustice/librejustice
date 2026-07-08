//! Logging applicatif (port de `apps/ingest/librejustice/logging.py`).
//!
//! Ce qu'on garde du module Python :
//!
//! - Niveau `TRACE` sous `DEBUG` pour les mesures très verbeuses ;
//! - Parsing de l'env `LIBREJUSTICE_LOG_LEVEL` (`trace`/`debug`/`info`/
//!   `warning`/`error`/`critical` ou un entier) ;
//! - Un seul subscriber stdout global (idempotent, pas de double-install) ;
//! - Format JSON optionnel si `LOG_FORMAT=json`, sinon format texte
//!   `<ts> <LEVEL> <target>: <msg>`.
//!
//! Côté Python le tracing applicatif utilise `logging` (niveaux entiers) ;
//! côté Rust on s'appuie sur `tracing` + `tracing-subscriber`. Les `**data`
//! structurés du `TraceLogger` deviennent des champs de span/event `tracing`
//! (`info!(count = 42, ms = 3.1, "upsert fait")`), captés nativement par le
//! formatter (texte ou JSON).
//!
//! L'installation du subscriber vit désormais dans `lj-telemetry` (subscriber
//! et export OTLP partagés api/ingest, ADR 0062). Ce module se limite à
//! résoudre le niveau (parité Python) et le format pour les `InitOpts`.

use tracing::Level;

/// Parse un niveau depuis une string (`info`, `trace`, …) ou un entier.
///
/// Port de `_parse_log_level` : valeur absente/vide → `INFO`. Les noms Python
/// se mappent sur les niveaux `tracing` (pas de `CRITICAL` distinct → `ERROR`).
/// Une string entière reprend le seuil `logging` Python : `>= 50` → error,
/// `>= 40` → error, `>= 30` → warn, `>= 20` → info, `>= 10` → debug, sinon
/// trace.
fn parse_log_level(value: Option<&str>) -> Level {
    let Some(raw) = value else {
        return Level::INFO;
    };
    let raw = raw.trim().to_ascii_lowercase();
    if raw.is_empty() {
        return Level::INFO;
    }
    match raw.as_str() {
        "trace" => Level::TRACE,
        "debug" => Level::DEBUG,
        "info" => Level::INFO,
        "warning" | "warn" => Level::WARN,
        "error" => Level::ERROR,
        // `logging.CRITICAL` n'a pas d'équivalent `tracing` : on rabat sur ERROR.
        "critical" => Level::ERROR,
        other => match other.parse::<i32>() {
            Ok(n) => level_from_int(n),
            // Comportement Python : valeur non reconnue → INFO.
            Err(_) => Level::INFO,
        },
    }
}

/// Mappe un seuil entier façon `logging` (CRITICAL=50…TRACE=5) vers `tracing`.
fn level_from_int(n: i32) -> Level {
    if n >= 40 {
        Level::ERROR
    } else if n >= 30 {
        Level::WARN
    } else if n >= 20 {
        Level::INFO
    } else if n >= 10 {
        Level::DEBUG
    } else {
        Level::TRACE
    }
}

/// Résout le niveau de log effectif (port de la précédence `configure_logging` :
/// argument explicite > `LIBREJUSTICE_LOG_LEVEL` > `INFO`).
pub fn resolve_level(level: Option<&str>) -> Level {
    match level {
        Some(value) => parse_log_level(Some(value)),
        None => parse_log_level(std::env::var("LIBREJUSTICE_LOG_LEVEL").ok().as_deref()),
    }
}

/// Format de sortie du layer fmt : `LOG_FORMAT=json` → JSON, sinon texte.
pub fn json_format() -> bool {
    matches!(std::env::var("LOG_FORMAT"), Ok(v) if v == "json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing::Level;

    // Spec : _parse_log_level (noms, casse insensible, défaut INFO).
    #[test]
    fn parse_named_levels() {
        assert_eq!(parse_log_level(Some("trace")), Level::TRACE);
        assert_eq!(parse_log_level(Some("DEBUG")), Level::DEBUG);
        assert_eq!(parse_log_level(Some(" Info ")), Level::INFO);
        assert_eq!(parse_log_level(Some("warning")), Level::WARN);
        assert_eq!(parse_log_level(Some("error")), Level::ERROR);
        assert_eq!(parse_log_level(Some("critical")), Level::ERROR);
    }

    #[test]
    fn parse_default_and_unknown() {
        assert_eq!(parse_log_level(None), Level::INFO);
        assert_eq!(parse_log_level(Some("")), Level::INFO);
        assert_eq!(parse_log_level(Some("nonsense")), Level::INFO);
    }

    // Spec : un entier est interprété comme un seuil logging Python.
    #[test]
    fn parse_int_levels() {
        assert_eq!(parse_log_level(Some("5")), Level::TRACE);
        assert_eq!(parse_log_level(Some("10")), Level::DEBUG);
        assert_eq!(parse_log_level(Some("20")), Level::INFO);
        assert_eq!(parse_log_level(Some("30")), Level::WARN);
        assert_eq!(parse_log_level(Some("50")), Level::ERROR);
    }
}
